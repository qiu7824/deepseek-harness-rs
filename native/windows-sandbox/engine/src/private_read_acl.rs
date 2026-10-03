//! Account-scoped access to product-owned private trees. The caller holds the
//! account's execution lease and the shared ACL update mutex until this returns.
use crate::private_read_plan::{PrivateAccess, navigation_ancestor_keys, private_access};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::os::windows::fs::MetadataExt;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::c_void,
    io::Write,
    path::{Path, PathBuf},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, *},
    Storage::FileSystem::*,
};

#[derive(Default, Deserialize, Serialize)]
struct State {
    roots: Vec<PathBuf>,
    #[serde(default)]
    version: u32,
    #[serde(default)]
    pending: bool,
    #[serde(default)]
    migrated: BTreeMap<String, RootStamp>,
    #[serde(default)]
    grants: Vec<PathBuf>,
    #[serde(default)]
    navigation_ancestors: Vec<PathBuf>,
}

const MIGRATION_VERSION: u32 = 3;
const NAVIGATION_ACCESS: u32 = FILE_READ_ATTRIBUTES | FILE_TRAVERSE;
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct RootStamp {
    volume: u32,
    index: u64,
    created: u64,
    boundary: String,
}

fn store_state(path: &Path, state: &State) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut pending = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    pending.write_all(&serde_json::to_vec(state)?)?;
    pending.as_file().sync_all()?;
    pending.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn boundary_for<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots
        .iter()
        .filter(|root| within(path, root))
        .max_by_key(|root| key(root).len())
}

fn minimal_roots(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort_by_key(|path| key(path).len());
    let mut roots: Vec<PathBuf> = Vec::new();
    for path in paths {
        if !roots.iter().any(|root| within(&path, root)) {
            roots.push(path);
        }
    }
    roots
}

fn key(path: &Path) -> String {
    let value = path.to_string_lossy().replace('/', "\\").to_lowercase();
    if let Some(share) = value.strip_prefix("\\\\?\\unc\\") {
        return format!("\\\\{}", share.trim_end_matches('\\'));
    }
    value
        .strip_prefix("\\\\?\\")
        .unwrap_or(&value)
        .trim_end_matches('\\')
        .to_string()
}
fn within(path: &Path, root: &Path) -> bool {
    let path = key(path);
    let root = key(root);
    path == root
        || path
            .strip_prefix(&root)
            .is_some_and(|tail| tail.starts_with('\\'))
}
fn canonical_existing(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    paths
        .iter()
        .map(|path| {
            dunce::canonicalize(path)
                .with_context(|| format!("resolve private access root {}", path.display()))
        })
        .collect()
}

