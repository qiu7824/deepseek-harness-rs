// Included inside private_read_acl::tests so these probes exercise the actual
// reconciliation entry point and Windows filesystem access checks together.
mod navigation_behavior {
    use super::*;

    const NAVIGATION: u32 = FILE_READ_ATTRIBUTES | FILE_TRAVERSE;
    const SYNTHETIC_ACCOUNT_A: &str = "S-1-5-21-678-901-234-1001";
    const SYNTHETIC_ACCOUNT_B: &str = "S-1-5-21-678-901-234-1002";
    const SYNTHETIC_GROUP: &str = "S-1-5-21-678-901-234-2001";
    const SYNTHETIC_CAP_A: &str = "S-1-15-3-1024-678-901-234-1001";
    const SYNTHETIC_CAP_B: &str = "S-1-15-3-1024-678-901-234-1002";

    struct Impersonation;

    impl Impersonation {
        fn enter(token: HANDLE) -> Result<Self> {
            ensure!(
                unsafe { ImpersonateLoggedOnUser(token) } != 0,
                "impersonate restricted fixture token failed: {}",
                unsafe { GetLastError() }
            );
            Ok(Self)
        }
    }

    impl Drop for Impersonation {
        fn drop(&mut self) {
            assert_ne!(unsafe { RevertToSelf() }, 0, "restore fixture host token");
        }
    }

    fn strict_token(sids: &[*mut c_void]) -> Result<Handle> {
        let base = Handle(unsafe { crate::token::get_current_token_for_restriction()? });
        let mut restrictions: Vec<_> = sids
            .iter()
            .map(|sid| SID_AND_ATTRIBUTES {
                Sid: *sid,
                Attributes: 0,
            })
            .collect();
        let mut token = 0;
        // Do not use the production token helper here: WRITE_RESTRICTED would
        // skip the restricting-SID check for reads, and its Everyone/logon SIDs
        // could admit objects independently of the fixture account/capability.
        ensure!(
            unsafe {
                CreateRestrictedToken(
                    base.0,
                    0x01, // DISABLE_MAX_PRIVILEGE; deliberately no WRITE_RESTRICTED.
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                    restrictions.len() as u32,
                    restrictions.as_mut_ptr(),
                    &mut token,
                )
            } != 0,
            "create strict fixture token failed: {}",
            unsafe { GetLastError() }
        );
        Ok(Handle(token))
    }

    fn as_token<T>(token: &Handle, probe: impl FnOnce() -> Result<T>) -> Result<T> {
        let _impersonation = Impersonation::enter(token.0)?;
        probe()
    }