fn navigation_paths(grants: &[PathBuf], private_roots: &[PathBuf]) -> BTreeMap<String, PathBuf> {
    let keys = navigation_ancestor_keys(
        &grants.iter().map(|path| key(path)).collect::<Vec<_>>(),
        &private_roots
            .iter()
            .map(|path| key(path))
            .collect::<Vec<_>>(),
    );
    grants
        .iter()
        .flat_map(|grant| grant.ancestors().skip(1))
        .filter(|ancestor| keys.contains(&key(ancestor)))
        .map(|ancestor| (key(ancestor), ancestor.to_path_buf()))
        .collect()
}
fn validate_private_root(root: &Path) -> Result<()> {
    ensure!(
        root.is_absolute()
            && root.parent().is_some()
            && !root
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir)),
        "private root must be an absolute product directory"
    );
    for name in [
        "SystemRoot",
        "WINDIR",
        "USERPROFILE",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "PROGRAMDATA",
    ] {
        if let Some(value) = std::env::var_os(name) {
            let protected = PathBuf::from(value);
            ensure!(
                key(root) != key(&protected),
                "private root cannot be an entire Windows, profile, or shared application directory"
            );
            if matches!(name, "SystemRoot" | "WINDIR") {
                ensure!(
                    !within(root, &protected),
                    "Windows system directories cannot be private execution roots"
                );
            }
        }
    }
    for ancestor in root.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) => ensure!(
                meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0,
                "PRIVATE_REPARSE_POINT: private root has an aliased ancestor {}",
                ancestor.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// Only the two dedicated accounts and the sandbox group's legacy allow ACEs
/// are changed. Host, administrator, system and unrelated account ACEs survive.
/// Old private roots remain in the slot's state so changing sessions cannot
/// silently abandon a previous read grant.
///
/// # Safety
/// SID pointers must remain valid; accounts must be the slot's dedicated users.
pub unsafe fn sync_private_read_acls(
    home: &Path,
    private_roots: &[PathBuf],
    reads: &[PathBuf],
    writes: &[PathBuf],
    account_sids: &[*mut c_void],
    group_sid: *mut c_void,
) -> Result<usize> {
    let state_path = home.join(".sandbox/private_read_acl_state.json");
    let mut state: State = match std::fs::read(&state_path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("parse private read ACL state")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => State::default(),
        Err(error) => return Err(error.into()),
    };
    for root in private_roots.iter().chain(&state.roots) {
        validate_private_root(root)?;
    }
    let current: Vec<_> = private_roots
        .iter()
        .map(|root| {
            if root.exists() {
                dunce::canonicalize(root).map_err(Into::into)
            } else {
                Ok::<_, anyhow::Error>(root.clone())
            }
        })
        .collect::<Result<_>>()?;
    state.roots.extend(current);
    state.roots.sort_by_key(|root| key(root));
    state.roots.dedup_by(|a, b| key(a) == key(b));
    let reads = canonical_existing(reads)?;
    let writes = canonical_existing(writes)?;
    for root in &state.roots {
        ensure!(
            root.is_absolute() && root.parent().is_some(),
            "private root must be a product directory"
        );
    }
    let mut current_grants: Vec<_> = reads
        .iter()
        .chain(&writes)
        .filter(|path| boundary_for(path, &state.roots).is_some_and(|root| key(path) != key(root)))
        .cloned()
        .collect();
    current_grants.sort_by_key(|path| key(path));
    current_grants.dedup_by(|a, b| key(a) == key(b));
    let root_keys = state.roots.iter().map(|path| key(path)).collect::<Vec<_>>();
    let read_keys = reads.iter().map(|path| key(path)).collect::<Vec<_>>();
    let write_keys = writes.iter().map(|path| key(path)).collect::<Vec<_>>();
    let navigation = navigation_paths(&current_grants, &state.roots);
    let navigation_keys = navigation.keys().cloned().collect::<BTreeSet<_>>();
    let mut navigation_changes = state.navigation_ancestors.clone();
    navigation_changes.extend(navigation_paths(&state.grants, &state.roots).into_values());
    navigation_changes.extend(navigation.values().cloned());
    let mut changes = state.grants.clone();
    changes.extend(current_grants.clone());
    let mut migrations = Vec::new();
    for root in &state.roots {
        if !root.exists() {
            state.migrated.remove(&key(root));
            continue;
        }
        let stamp = unsafe { root_stamp(root, account_sids, group_sid) }?;
        if state.version != MIGRATION_VERSION
            || state.pending
            || state.migrated.get(&key(root)) != Some(&stamp)
        {
            migrations.push(root.clone());
        }
    }
    changes.extend(migrations);
    // Journal every attempted grant before changing ACLs. A crash forces a
    // complete migration, so even a partially granted subtree is never lost.
    state.grants.extend(current_grants.clone());
    state.grants.sort_by_key(|path| key(path));
    state.grants.dedup_by(|a, b| key(a) == key(b));
    state
        .navigation_ancestors
        .extend(navigation.values().cloned());
    state.navigation_ancestors.sort_by_key(|path| key(path));
    state.navigation_ancestors.dedup_by(|a, b| key(a) == key(b));
    state.pending = true;
    store_state(&state_path, &state)?;
    let mut seen = BTreeSet::new();
    let mut count = 0;
    for root in minimal_roots(changes) {
        if !root.exists() {
            continue;
        }
        let mut pending = vec![root.clone()];
        while let Some(path) = pending.pop() {
            if !seen.insert(key(&path)) {
                continue;
            }
            count += 1;
            let boundary = boundary_for(&path, &state.roots)
                .context("private grant escaped its registered boundary")?;
            let access = private_access(
                &key(&path),
                &root_keys,
                &read_keys,
                &write_keys,
                &navigation_keys,
            );
            let directory = unsafe {
                reconcile_object(
                    &path,
                    &root,
                    account_sids,
                    group_sid,
                    matches!(access, PrivateAccess::Read | PrivateAccess::Write),
                    access == PrivateAccess::Write,
                    access == PrivateAccess::Navigate,
                    key(&path) == key(boundary),
                )
            }?;
            if directory {
                for entry in std::fs::read_dir(&path)
                    .with_context(|| format!("enumerate private root {}", path.display()))?
                {
                    pending.push(entry?.path());
                }
            }
        }
    }
    // Strict ancestors are point updates, not new recursive migration roots.
    // Visit revoked grants' old ancestors as well, including version-2 state.
    for path in navigation_changes {
        if !path.exists() || !seen.insert(key(&path)) {
            continue;
        }
        let boundary = boundary_for(&path, &state.roots)
            .context("private navigation escaped its registered boundary")?;
        let access = private_access(
            &key(&path),
            &root_keys,
            &read_keys,
            &write_keys,
            &navigation_keys,
        );
        unsafe {
            reconcile_object(
                &path,
                boundary,
                account_sids,
                group_sid,
                matches!(access, PrivateAccess::Read | PrivateAccess::Write),
                access == PrivateAccess::Write,
                access == PrivateAccess::Navigate,
                key(&path) == key(boundary),
            )?;
        }
        count += 1;
    }
    for root in &state.roots {
        if root.exists() {
            state.migrated.insert(key(root), unsafe {
                root_stamp(root, account_sids, group_sid)
            }?);
        }
    }
    state.grants = current_grants;
    state.navigation_ancestors = navigation.into_values().collect();
    state.version = MIGRATION_VERSION;
    state.pending = false;
    store_state(&state_path, &state)?;
    Ok(count)
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0 as HLOCAL);
            }
        }
    }
}

// A directory timestamp cannot prove that its ACL is still authoritative.
// Bind the migration to the actual filesystem object, owner, protected DACL
// flag and complete allow ACEs plus this slot/group's deny ACEs. Other slots
// only add their own deny ACEs at these containers; those unrelated reductions
// do not invalidate a completed migration for this slot.
unsafe fn root_stamp(
    path: &Path,
    accounts: &[*mut c_void],
    group: *mut c_void,
) -> Result<RootStamp> {
    let handle = Handle(unsafe {
        CreateFileW(
            crate::winutil::to_wide_file_path(path)?.as_ptr(),
            READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            0,
        )
    });
    ensure!(
        handle.0 != INVALID_HANDLE_VALUE,
        "open private migration root failed (Win32 {}) for {}",
        unsafe { GetLastError() },
        path.display()
    );
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    ensure!(
        unsafe { GetFileInformationByHandle(handle.0, &mut info) } != 0,
        "inspect private migration root failed"
    );
    ensure!(
        info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
            && info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0,
        "PRIVATE_REPARSE_POINT: migration root must be an ordinary directory"
    );
    let mut name = vec![0u16; 32768];
    let length =
        unsafe { GetFinalPathNameByHandleW(handle.0, name.as_mut_ptr(), name.len() as u32, 0) };
    ensure!(
        length > 0 && (length as usize) < name.len(),
        "resolve private migration handle failed"
    );
    ensure!(
        key(&PathBuf::from(String::from_utf16(
            &name[..length as usize]
        )?)) == key(path),
        "PRIVATE_PATH_CHANGED: migration root moved"
    );
    let mut owner = std::ptr::null_mut();
    let mut dacl = std::ptr::null_mut();
    let mut descriptor = std::ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle.0,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    let _guard = Local(descriptor);
    ensure!(
        status == ERROR_SUCCESS && !owner.is_null() && !dacl.is_null(),
        "read private migration boundary failed: {status}"
    );
    let mut control = 0;
    let mut revision = 0;
    ensure!(
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } != 0,
        "read private DACL control failed"
    );
    let mut digest = Sha256::new();
    digest.update((control & SE_DACL_PROTECTED).to_le_bytes());
    digest.update(unsafe {
        std::slice::from_raw_parts(owner.cast::<u8>(), GetLengthSid(owner) as usize)
    });
    for index in 0..unsafe { (*dacl).AceCount } {
        let mut ace = std::ptr::null_mut();
        ensure!(
            unsafe { GetAce(dacl, index as u32, &mut ace) } != 0,
            "read migration ACE failed"
        );
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if header.AceType == 1 {
            let sid = unsafe {
                (ace as *mut u8)
                    .add(std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>())
                    .cast()
            };
            if unsafe { EqualSid(sid, group) } == 0
                && !accounts
                    .iter()
                    .any(|account| unsafe { EqualSid(sid, *account) } != 0)
            {
                continue;
            }
        }
        digest.update(unsafe {
            std::slice::from_raw_parts(ace as *const u8, header.AceSize as usize)
        });
    }
    Ok(RootStamp {
        volume: info.dwVolumeSerialNumber,
        index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        created: (u64::from(info.ftCreationTime.dwHighDateTime) << 32)
            | u64::from(info.ftCreationTime.dwLowDateTime),
        boundary: format!("{:x}", digest.finalize()),
    })
}

unsafe fn has_navigation_signature(dacl: *mut ACL, sid: *mut c_void) -> Result<bool> {
    let mut self_denied = false;
    let mut children_denied = false;
    for index in 0..unsafe { (*dacl).AceCount } {
        let mut ace = std::ptr::null_mut();
        ensure!(
            unsafe { GetAce(dacl, index as u32, &mut ace) } != 0,
            "read navigation signature ACE failed"
        );
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if header.AceType != 1 {
            continue;
        }
        let denied = unsafe { &*(ace as *const ACCESS_DENIED_ACE) };
        let denied_sid = std::ptr::addr_of!(denied.SidStart).cast_mut().cast();
        if unsafe { EqualSid(denied_sid, sid) } == 0 {
            continue;
        }
        self_denied |=
            header.AceFlags == 0 && denied.Mask == (FILE_ALL_ACCESS & !NAVIGATION_ACCESS);
        children_denied |= u32::from(header.AceFlags)
            == (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE)
            && denied.Mask == FILE_ALL_ACCESS;
    }
    Ok(self_denied && children_denied)
}