    fn open(path: &Path, access: u32, disposition: u32, flags: u32) -> Result<Handle> {
        let wide = crate::winutil::to_wide_file_path(path)?;
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                disposition,
                flags,
                0,
            )
        };
        ensure!(
            handle != INVALID_HANDLE_VALUE,
            "fixture open denied for {} (access {access:#x}, Win32 {})",
            path.display(),
            unsafe { GetLastError() }
        );
        Ok(Handle(handle))
    }

    fn denied(token: &Handle, path: &Path, access: u32, disposition: u32) -> Result<()> {
        let wide = crate::winutil::to_wide_file_path(path)?;
        as_token(token, || {
            let handle = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    access,
                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                    std::ptr::null(),
                    disposition,
                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
                    0,
                )
            };
            let error = unsafe { GetLastError() };
            if handle != INVALID_HANDLE_VALUE {
                drop(Handle(handle));
                anyhow::bail!(
                    "unexpected fixture access to {} (access {access:#x})",
                    path.display()
                );
            }
            ensure!(
                error == ERROR_ACCESS_DENIED,
                "expected access denied for {}, got Win32 {error}",
                path.display()
            );
            Ok(())
        })
    }

    fn directory_metadata(token: &Handle, path: &Path) -> Result<()> {
        as_token(token, || {
            // An overlapped handle avoids an implicit SYNCHRONIZE requirement:
            // this probe requests exactly the two navigation rights.
            let handle = open(
                path,
                NAVIGATION,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
            )?;
            let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
            ensure!(
                unsafe { GetFileInformationByHandle(handle.0, &mut info) } != 0,
                "query owned ancestor metadata failed: {}",
                unsafe { GetLastError() }
            );
            ensure!(
                info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
                "navigation fixture must remain a directory"
            );
            Ok(())
        })
    }

    fn private_directory_denied(token: &Handle, path: &Path) -> Result<()> {
        // Reject attributes independently: denial of the combined navigation
        // mask alone could hide a remaining metadata-only permission leak.
        denied(token, path, FILE_READ_ATTRIBUTES, OPEN_EXISTING)?;
        denied(token, path, NAVIGATION, OPEN_EXISTING)?;
        denied(token, path, FILE_LIST_DIRECTORY, OPEN_EXISTING)
    }

    fn read_file(token: &Handle, path: &Path, expected: &[u8]) -> Result<()> {
        as_token(token, || {
            let handle = open(path, GENERIC_READ, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL)?;
            let mut bytes = [0u8; 64];
            let mut read = 0;
            ensure!(
                unsafe {
                    ReadFile(
                        handle.0,
                        bytes.as_mut_ptr().cast(),
                        bytes.len() as u32,
                        &mut read,
                        std::ptr::null_mut(),
                    )
                } != 0,
                "read permitted fixture file failed: {}",
                unsafe { GetLastError() }
            );
            ensure!(
                &bytes[..read as usize] == expected,
                "fixture file content changed unexpectedly"
            );
            Ok(())
        })
    }

    fn read_write_file(token: &Handle, path: &Path) -> Result<()> {
        as_token(token, || {
            let handle = open(
                path,
                GENERIC_READ | GENERIC_WRITE,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
            )?;
            let value = b"changed";
            let mut written = 0;
            ensure!(
                unsafe {
                    WriteFile(
                        handle.0,
                        value.as_ptr().cast(),
                        value.len() as u32,
                        &mut written,
                        std::ptr::null_mut(),
                    )
                } != 0
                    && written as usize == value.len(),
                "write permitted fixture file failed: {}",
                unsafe { GetLastError() }
            );
            ensure!(
                unsafe { SetFilePointerEx(handle.0, 0, std::ptr::null_mut(), FILE_BEGIN) } != 0,
                "rewind permitted fixture file failed: {}",
                unsafe { GetLastError() }
            );
            let mut bytes = [0u8; 7];
            let mut read = 0;
            ensure!(
                unsafe {
                    ReadFile(
                        handle.0,
                        bytes.as_mut_ptr().cast(),
                        bytes.len() as u32,
                        &mut read,
                        std::ptr::null_mut(),
                    )
                } != 0
                    && read as usize == bytes.len()
                    && &bytes == value,
                "read back permitted fixture write failed"
            );
            Ok(())
        })
    }

    fn navigation_without_listing(token: &Handle, paths: &[PathBuf]) -> Result<()> {
        for path in paths {
            directory_metadata(token, path)?;
            denied(token, path, FILE_LIST_DIRECTORY, OPEN_EXISTING)?;
            let target = path.join("must-not-be-created.txt");
            denied(token, &target, GENERIC_WRITE, CREATE_NEW)?;
            ensure!(!target.exists(), "private ancestor admitted a new file");
        }
        Ok(())
    }

    #[test]
    fn owned_navigation_allows_real_access_without_listing_private_ancestors() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let private = temp.path().join("private");
        let scratch = private.join("scratch");
        let content = scratch.join("content");
        let owned = content.join("owner-a");
        let worktree = owned.join("worktree");
        let foreign = content.join("owner-b/worktree");
        let source = private.join("source/project");
        for path in [&worktree, &foreign, &source] {
            std::fs::create_dir_all(path)?;
        }
        let source_file = source.join("input.txt");
        let owned_file = worktree.join("result.txt");
        let foreign_file = foreign.join("other.txt");
        let secret = private.join("settings.json");
        std::fs::write(&source_file, b"source")?;
        std::fs::write(&owned_file, b"initial")?;
        std::fs::write(&foreign_file, b"foreign")?;
        std::fs::write(&secret, b"private")?;
        let account = LocalSid::from_string(SYNTHETIC_ACCOUNT_A)?;
        let group = LocalSid::from_string(SYNTHETIC_GROUP)?;
        let cap = LocalSid::from_string(SYNTHETIC_CAP_A)?;
        let account_token = strict_token(&[account.as_ptr(), group.as_ptr()])?;
        let sdk_token = strict_token(&[account.as_ptr(), group.as_ptr(), cap.as_ptr()])?;
        let state_home = temp.path().join("state");
        // Begin with a reconciled, fully private boundary. Granting owned
        // descendants must replace existing full denials, not merely work on
        // a fresh tree that has never had the account/group deny ACEs.
        unsafe {
            sync_private_read_acls(
                &state_home,
                &[private.clone(), scratch.clone()],
                &[],
                &[],
                &[account.as_ptr()],
                group.as_ptr(),
            )?;
        }
        private_directory_denied(&account_token, &private)?;
        private_directory_denied(&account_token, &scratch)?;
        unsafe {
            // Match the SDK's existing capability grants on exact runtime
            // roots; ancestors receive no broad read/write capability grant.
            crate::acl::ensure_allow_write_aces(&owned, &[cap.as_ptr()])?;
            crate::acl::ensure_allow_mask_aces(
                &source,
                &[cap.as_ptr()],
                FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
            )?;
            sync_private_read_acls(
                &state_home,
                &[private.clone(), scratch.clone()],
                std::slice::from_ref(&source),
                std::slice::from_ref(&owned),
                &[account.as_ptr()],
                group.as_ptr(),
            )?;
        }
        for token in [&account_token, &sdk_token] {
            navigation_without_listing(
                token,
                &[private.clone(), scratch.clone(), content.clone()],
            )?;
            directory_metadata(token, &worktree)?;
            read_write_file(token, &owned_file)?;
            read_file(token, &source_file, b"source")?;
            denied(token, &source_file, GENERIC_WRITE, OPEN_EXISTING)?;
            denied(token, &secret, GENERIC_READ, OPEN_EXISTING)?;
            denied(token, &secret, FILE_READ_ATTRIBUTES, OPEN_EXISTING)?;
            private_directory_denied(token, &foreign)?;
            denied(token, &foreign_file, GENERIC_READ, OPEN_EXISTING)?;
            denied(token, &foreign_file, FILE_READ_ATTRIBUTES, OPEN_EXISTING)?;
        }

        // Reproduce a completed v2 journal with the same owned/source grants.
        // Its boundary fingerprints deliberately match the old full-denial
        // ACLs, so migration cannot rely on a changed root stamp or new grant.
        let state_path = state_home.join(".sandbox/private_read_acl_state.json");
        let mut legacy: State = serde_json::from_slice(&std::fs::read(&state_path)?)?;
        let ancestors = legacy.navigation_ancestors.clone();
        ensure!(
            ancestors.iter().any(|path| key(path) == key(&private))
                && ancestors.iter().any(|path| key(path) == key(&scratch))
                && ancestors.iter().any(|path| key(path) == key(&content)),
            "nested owned grants must register all strict private ancestors"
        );
        for ancestor in &ancestors {
            let boundary = boundary_for(ancestor, &legacy.roots)
                .context("legacy navigation ancestor lost its private boundary")?;
            unsafe {
                reconcile_object(
                    ancestor,
                    boundary,
                    &[account.as_ptr()],
                    group.as_ptr(),
                    false,
                    false,
                    false,
                    key(ancestor) == key(boundary),
                )?;
            }
        }
        legacy.version = 2;
        legacy.navigation_ancestors.clear();
        legacy.pending = false;
        for root in &legacy.roots {
            legacy.migrated.insert(key(root), unsafe {
                root_stamp(root, &[account.as_ptr()], group.as_ptr())
            }?);
        }
        store_state(&state_path, &legacy)?;
        denied(
            &account_token,
            &private,
            FILE_READ_ATTRIBUTES,
            OPEN_EXISTING,
        )?;
        // Keep the grant subtrees intact while making only their ancestors
        // unusable, matching a warm journal from before navigation support.
        read_write_file(&account_token, &owned_file)?;
        read_file(&account_token, &source_file, b"source")?;
        unsafe {
            sync_private_read_acls(
                &state_home,
                &[private.clone(), scratch.clone()],
                std::slice::from_ref(&source),
                std::slice::from_ref(&owned),
                &[account.as_ptr()],
                group.as_ptr(),
            )?;
        }
        let migrated: State = serde_json::from_slice(&std::fs::read(&state_path)?)?;
        ensure!(migrated.version == 3 && !migrated.pending);
        ensure!(migrated.grants == legacy.grants, "migration changed grants");
        for token in [&account_token, &sdk_token] {
            navigation_without_listing(token, &ancestors)?;
            directory_metadata(token, &worktree)?;
            read_write_file(token, &owned_file)?;
            read_file(token, &source_file, b"source")?;
            denied(token, &source_file, GENERIC_WRITE, OPEN_EXISTING)?;
            denied(token, &secret, FILE_READ_ATTRIBUTES, OPEN_EXISTING)?;
            private_directory_denied(token, &foreign)?;
        }

        // Host-created objects after reconciliation must inherit the intended
        // permissions, without another pass adopting unregistered siblings.
        let new_owned = worktree.join("new-owned-directory");
        let unknown = content.join("unregistered-owner");
        std::fs::create_dir(&new_owned)?;
        std::fs::create_dir(&unknown)?;
        let new_owned_file = new_owned.join("new.txt");
        let unknown_file = unknown.join("secret.txt");
        std::fs::write(&new_owned_file, b"initial")?;
        std::fs::write(&unknown_file, b"private")?;
        for token in [&account_token, &sdk_token] {
            directory_metadata(token, &new_owned)?;
            read_write_file(token, &new_owned_file)?;
            private_directory_denied(token, &unknown)?;
            denied(token, &unknown_file, GENERIC_READ, OPEN_EXISTING)?;
            denied(token, &unknown_file, FILE_READ_ATTRIBUTES, OPEN_EXISTING)?;
        }
        ensure!(std::fs::read(&source_file)? == b"source");
        ensure!(std::fs::read(&secret)? == b"private");
        ensure!(std::fs::read(&foreign_file)? == b"foreign");
        ensure!(std::fs::read(&unknown_file)? == b"private");
        Ok(())
    }

    #[test]
    fn warm_owned_navigation_is_bounded_and_switching_grants_revokes_old_ancestors() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let private = temp.path().join("private");
        let branch_a = private.join("owner-a");
        let branch_b = private.join("owner-b");
        let owned_a = branch_a.join("payload/worktree");
        let owned_b = branch_b.join("payload/worktree");
        let foreign = private.join("foreign");
        for path in [&owned_a, &owned_b, &foreign] {
            std::fs::create_dir_all(path)?;
        }
        for index in 0..80 {
            std::fs::write(foreign.join(format!("{index}.txt")), b"foreign")?;
        }
        let file_a = owned_a.join("result.txt");
        let file_b = owned_b.join("result.txt");
        std::fs::write(&file_a, b"initial")?;
        std::fs::write(&file_b, b"initial")?;
        let account = LocalSid::from_string(SYNTHETIC_ACCOUNT_A)?;
        let group = LocalSid::from_string(SYNTHETIC_GROUP)?;
        let cap = LocalSid::from_string(SYNTHETIC_CAP_A)?;
        unsafe {
            crate::acl::ensure_allow_write_aces(&owned_a, &[cap.as_ptr()])?;
            crate::acl::ensure_allow_write_aces(&owned_b, &[cap.as_ptr()])?;
        }
        let state_home = temp.path().join("state");
        let sync = |writes: &[PathBuf]| unsafe {
            sync_private_read_acls(
                &state_home,
                std::slice::from_ref(&private),
                &[],
                writes,
                &[account.as_ptr()],
                group.as_ptr(),
            )
        };
        ensure!(sync(std::slice::from_ref(&owned_a))? > 80);
        for _ in 0..2 {
            ensure!(
                sync(std::slice::from_ref(&owned_a))? <= 12,
                "warm navigation must not enumerate the eighty foreign files"
            );
        }
        let token = strict_token(&[account.as_ptr(), group.as_ptr(), cap.as_ptr()])?;
        directory_metadata(&token, &branch_a)?;
        read_write_file(&token, &file_a)?;
        private_directory_denied(&token, &branch_b)?;

        sync(std::slice::from_ref(&owned_b))?;
        directory_metadata(&token, &private)?;
        directory_metadata(&token, &branch_b)?;
        read_write_file(&token, &file_b)?;
        private_directory_denied(&token, &branch_a)?;
        denied(&token, &file_a, GENERIC_READ | GENERIC_WRITE, OPEN_EXISTING)?;
        denied(&token, &private, FILE_LIST_DIRECTORY, OPEN_EXISTING)?;
        ensure!(
            sync(std::slice::from_ref(&owned_b))? <= 12,
            "grant switches must preserve bounded warm navigation"
        );

        sync(&[])?;
        private_directory_denied(&token, &private)?;
        private_directory_denied(&token, &branch_b)?;
        denied(&token, &file_b, GENERIC_READ | GENERIC_WRITE, OPEN_EXISTING)?;
        Ok(())
    }

    #[test]
    fn another_slot_keeps_navigation_until_its_own_last_grant_is_revoked() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let private = temp.path().join("private");
        let common = private.join("scratch/content");
        let owned_a = common.join("owner-a/worktree");
        let owned_b = common.join("owner-b/worktree");
        for path in [&owned_a, &owned_b] {
            std::fs::create_dir_all(path)?;
        }
        let file_a = owned_a.join("result.txt");
        let file_b = owned_b.join("result.txt");
        std::fs::write(&file_a, b"initial")?;
        std::fs::write(&file_b, b"initial")?;
        let account_a = LocalSid::from_string(SYNTHETIC_ACCOUNT_A)?;
        let account_b = LocalSid::from_string(SYNTHETIC_ACCOUNT_B)?;
        let group = LocalSid::from_string(SYNTHETIC_GROUP)?;
        let cap_a = LocalSid::from_string(SYNTHETIC_CAP_A)?;
        let cap_b = LocalSid::from_string(SYNTHETIC_CAP_B)?;
        unsafe {
            crate::acl::ensure_allow_write_aces(&owned_a, &[cap_a.as_ptr()])?;
            crate::acl::ensure_allow_write_aces(&owned_b, &[cap_b.as_ptr()])?;
        }
        let sync = |state: &str, account: &LocalSid, writes: &[PathBuf]| unsafe {
            sync_private_read_acls(
                &temp.path().join(state),
                std::slice::from_ref(&private),
                &[],
                writes,
                &[account.as_ptr()],
                group.as_ptr(),
            )
        };
        sync("state-a", &account_a, std::slice::from_ref(&owned_a))?;
        let token_a = strict_token(&[account_a.as_ptr(), group.as_ptr(), cap_a.as_ptr()])?;
        let token_b = strict_token(&[account_b.as_ptr(), group.as_ptr(), cap_b.as_ptr()])?;
        navigation_without_listing(&token_a, &[private.clone(), common.clone()])?;
        read_write_file(&token_a, &file_a)?;

        sync("state-b", &account_b, std::slice::from_ref(&owned_b))?;
        for token in [&token_a, &token_b] {
            navigation_without_listing(token, &[private.clone(), common.clone()])?;
        }
        read_write_file(&token_a, &file_a)?;
        read_write_file(&token_b, &file_b)?;
        denied(&token_a, &file_b, GENERIC_READ, OPEN_EXISTING)?;
        denied(&token_b, &file_a, GENERIC_READ, OPEN_EXISTING)?;

        sync("state-b", &account_b, &[])?;
        navigation_without_listing(&token_a, &[private.clone(), common.clone()])?;
        read_write_file(&token_a, &file_a)?;
        private_directory_denied(&token_b, &private)?;
        denied(&token_b, &file_b, GENERIC_READ, OPEN_EXISTING)?;

        sync("state-a", &account_a, &[])?;
        for token in [&token_a, &token_b] {
            private_directory_denied(token, &private)?;
            private_directory_denied(token, &common)?;
            denied(token, &file_a, GENERIC_READ, OPEN_EXISTING)?;
            denied(token, &file_b, GENERIC_READ, OPEN_EXISTING)?;
        }
        Ok(())
    }
}