unsafe fn reconcile_object(
    path: &Path,
    root: &Path,
    accounts: &[*mut c_void],
    group: *mut c_void,
    read: bool,
    write: bool,
    navigation: bool,
    boundary: bool,
) -> Result<bool> {
    let wide = crate::winutil::to_wide_file_path(path)?;
    let handle = Handle(unsafe {
        CreateFileW(
            wide.as_ptr(),
            READ_CONTROL | WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            0,
        )
    });
    ensure!(
        handle.0 != INVALID_HANDLE_VALUE,
        "open private ACL failed (Win32 {}) for {}",
        unsafe { GetLastError() },
        path.display()
    );
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    ensure!(
        unsafe { GetFileInformationByHandle(handle.0, &mut info) } != 0,
        "inspect private object failed"
    );
    // The runner itself creates an opaque cwd junction in its private home.
    // OPEN_REPARSE_POINT keeps this handle on the link object; never walk its
    // target or use that target as an authority to change external ACLs.
    let reparse = info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    let directory = info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
    let navigation = navigation && directory && !reparse;
    ensure!(
        directory || info.nNumberOfLinks == 1,
        "PRIVATE_HARD_LINK: refusing to change ACLs through a multiply-linked file"
    );
    let mut final_path = vec![0u16; 32768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            handle.0,
            final_path.as_mut_ptr(),
            final_path.len() as u32,
            0,
        )
    };
    ensure!(
        length > 0 && (length as usize) < final_path.len(),
        "resolve private object handle failed"
    );
    let final_path = PathBuf::from(String::from_utf16(&final_path[..length as usize])?);
    ensure!(
        within(&final_path, root) && key(&final_path) == key(path),
        "PRIVATE_PATH_CHANGED: private object escaped its authorized tree"
    );
    let mut dacl = std::ptr::null_mut();
    let mut descriptor = std::ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle.0,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    let _descriptor = Local(descriptor);
    ensure!(
        status == ERROR_SUCCESS && !dacl.is_null(),
        "read private DACL failed: {status}"
    );
    let mut kept = Vec::<Vec<u8>>::new();
    let mut other_navigation = false;
    for index in 0..unsafe { (*dacl).AceCount } {
        let mut ace = std::ptr::null_mut();
        ensure!(
            unsafe { GetAce(dacl, index as u32, &mut ace) } != 0,
            "read private ACE failed"
        );
        let header = unsafe { &*(ace as *const ACE_HEADER) };
        if matches!(header.AceType, 0 | 1) {
            let sid = unsafe {
                (ace as *mut u8)
                    .add(std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>())
                    .cast()
            };
            let mask = unsafe { (*(ace as *const ACCESS_ALLOWED_ACE)).Mask };
            let own_account = accounts
                .iter()
                .any(|account| unsafe { EqualSid(sid, *account) } != 0);
            // Preserve another slot only when its same-machine SID has the
            // exact navigation signature: a self-only allow, the reduced
            // self deny, and the full inherit-only deny for new children.
            other_navigation |= directory
                && !reparse
                && header.AceType == 0
                && header.AceFlags == 0
                && mask == NAVIGATION_ACCESS
                && !own_account
                && unsafe { EqualSid(sid, group) } == 0
                && accounts
                    .iter()
                    .any(|account| unsafe { EqualPrefixSid(sid, *account) } != 0)
                && unsafe { has_navigation_signature(dacl, sid) }?;
            let group_denial = boundary
                && header.AceType == 1
                && unsafe { EqualSid(sid, group) } != 0
                && (mask == (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE)
                    || mask == ((FILE_GENERIC_READ | FILE_GENERIC_EXECUTE) & !NAVIGATION_ACCESS))
                && [
                    0,
                    OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
                    OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE,
                ]
                .contains(&(u32::from(header.AceFlags) & !INHERITED_ACE));
            if accounts
                .iter()
                .any(|account| unsafe { EqualSid(sid, *account) } != 0)
                || header.AceType == 0 && unsafe { EqualSid(sid, group) } != 0
                || group_denial
            {
                continue;
            }
        }
        kept.push(
            unsafe { std::slice::from_raw_parts(ace as *const u8, header.AceSize as usize) }
                .to_vec(),
        );
    }
    let mask = if !read {
        FILE_ALL_ACCESS
            & if navigation {
                !NAVIGATION_ACCESS
            } else {
                u32::MAX
            }
    } else if write {
        FILE_GENERIC_READ | FILE_GENERIC_EXECUTE | FILE_GENERIC_WRITE | DELETE
    } else {
        FILE_GENERIC_READ | FILE_GENERIC_EXECUTE
    };
    let mut entries: Vec<_> = accounts
        .iter()
        .map(|sid| EXPLICIT_ACCESS_W {
            grfAccessPermissions: mask,
            grfAccessMode: if read { SET_ACCESS } else { DENY_ACCESS },
            grfInheritance: if navigation {
                0
            } else if directory {
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            } else {
                0
            },
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: (*sid).cast(),
            },
        })
        .collect();
    if navigation {
        for sid in accounts {
            entries.push(EXPLICIT_ACCESS_W {
                grfAccessPermissions: NAVIGATION_ACCESS,
                grfAccessMode: SET_ACCESS,
                grfInheritance: 0,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_UNKNOWN,
                    ptstrName: (*sid).cast(),
                },
            });
            entries.push(EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_ALL_ACCESS,
                grfAccessMode: DENY_ACCESS,
                grfInheritance: OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_UNKNOWN,
                    ptstrName: (*sid).cast(),
                },
            });
        }
    }
    if read {
        let denied = if write {
            WRITE_DAC | WRITE_OWNER
        } else {
            FILE_WRITE_DATA
                | FILE_APPEND_DATA
                | FILE_WRITE_EA
                | FILE_WRITE_ATTRIBUTES
                | FILE_DELETE_CHILD
                | DELETE
                | WRITE_DAC
                | WRITE_OWNER
        };
        entries.extend(accounts.iter().map(|sid| EXPLICIT_ACCESS_W {
            grfAccessPermissions: denied,
            grfAccessMode: DENY_ACCESS,
            grfInheritance: if directory {
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            } else {
                0
            },
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: (*sid).cast(),
            },
        }));
    }
    if boundary {
        let group_navigation = navigation || other_navigation;
        entries.push(EXPLICIT_ACCESS_W {
            grfAccessPermissions: (FILE_GENERIC_READ | FILE_GENERIC_EXECUTE)
                & if group_navigation {
                    !NAVIGATION_ACCESS
                } else {
                    u32::MAX
                },
            grfAccessMode: DENY_ACCESS,
            grfInheritance: if group_navigation {
                0
            } else {
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            },
            Trustee: TRUSTEE_W {
                pMultipleTrustee: std::ptr::null_mut(),
                MultipleTrusteeOperation: 0,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_UNKNOWN,
                ptstrName: group.cast(),
            },
        });
        if group_navigation {
            entries.push(EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
                grfAccessMode: DENY_ACCESS,
                grfInheritance: OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE | INHERIT_ONLY_ACE,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: std::ptr::null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_UNKNOWN,
                    ptstrName: group.cast(),
                },
            });
        }
    }
    // Keep self-only and inherit-only ACEs distinct. SetEntriesInAclW may
    // combine same-SID entries; construct a canonical DACL without that merge.
    let bytes = std::mem::size_of::<ACL>()
        + kept.iter().map(Vec::len).sum::<usize>()
        + entries
            .iter()
            .map(|entry| 8 + unsafe { GetLengthSid(entry.Trustee.ptstrName.cast()) } as usize)
            .sum::<usize>();
    ensure!(
        bytes <= u16::MAX as usize,
        "private DACL exceeds Windows ACL limit"
    );
    let mut storage = vec![0u32; bytes.div_ceil(4)];
    let replacement = storage.as_mut_ptr().cast::<ACL>();
    let revision = u32::from(unsafe { (*dacl).AclRevision });
    ensure!(
        unsafe { InitializeAcl(replacement, (storage.len() * 4) as u32, revision) } != 0,
        "initialize private DACL failed"
    );
    let add_entry = |entry: &EXPLICIT_ACCESS_W| -> Result<()> {
        let ok = unsafe {
            if entry.grfAccessMode == DENY_ACCESS {
                AddAccessDeniedAceEx(
                    replacement,
                    revision,
                    entry.grfInheritance,
                    entry.grfAccessPermissions,
                    entry.Trustee.ptstrName.cast(),
                )
            } else {
                AddAccessAllowedAceEx(
                    replacement,
                    revision,
                    entry.grfInheritance,
                    entry.grfAccessPermissions,
                    entry.Trustee.ptstrName.cast(),
                )
            }
        };
        ensure!(ok != 0, "construct private managed ACE failed");
        Ok(())
    };
    let add_kept = |ace: &Vec<u8>| -> Result<()> {
        ensure!(
            unsafe {
                AddAce(
                    replacement,
                    revision,
                    u32::MAX,
                    ace.as_ptr().cast(),
                    ace.len() as u32,
                )
            } != 0,
            "preserve host ACE failed"
        );
        Ok(())
    };
    for entry in entries
        .iter()
        .filter(|entry| entry.grfAccessMode == DENY_ACCESS)
    {
        add_entry(entry)?;
    }
    for ace in kept
        .iter()
        .filter(|ace| u32::from(ace[1]) & INHERITED_ACE == 0)
    {
        add_kept(ace)?;
    }
    for entry in entries
        .iter()
        .filter(|entry| entry.grfAccessMode != DENY_ACCESS)
    {
        add_entry(entry)?;
    }
    for ace in kept
        .iter()
        .filter(|ace| u32::from(ace[1]) & INHERITED_ACE != 0)
    {
        add_kept(ace)?;
    }
    // Root protection preserves its current host ACL while preventing a broad
    // sandbox group grant on an outside parent from being inherited again.
    let flags = DACL_SECURITY_INFORMATION
        | if boundary {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            0
        };
    let status = unsafe {
        SetSecurityInfo(
            handle.0,
            SE_FILE_OBJECT,
            flags,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            replacement,
            std::ptr::null_mut(),
        )
    };
    ensure!(
        status == ERROR_SUCCESS,
        "write private DACL {} failed: {status}",
        path.display()
    );
    Ok(directory && !reparse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::LocalSid;

    #[test]
    fn boundary_keys_match_extended_drive_and_unc_spellings() {
        assert_eq!(
            key(Path::new(r"D:\资料\Private")),
            key(Path::new(r"\\?\d:\资料\private"))
        );
        assert_eq!(
            key(Path::new(r"\\Server\Share\Private\")),
            key(Path::new(r"\\?\UNC\server\share\private"))
        );
        assert!(within(
            Path::new(r"\\?\UNC\server\share\private\child"),
            Path::new(r"\\server\share\private")
        ));
    }

    #[test]
    fn long_private_descendants_support_migration_exact_grants_and_revocation() -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("state");
        let private = temp.path().join("private");
        let directory = private
            .join(format!("session-{}", "中".repeat(110)))
            .join(format!("agent-{}", "a".repeat(110)));
        std::fs::create_dir_all(&directory)?;
        let own = directory.join("exact.txt");
        let other = directory.join("other.txt");
        std::fs::write(&own, "owned")?;
        std::fs::write(&other, "foreign")?;
        assert!(own.as_os_str().encode_wide().count() > 260);
        let account = LocalSid::from_string("S-1-5-21-654-987-321-1001")?;
        let group = LocalSid::from_string("S-1-5-21-654-987-321-1002")?;
        unsafe {
            crate::acl::ensure_allow_write_aces(&directory, &[group.as_ptr()])?;
            root_stamp(&directory, &[account.as_ptr()], group.as_ptr())?;
            sync_private_read_acls(
                &home,
                std::slice::from_ref(&private),
                std::slice::from_ref(&own),
                &[],
                &[account.as_ptr()],
                group.as_ptr(),
            )?;
        }
        assert!(
            direct_permissions(&own, account.as_ptr())
                .iter()
                .any(|(kind, mask)| *kind == 0 && mask & FILE_READ_DATA != 0)
        );
        assert!(
            direct_permissions(&other, account.as_ptr())
                .iter()
                .any(|(kind, mask)| *kind == 1 && mask & FILE_READ_DATA != 0)
        );
        assert!(
            direct_permissions(&directory, group.as_ptr())
                .iter()
                .all(|(kind, _)| *kind != 0)
        );
        unsafe {
            sync_private_read_acls(&home, &[], &[], &[], &[account.as_ptr()], group.as_ptr())?;
        }
        assert!(
            direct_permissions(&own, account.as_ptr())
                .iter()
                .any(|(kind, mask)| *kind == 1 && mask & FILE_READ_DATA != 0)
        );
        assert_eq!(std::fs::read_to_string(&own)?, "owned");
        assert_eq!(std::fs::read_to_string(&other)?, "foreign");
        Ok(())
    }

    #[test]
    fn private_root_validation_allows_product_directories_but_not_system_roots() {
        assert!(validate_private_root(Path::new(r"D:\dsh-managed-private-fixture")).is_ok());
        assert!(validate_private_root(Path::new(r"D:\")).is_err());
        if let Some(system) = std::env::var_os("SystemRoot") {
            assert!(validate_private_root(Path::new(&system)).is_err());
            assert!(validate_private_root(&PathBuf::from(system).join("System32")).is_err());
        }
    }

    fn direct_permissions(path: &Path, sid: *mut c_void) -> Vec<(u8, u32)> {
        unsafe {
            let (dacl, descriptor) = crate::acl::fetch_dacl_handle(path).unwrap();
            let _guard = Local(descriptor);
            let mut values = Vec::new();
            for index in 0..(*dacl).AceCount {
                let mut ace = std::ptr::null_mut();
                assert_ne!(GetAce(dacl, index as u32, &mut ace), 0);
                let header = &*(ace as *const ACE_HEADER);
                if !matches!(header.AceType, 0 | 1)
                    || u32::from(header.AceFlags) & INHERITED_ACE != 0
                {
                    continue;
                }
                let entry = &*(ace as *const ACCESS_ALLOWED_ACE);
                let candidate = (&entry.SidStart as *const u32).cast_mut().cast();
                if EqualSid(candidate, sid) != 0 {
                    values.push((header.AceType, entry.Mask));
                }
            }
            values
        }
    }

    #[test]
    fn exact_files_revoke_old_grants_and_preserve_other_slot_access() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("state");
        let private = temp.path().join("private");
        std::fs::create_dir(&private)?;
        let a = private.join("a.txt");
        let b = private.join("b.txt");
        std::fs::write(&a, "a")?;
        std::fs::write(&b, "b")?;
        let first = LocalSid::from_string("S-1-5-21-123-456-789-1001")?;
        let second = LocalSid::from_string("S-1-5-21-123-456-789-1002")?;
        let group = LocalSid::from_string("S-1-5-21-123-456-789-1003")?;
        unsafe {
            crate::acl::ensure_allow_write_aces(&b, &[group.as_ptr(), first.as_ptr()])?;
        }
        unsafe {
            sync_private_read_acls(
                &home,
                std::slice::from_ref(&private),
                std::slice::from_ref(&a),
                &[],
                &[first.as_ptr()],
                group.as_ptr(),
            )?;
        }
        assert!(
            direct_permissions(&a, first.as_ptr())
                .iter()
                .any(|(kind, mask)| *kind == 0 && mask & FILE_READ_DATA != 0)
        );
        assert_eq!(direct_permissions(&b, first.as_ptr())[0].0, 1);
        assert!(
            direct_permissions(&b, group.as_ptr())
                .iter()
                .all(|(kind, _)| *kind != 0)
        );
        unsafe {
            sync_private_read_acls(
                &temp.path().join("second"),
                std::slice::from_ref(&private),
                std::slice::from_ref(&b),
                &[],
                &[second.as_ptr()],
                group.as_ptr(),
            )?;
        }
        assert!(
            direct_permissions(&a, first.as_ptr())
                .iter()
                .any(|(kind, mask)| *kind == 0 && mask & FILE_READ_DATA != 0),
            "another slot keeps its exact grant"
        );
        assert_eq!(direct_permissions(&a, second.as_ptr())[0].0, 1);
        unsafe {
            sync_private_read_acls(
                &home,
                &[],
                std::slice::from_ref(&b),
                &[],
                &[first.as_ptr()],
                group.as_ptr(),
            )?;
        }
        assert_eq!(
            direct_permissions(&a, first.as_ptr())[0].0,
            1,
            "omitting a previous container does not abandon its ACL ownership"
        );
        assert!(
            direct_permissions(&b, first.as_ptr())
                .iter()
                .any(|(kind, mask)| *kind == 0 && mask & FILE_READ_DATA != 0)
        );
        assert_eq!(
            std::fs::read_to_string(&a)?,
            "a",
            "host access remains intact"
        );
        assert_eq!(std::fs::read_to_string(&b)?, "b");
        Ok(())
    }

    #[test]
    fn successful_migration_limits_warm_work_and_invalidates_changed_boundaries() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("private");
        let own = root.join("own");
        let foreign = root.join("foreign");
        let home = temp.path().join("state");
        std::fs::create_dir_all(&own)?;
        std::fs::create_dir(&foreign)?;
        std::fs::write(own.join("read.txt"), "own")?;
        for index in 0..80 {
            std::fs::write(foreign.join(format!("{index}.txt")), "foreign")?;
        }
        let account = LocalSid::from_string("S-1-5-21-22-33-44-1001")?;
        let group = LocalSid::from_string("S-1-5-21-22-33-44-1002")?;
        let sync = || unsafe {
            sync_private_read_acls(
                &home,
                std::slice::from_ref(&root),
                std::slice::from_ref(&own),
                &[],
                &[account.as_ptr()],
                group.as_ptr(),
            )
        };
        assert!(sync()? > 80);
        let new_foreign = foreign.join("created-after-migration.txt");
        std::fs::write(&new_foreign, "new")?;
        unsafe {
            let (dacl, descriptor) = crate::acl::fetch_dacl_handle(&new_foreign)?;
            let _guard = Local(descriptor);
            let mut inherited_denial = false;
            for index in 0..(*dacl).AceCount {
                let mut ace = std::ptr::null_mut();
                assert_ne!(GetAce(dacl, index as u32, &mut ace), 0);
                let header = &*(ace as *const ACE_HEADER);
                if header.AceType == 1 && u32::from(header.AceFlags) & INHERITED_ACE != 0 {
                    let entry = &*(ace as *const ACCESS_DENIED_ACE);
                    let sid = (&entry.SidStart as *const u32).cast_mut().cast();
                    inherited_denial |=
                        EqualSid(sid, account.as_ptr()) != 0 && entry.Mask & FILE_READ_DATA != 0;
                }
            }
            assert!(
                inherited_denial,
                "a new foreign file inherits the existing account denial"
            );
        }
        assert_eq!(
            sync()?,
            3,
            "warm execution visits its current grant, not eighty unrelated files"
        );
        let other = LocalSid::from_string("S-1-5-21-22-33-44-1003")?;
        unsafe {
            sync_private_read_acls(
                &temp.path().join("other-slot"),
                std::slice::from_ref(&root),
                &[],
                &[],
                &[other.as_ptr()],
                group.as_ptr(),
            )?;
        }
        assert_eq!(
            sync()?,
            3,
            "another slot's additional denials cannot cause repeated whole-tree migrations"
        );
        unsafe {
            crate::acl::ensure_allow_mask_aces(&root, &[account.as_ptr()], FILE_GENERIC_READ)?;
        }
        assert!(
            direct_permissions(&root, account.as_ptr())
                .iter()
                .any(|(kind, _)| *kind == 0)
        );
        assert!(
            sync()? > 80,
            "changed root ACL invalidates its completed migration"
        );
        let path = home.join(".sandbox/private_read_acl_state.json");
        let mut state: State = serde_json::from_slice(&std::fs::read(&path)?)?;
        state.pending = true;
        store_state(&path, &state)?;
        assert!(
            sync()? > 80,
            "an interrupted ACL pass is never treated as complete"
        );
        let before: State = serde_json::from_slice(&std::fs::read(&path)?)?;
        let stamp = before.migrated[&key(&root)].clone();
        std::fs::rename(&root, temp.path().join("old-private"))?;
        std::fs::create_dir_all(&own)?;
        std::fs::write(own.join("read.txt"), "replacement")?;
        std::fs::write(root.join("new-foreign.txt"), "foreign")?;
        assert_eq!(
            sync()?,
            4,
            "a new filesystem object at the same path is migrated completely"
        );
        let after: State = serde_json::from_slice(&std::fs::read(&path)?)?;
        assert_ne!(after.migrated[&key(&root)], stamp);
        Ok(())
    }

    #[test]
    fn old_grant_descendants_are_revoked_and_nested_boundaries_stay_private() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("private");
        let a = root.join("a");
        let b = root.join("b");
        let nested = a.join("nested-private");
        let home = temp.path().join("state");
        for path in [&a, &b, &nested] {
            std::fs::create_dir_all(path)?;
        }
        let old = a.join("explicit.txt");
        let hidden = nested.join("hidden.txt");
        std::fs::write(&old, "a")?;
        std::fs::write(&hidden, "hidden")?;
        std::fs::write(b.join("b.txt"), "b")?;
        let account = LocalSid::from_string("S-1-5-21-55-66-77-1001")?;
        let group = LocalSid::from_string("S-1-5-21-55-66-77-1002")?;
        let roots = vec![root, nested];
        unsafe {
            sync_private_read_acls(
                &home,
                &roots,
                std::slice::from_ref(&a),
                &[],
                &[account.as_ptr()],
                group.as_ptr(),
            )?;
        }
        assert!(
            direct_permissions(&hidden, account.as_ptr())
                .iter()
                .any(|(kind, mask)| *kind == 1 && mask & FILE_READ_DATA != 0)
        );
        unsafe {
            crate::acl::ensure_allow_write_aces(&old, &[account.as_ptr(), group.as_ptr()])?;
        }
        unsafe {
            sync_private_read_acls(
                &home,
                &roots,
                std::slice::from_ref(&b),
                &[],
                &[account.as_ptr()],
                group.as_ptr(),
            )?;
        }
        assert!(
            direct_permissions(&old, account.as_ptr())
                .iter()
                .any(|(kind, mask)| *kind == 1 && mask & FILE_READ_DATA != 0)
        );
        assert!(
            direct_permissions(&old, group.as_ptr())
                .iter()
                .all(|(kind, _)| *kind != 0)
        );
        Ok(())
    }

    #[test]
    fn private_hardlinks_fail_without_changing_outside_acl() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("private");
        std::fs::create_dir(&root)?;
        let outside = temp.path().join("outside.txt");
        std::fs::write(&outside, "outside")?;
        let account = LocalSid::from_string("S-1-5-21-111-222-333-1001")?;
        let group = LocalSid::from_string("S-1-5-21-111-222-333-1002")?;
        std::fs::hard_link(&outside, root.join("alias.txt"))?;
        let error = unsafe {
            sync_private_read_acls(
                &temp.path().join("state"),
                std::slice::from_ref(&root),
                &[],
                &[],
                &[account.as_ptr()],
                group.as_ptr(),
            )
        }
        .unwrap_err();
        assert!(error.to_string().contains("PRIVATE_HARD_LINK"));
        assert!(direct_permissions(&outside, account.as_ptr()).is_empty());
        assert_eq!(std::fs::read_to_string(outside)?, "outside");
        Ok(())
    }

    #[test]
    fn opaque_junctions_do_not_grant_or_rewrite_the_external_target() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("private");
        let target = temp.path().join("outside");
        std::fs::create_dir(&root)?;
        std::fs::create_dir(&target)?;
        let file = target.join("unchanged.txt");
        std::fs::write(&file, "unchanged")?;
        let alias = root.join("cwd-alias");
        std::fs::create_dir(&alias)?;
        let target = dunce::canonicalize(&target)?;
        let print: Vec<u16> = target.to_string_lossy().encode_utf16().collect();
        let substitute: Vec<u16> = format!("\\??\\{}", target.display())
            .encode_utf16()
            .collect();
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&0xa0000003_u32.to_le_bytes());
        buffer.extend_from_slice(
            &((8 + (substitute.len() + print.len() + 2) * 2) as u16).to_le_bytes(),
        );
        buffer.extend_from_slice(&0u16.to_le_bytes());
        for value in [
            0,
            (substitute.len() * 2) as u16,
            ((substitute.len() + 1) * 2) as u16,
            (print.len() * 2) as u16,
        ] {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        for value in substitute
            .iter()
            .chain(std::iter::once(&0))
            .chain(print.iter())
            .chain(std::iter::once(&0))
        {
            buffer.extend_from_slice(&value.to_le_bytes());
        }
        let handle = Handle(unsafe {
            CreateFileW(
                crate::winutil::to_wide(&alias).as_ptr(),
                GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                0,
            )
        });
        ensure!(
            handle.0 != INVALID_HANDLE_VALUE,
            "open junction fixture failed"
        );
        let mut returned = 0;
        ensure!(
            unsafe {
                windows_sys::Win32::System::IO::DeviceIoControl(
                    handle.0,
                    0x900a4,
                    buffer.as_ptr().cast(),
                    buffer.len() as u32,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                )
            } != 0,
            "create junction fixture failed: {}",
            unsafe { GetLastError() }
        );
        drop(handle);
        let account = LocalSid::from_string("S-1-5-21-333-444-555-1001")?;
        let group = LocalSid::from_string("S-1-5-21-333-444-555-1002")?;
        let home = temp.path().join("state");
        let refused = unsafe {
            sync_private_read_acls(
                &home,
                std::slice::from_ref(&alias),
                &[],
                &[],
                &[account.as_ptr()],
                group.as_ptr(),
            )
        }
        .unwrap_err();
        assert!(refused.to_string().contains("PRIVATE_REPARSE_POINT"));
        unsafe {
            sync_private_read_acls(
                &home,
                std::slice::from_ref(&root),
                &[],
                &[],
                &[account.as_ptr()],
                group.as_ptr(),
            )?;
        }
        assert!(direct_permissions(&target, account.as_ptr()).is_empty());
        assert!(direct_permissions(&file, account.as_ptr()).is_empty());
        assert_eq!(std::fs::read_to_string(&file)?, "unchanged");
        std::fs::remove_dir(&alias)?;
        Ok(())
    }

    include!("private_read_acl_access_tests.rs");
}
