#[cfg(windows)]
#[path = "../runtime_read_access.rs"]
mod runtime_read_access;

#[cfg(not(windows))]
fn main() {
    eprintln!("dsh-sandbox-windows: Windows only");
    std::process::exit(125);
}

#[cfg(windows)]
fn main() {
    match windows_runner::run() {
        Ok(exit_code) => std::process::exit(exit_code),
        Err(error) => {
            eprintln!("dsh-sandbox-windows: {error}");
            std::process::exit(125);
        }
    }
}

#[cfg(windows)]
pub mod windows_runner {
    mod run_journal {
        include!("../run_journal.rs");
    }
    use std::ffi::{OsStr, c_void};
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::Security::{
        FreeSid, GetSidIdentifierAuthority, GetSidSubAuthority, GetSidSubAuthorityCount, PSID,
        SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Memory::{
        GetProcessHeap, HEAP_ZERO_MEMORY, HeapAlloc, HeapFree,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateMutexW, CreateProcessW,
        DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
        InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
        PROCESS_INFORMATION, ReleaseMutex, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        UpdateProcThreadAttribute, WaitForSingleObject,
    };

    const INFINITE: u32 = 0xffff_ffff;

    #[link(name = "userenv")]
    unsafe extern "system" {
        fn CreateAppContainerProfile(
            name: *const u16,
            display_name: *const u16,
            description: *const u16,
            capabilities: *const SID_AND_ATTRIBUTES,
            capability_count: u32,
            sid: *mut PSID,
        ) -> i32;
        fn DeleteAppContainerProfile(name: *const u16) -> i32;
    }

    struct Handle(HANDLE);
    impl Handle {
        fn new(value: HANDLE, operation: &str) -> Result<Self, String> {
            if value.is_null() {
                Err(last_error(operation))
            } else {
                Ok(Self(value))
            }
        }
    }
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    struct Sid(PSID);
    impl Drop for Sid {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { FreeSid(self.0) };
            }
        }
    }

    struct AppContainerProfile {
        name: Vec<u16>,
        sid: Sid,
        preserve: std::sync::Arc<std::sync::atomic::AtomicBool>,
        removed: bool,
    }
    impl AppContainerProfile {
        fn create() -> Result<Self, String> {
            let unique = format!(
                "DeepSeekHarnessSandbox.{}.{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|error| format!("clock: {error}"))?
                    .as_nanos()
            );
            let name = wide(&unique);
            let display = wide("DeepSeek Harness Sandbox");
            let description = wide("Ephemeral DeepSeek Harness sandbox profile");
            let mut sid = null_mut();
            let result = unsafe {
                CreateAppContainerProfile(
                    name.as_ptr(),
                    display.as_ptr(),
                    description.as_ptr(),
                    null(),
                    0,
                    &mut sid,
                )
            };
            if result < 0 {
                return Err(format!(
                    "CreateAppContainerProfile failed with HRESULT 0x{:08x}",
                    result as u32
                ));
            }
            Ok(Self {
                name,
                sid: Sid(sid),
                preserve: Default::default(),
                removed: false,
            })
        }
        fn remove(&mut self) -> Result<(), String> {
            if self.removed {
                return Ok(());
            }
            if self.preserve.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(
                    "sandbox temporary storage changed; profile preserved for safe recovery".into(),
                );
            }
            let result = unsafe { DeleteAppContainerProfile(self.name.as_ptr()) };
            if result < 0 {
                return Err(format!(
                    "DeleteAppContainerProfile failed: 0x{:08x}",
                    result as u32
                ));
            }
            self.removed = true;
            Ok(())
        }
    }
    impl Drop for AppContainerProfile {
        fn drop(&mut self) {
            if self.removed || self.preserve.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            unsafe {
                DeleteAppContainerProfile(self.name.as_ptr());
            }
        }
    }

    struct ManagedTempRedirect {
        link: PathBuf,
        target: PathBuf,
        preserve: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl ManagedTempRedirect {
        fn install(
            profile: &AppContainerProfile,
            roots: &[PathBuf],
        ) -> Result<Option<Self>, String> {
            let Some(target) = std::env::var_os("DSH_SCRATCH_DIR").map(PathBuf::from) else {
                return Ok(None);
            };
            let target = std::fs::canonicalize(target)
                .map_err(|e| format!("managed temporary directory: {e}"))?;
            if !roots
                .iter()
                .any(|root| std::fs::canonicalize(root).is_ok_and(|root| target.starts_with(root)))
            {
                return Err("managed temporary directory is outside the declared roots".into());
            }
            let base = std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is missing")?;
            let name = String::from_utf16_lossy(&profile.name[..profile.name.len() - 1])
                .to_ascii_lowercase();
            let link = PathBuf::from(base)
                .join("Packages")
                .join(name)
                .join("AC/Temp");
            if link.exists() {
                let meta = std::fs::symlink_metadata(&link).map_err(|e| e.to_string())?;
                use std::os::windows::fs::MetadataExt;
                if meta.file_attributes() & 0x400 != 0
                    || std::fs::read_dir(&link)
                        .map_err(|e| e.to_string())?
                        .next()
                        .is_some()
                {
                    profile
                        .preserve
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    return Err(
                        "AppContainer temporary directory already contains unowned data".into(),
                    );
                }
                std::fs::remove_dir(&link).map_err(|e| e.to_string())?;
            }
            std::fs::create_dir_all(link.parent().unwrap()).map_err(|e| e.to_string())?;
            junction::create(&target, &link).map_err(|e| {
                profile
                    .preserve
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                format!("cannot connect AppContainer temporary storage: {e}")
            })?;
            Ok(Some(Self {
                link,
                target,
                preserve: profile.preserve.clone(),
            }))
        }
    }
    impl Drop for ManagedTempRedirect {
        fn drop(&mut self) {
            if std::fs::canonicalize(&self.link).ok().as_ref() != Some(&self.target)
                || std::fs::remove_dir(&self.link).is_err()
            {
                // Never let profile deletion follow a changed or busy link.
                self.preserve
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
    }

    fn sid_string(sid: PSID) -> Result<String, String> {
        if sid.is_null() {
            return Err("AppContainer returned a null SID".to_string());
        }
        let authority = unsafe { GetSidIdentifierAuthority(sid) };
        let count = unsafe { GetSidSubAuthorityCount(sid) };
        if authority.is_null() || count.is_null() {
            return Err(last_error("read AppContainer SID"));
        }
        let value = unsafe { (*authority).Value };
        let authority_value = ((value[0] as u64) << 40)
            | ((value[1] as u64) << 32)
            | ((value[2] as u64) << 24)
            | ((value[3] as u64) << 16)
            | ((value[4] as u64) << 8)
            | value[5] as u64;
        let mut result = format!("S-1-{authority_value}");
        for index in 0..unsafe { *count } as u32 {
            let sub = unsafe { GetSidSubAuthority(sid, index) };
            if sub.is_null() {
                return Err(last_error("read AppContainer SID subauthority"));
            }
            result.push_str(&format!("-{}", unsafe { *sub }));
        }
        Ok(result)
    }

    struct AttributeList {
        heap: HANDLE,
        ptr: *mut c_void,
    }
    impl AttributeList {
        fn new() -> Result<Self, String> {
            let mut bytes = 0usize;
            unsafe {
                InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut bytes);
            }
            if bytes == 0 {
                return Err(last_error("size process attribute list"));
            }
            let heap = unsafe { GetProcessHeap() };
            let ptr = unsafe { HeapAlloc(heap, HEAP_ZERO_MEMORY, bytes) };
            if ptr.is_null() {
                return Err(last_error("HeapAlloc(process attribute list)"));
            }
            if unsafe { InitializeProcThreadAttributeList(ptr, 1, 0, &mut bytes) } == 0 {
                unsafe { HeapFree(heap, 0, ptr) };
                return Err(last_error("InitializeProcThreadAttributeList"));
            }
            Ok(Self { heap, ptr })
        }

        fn set_security_capabilities(
            &mut self,
            capabilities: &SECURITY_CAPABILITIES,
        ) -> Result<(), String> {
            let ok = unsafe {
                UpdateProcThreadAttribute(
                    self.ptr,
                    0,
                    PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                    capabilities as *const _ as *const c_void,
                    size_of::<SECURITY_CAPABILITIES>(),
                    null_mut(),
                    null(),
                )
            };
            if ok == 0 {
                Err(last_error("UpdateProcThreadAttribute"))
            } else {
                Ok(())
            }
        }
    }
    impl Drop for AttributeList {
        fn drop(&mut self) {
            unsafe {
                DeleteProcThreadAttributeList(self.ptr);
                HeapFree(self.heap, 0, self.ptr);
            }
        }
    }

    struct MutexGuard(Handle);
    impl Drop for MutexGuard {
        fn drop(&mut self) {
            unsafe { ReleaseMutex(self.0.0) };
        }
    }

    fn lock_acl_updates() -> Result<MutexGuard, String> {
        lock_named_acl_updates("Global\\DSH-Sandbox-Acl-Updates")
    }
    fn lock_named_acl_updates(name: &str) -> Result<MutexGuard, String> {
        let name = wide(name);
        let handle = Handle::new(
            unsafe { CreateMutexW(null(), 0, name.as_ptr()) },
            "CreateMutexW",
        )?;
        let result = unsafe { WaitForSingleObject(handle.0, INFINITE) };
        // WAIT_ABANDONED grants ownership too. A cancelled runner must not
        // poison every later permission preparation using this mutex.
        if result != WAIT_OBJECT_0 && result != 0x80 {
            return Err(last_error("WaitForSingleObject(ACL mutex)"));
        }
        Ok(MutexGuard(handle))
    }

    #[test]
    fn abandoned_acl_mutex_remains_acquirable() {
        let name = format!(
            "Local\\DSH-Acl-Test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let wide_name = wide(&name);
        let keep = Handle::new(
            unsafe { CreateMutexW(null(), 0, wide_name.as_ptr()) },
            "CreateMutexW",
        )
        .unwrap();
        let worker_name = name.clone();
        std::thread::spawn(move || {
            let name = wide(&worker_name);
            let handle = Handle::new(
                unsafe { CreateMutexW(null(), 0, name.as_ptr()) },
                "CreateMutexW",
            )
            .unwrap();
            assert_eq!(
                unsafe { WaitForSingleObject(handle.0, INFINITE) },
                WAIT_OBJECT_0
            );
            // Closing this handle and exiting the owning thread abandons the
            // still-existing object retained by the parent handle.
            drop(handle);
        })
        .join()
        .unwrap();
        let recovered = lock_named_acl_updates(&name).unwrap();
        drop(recovered);
        drop(keep);
    }

    struct AclGrant {
        workspace: PathBuf,
        sid: String,
        armed: bool,
    }

    // Windows PowerShell resolves every component of its startup directory.
    // Give the ephemeral SID metadata/traverse access to ancestors only, with
    // no listing, file reads, writes or inherited permission. Updating the
    // handle's security descriptor without propagation avoids walking an entire
    // drive's children, including when another process holds the directory open.
    struct AncestorAccess {
        handles: Vec<Handle>,
        sid: PSID,
    }
    impl AncestorAccess {
        fn grant(roots: &[&Path], sid: PSID) -> Result<Self, String> {
            use windows_sys::Win32::Storage::FileSystem::*;
            let mut result = Self {
                handles: Vec::new(),
                sid,
            };
            let mut seen = std::collections::BTreeSet::new();
            for root in roots {
                for parent in root.ancestors().skip(1) {
                    if parent.as_os_str().is_empty() || !seen.insert(parent.to_path_buf()) {
                        continue;
                    }
                    let name = wide(parent.as_os_str());
                    let handle = unsafe {
                        CreateFileW(
                            name.as_ptr(),
                            0x00060000, // READ_CONTROL | WRITE_DAC; no DELETE sharing needed
                            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                            null(),
                            OPEN_EXISTING,
                            FILE_FLAG_BACKUP_SEMANTICS,
                            null_mut(),
                        )
                    };
                    if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                        return Err(format!(
                            "{}: {}",
                            parent.display(),
                            last_error("open sandbox ancestor permissions")
                        ));
                    }
                    let handle = Handle::new(handle, "open sandbox ancestor")?;
                    update_ancestor_access(handle.0, sid, true)?;
                    result.handles.push(handle);
                }
            }
            Ok(result)
        }
    }
    impl Drop for AncestorAccess {
        fn drop(&mut self) {
            let _ = self.revoke();
        }
    }
    impl AncestorAccess {
        fn revoke(&mut self) -> Result<(), String> {
            let mut errors = Vec::new();
            while let Some(handle) = self.handles.pop() {
                if let Err(error) = update_ancestor_access(handle.0, self.sid, false) {
                    errors.push(error);
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("; "))
            }
        }
    }
    fn update_ancestor_access(handle: HANDLE, sid: PSID, grant: bool) -> Result<(), String> {
        update_access(handle, sid, grant, 0x001200a0, 0)
    }
    fn update_access(
        handle: HANDLE,
        sid: PSID,
        grant: bool,
        rights: u32,
        inheritance: u32,
    ) -> Result<(), String> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::*;
        use windows_sys::Win32::Security::*;
        let _lock = lock_acl_updates()?;
        unsafe {
            let mut descriptor = null_mut();
            let mut old_acl = null_mut();
            let status = GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut old_acl,
                null_mut(),
                &mut descriptor,
            );
            if status != 0 {
                return Err(format!("read ancestor permissions: Windows error {status}"));
            }
            let entry = EXPLICIT_ACCESS_W {
                grfAccessPermissions: rights,
                grfAccessMode: if grant { GRANT_ACCESS } else { REVOKE_ACCESS },
                grfInheritance: inheritance,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: null_mut(),
                    MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_UNKNOWN,
                    ptstrName: sid.cast(),
                },
            };
            let mut acl = null_mut();
            let status = SetEntriesInAclW(1, &entry, old_acl, &mut acl);
            if status != 0 {
                LocalFree(descriptor);
                return Err(format!(
                    "prepare ancestor permissions: Windows error {status}"
                ));
            }
            // SetEntriesInAcl can insert an explicit allow after an inherited
            // deny. Keep inherited order, but place all explicit ACEs first so
            // subsequent .NET ACL updates can still modify the directory.
            let mut ordered = vec![0u8; (*acl).AclSize as usize];
            let ordered_acl = ordered.as_mut_ptr().cast::<ACL>();
            let mut entries = Vec::new();
            for index in 0..(*acl).AceCount as u32 {
                let mut ace = null_mut();
                if GetAce(acl, index, &mut ace) == 0 {
                    LocalFree(acl.cast());
                    LocalFree(descriptor);
                    return Err(last_error("read ancestor ACE"));
                }
                let header = &*ace.cast::<ACE_HEADER>();
                let group = if header.AceFlags & 0x10 != 0 {
                    2
                } else if matches!(header.AceType, 1 | 6 | 10 | 12) {
                    0
                } else {
                    1
                };
                entries.push((group, ace, header.AceSize));
            }
            entries.sort_by_key(|entry| entry.0);
            let mut canonical =
                InitializeAcl(ordered_acl, ordered.len() as u32, (*acl).AclRevision as u32) != 0;
            for (_, ace, size) in entries {
                canonical &= AddAce(
                    ordered_acl,
                    (*acl).AclRevision as u32,
                    u32::MAX,
                    ace,
                    size as u32,
                ) != 0;
            }
            let status = if canonical && inheritance == 0 {
                set_object_dacl(handle, ordered_acl, descriptor)
            } else if canonical {
                SetSecurityInfo(
                    handle,
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    ordered_acl,
                    null(),
                )
            } else {
                1336
            };
            let error = if status == 0 {
                None
            } else {
                Some(format!(
                    "update ancestor permissions: Windows error {status}"
                ))
            };
            LocalFree(acl.cast());
            LocalFree(descriptor);
            error.map_or(Ok(()), Err)
        }
    }

    // SetSecurityInfo(SE_FILE_OBJECT) walks existing descendants even when the
    // new ACE itself is not inheritable. Ancestors need only a local metadata
    // grant. Use the native handle-based operation so an ACL-only handle remains
    // nonrecursive and a concurrent rename cannot redirect the update.
    unsafe fn set_object_dacl(
        handle: HANDLE,
        acl: *const windows_sys::Win32::Security::ACL,
        original: windows_sys::Win32::Security::PSECURITY_DESCRIPTOR,
    ) -> u32 {
        use windows_sys::Win32::Security::*;
        #[link(name = "ntdll")]
        unsafe extern "system" {
            fn NtSetSecurityObject(
                handle: HANDLE,
                information: u32,
                descriptor: PSECURITY_DESCRIPTOR,
            ) -> i32;
            fn RtlNtStatusToDosError(status: i32) -> u32;
        }
        unsafe {
            let mut descriptor: SECURITY_DESCRIPTOR = zeroed();
            let pointer = (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast();
            let mut control = 0;
            let mut revision = 0;
            let preserved = SE_DACL_PROTECTED | SE_DACL_AUTO_INHERITED;
            if GetSecurityDescriptorControl(original, &mut control, &mut revision) == 0
                || InitializeSecurityDescriptor(pointer, 1) == 0
                || SetSecurityDescriptorDacl(pointer, 1, acl, 0) == 0
                || SetSecurityDescriptorControl(pointer, preserved, control & preserved) == 0
            {
                return GetLastError();
            }
            let status = NtSetSecurityObject(handle, DACL_SECURITY_INFORMATION, pointer);
            if status >= 0 {
                0
            } else {
                RtlNtStatusToDosError(status)
            }
        }
    }

    #[test]
    fn ancestor_acl_updates_leave_existing_children_unchanged() {
        use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, LocalFree};
        use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
        use windows_sys::Win32::Security::*;
        use windows_sys::Win32::Storage::FileSystem::*;

        struct Snapshot(PSECURITY_DESCRIPTOR, *mut ACL);
        impl Snapshot {
            fn read(handle: HANDLE) -> Self {
                let mut descriptor = null_mut();
                let mut acl = null_mut();
                assert_eq!(
                    unsafe {
                        GetSecurityInfo(
                            handle,
                            SE_FILE_OBJECT,
                            DACL_SECURITY_INFORMATION,
                            null_mut(),
                            null_mut(),
                            &mut acl,
                            null_mut(),
                            &mut descriptor,
                        )
                    },
                    0
                );
                assert!(!acl.is_null());
                Self(descriptor, acl)
            }
            fn bytes(&self) -> Vec<u8> {
                unsafe {
                    std::slice::from_raw_parts(self.1.cast(), (*self.1).AclSize as usize).to_vec()
                }
            }
        }
        impl Drop for Snapshot {
            fn drop(&mut self) {
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
        fn open(path: &Path, access: u32, sharing: u32) -> Handle {
            let handle = unsafe {
                CreateFileW(
                    wide(path.as_os_str()).as_ptr(),
                    access,
                    sharing,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS,
                    null_mut(),
                )
            };
            assert_ne!(
                handle,
                INVALID_HANDLE_VALUE,
                "{}",
                last_error("open test directory")
            );
            Handle(handle)
        }
        let profile = AppContainerProfile::create().unwrap();
        let marker = AppContainerProfile::create().unwrap();
        let root = std::env::temp_dir().join(format!(
            "dsh-ancestor-acl-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let child = root.join("child");
        std::fs::create_dir_all(&child).unwrap();
        // Match a live process's current-directory handle: no DELETE sharing.
        let held = open(
            &root,
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        );
        let parent = open(
            &root,
            READ_CONTROL | WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        );
        let child_handle = open(
            &child,
            READ_CONTROL | WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        );
        let before = Snapshot::read(child_handle.0);
        // Give the parent an inheritable marker, then retain the child's prior
        // ACL to detect even an unrelated inheritance walk deterministically.
        update_access(
            parent.0,
            marker.sid.0,
            true,
            FILE_GENERIC_READ,
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE,
        )
        .unwrap();
        assert_ne!(Snapshot::read(child_handle.0).bytes(), before.bytes());
        assert_eq!(
            unsafe { set_object_dacl(child_handle.0, before.1, before.0) },
            0
        );
        let child_baseline = Snapshot::read(child_handle.0).bytes();
        // The fixture's inheritable grant may normalize the parent's existing
        // inheritance. Compare the operations under test with that seeded state.
        let original = Snapshot::read(parent.0);
        update_ancestor_access(parent.0, profile.sid.0, true).unwrap();
        assert_eq!(Snapshot::read(child_handle.0).bytes(), child_baseline);
        update_ancestor_access(parent.0, profile.sid.0, false).unwrap();
        assert_eq!(Snapshot::read(child_handle.0).bytes(), child_baseline);
        assert_eq!(Snapshot::read(parent.0).bytes(), original.bytes());
        update_ancestor_access(parent.0, marker.sid.0, false).unwrap();
        drop(child_handle);
        drop(parent);
        drop(held);
        let resolved = std::fs::canonicalize(&root).unwrap();
        let temporary = std::fs::canonicalize(std::env::temp_dir()).unwrap();
        assert!(resolved != temporary && resolved.starts_with(&temporary));
        std::fs::remove_dir_all(resolved).unwrap();
    }
    impl AclGrant {
        fn grant(workspace: &Path, sid: &str, writable: bool) -> Result<Self, String> {
            if let Err(error) = update_workspace_acl(workspace, sid, true, writable) {
                let _ = update_workspace_acl(workspace, sid, false, false);
                return Err(error);
            }
            Ok(Self {
                workspace: workspace.to_path_buf(),
                sid: sid.to_string(),
                armed: true,
            })
        }

        fn revoke(&mut self) -> Result<(), String> {
            if self.armed {
                update_workspace_acl(&self.workspace, &self.sid, false, false)?;
                self.armed = false;
            }
            Ok(())
        }
    }
    impl Drop for AclGrant {
        fn drop(&mut self) {
            if let Err(error) = self.revoke() {
                eprintln!("dsh-sandbox-windows: permission cleanup failed: {error}");
            }
        }
    }

    fn is_user_profile_root(workspace: &Path, profile: Option<&OsStr>) -> bool {
        let Some(profile) = profile else {
            return false;
        };
        let normalize = |value: &OsStr| {
            value
                .to_string_lossy()
                .trim_start_matches(r"\\?\")
                .trim_end_matches(['\\', '/'])
                .replace('/', "\\")
                .to_ascii_lowercase()
        };
        normalize(workspace.as_os_str()) == normalize(profile)
    }

    fn update_workspace_acl_native(
        workspace: &Path,
        sid: PSID,
        grant: bool,
        writable: bool,
    ) -> Result<(), String> {
        use windows_sys::Win32::Security::*;
        use windows_sys::Win32::Storage::FileSystem::*;
        let name = wide(workspace.as_os_str());
        // Explicit ACL access retains Win32's normal inheritance propagation.
        // MAXIMUM_ALLOWED would suppress it and leave existing files unusable.
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                READ_CONTROL | WRITE_DAC,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                null_mut(),
            )
        };
        if raw == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return Err(format!(
                "{}: {}",
                workspace.display(),
                last_error("open sandbox root ACL")
            ));
        }
        let handle = Handle::new(raw, "open sandbox root ACL")?;
        let rights = if writable {
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE
        } else {
            FILE_GENERIC_READ | FILE_GENERIC_EXECUTE
        };
        let inheritance =
            if is_user_profile_root(workspace, std::env::var_os("USERPROFILE").as_deref()) {
                0
            } else {
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            };
        update_access(handle.0, sid, grant, rights, inheritance)
    }
    fn update_workspace_acl(
        workspace: &Path,
        sid: &str,
        grant: bool,
        writable: bool,
    ) -> Result<(), String> {
        use windows_sys::Win32::Foundation::LocalFree;
        use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
        let value = wide(sid);
        let mut identity = null_mut();
        if unsafe { ConvertStringSidToSidW(value.as_ptr(), &mut identity) } == 0 {
            return Err(last_error("parse sandbox SID"));
        }
        let result = update_workspace_acl_native(workspace, identity, grant, writable);
        unsafe {
            LocalFree(identity);
        }
        result
    }

    pub fn run() -> Result<i32, String> {
        run_args(std::env::args().skip(1))
    }

    pub fn prepare_runtime_permissions(roots: &[PathBuf], cache: &Path) -> Result<(), String> {
        for root in roots {
            let _access = super::runtime_read_access::RuntimeReadAccess::acquire(
                root,
                cache,
                |root, sid| update_workspace_acl_native(root, sid, true, false),
            )?;
        }
        Ok(())
    }

    pub fn run_args(args: impl Iterator<Item = String>) -> Result<i32, String> {
        let (
            mode,
            workspace,
            temp_roots,
            read_roots,
            runtime_roots,
            runtime_cache,
            ready_event,
            cleanup_state,
            command_timeout,
            argv,
        ) = parse_args(args)?;
        if let Some(directory) = &cleanup_state {
            for warning in run_journal::recover(directory) {
                eprintln!("[sandbox-cleanup] {warning}");
            }
        }
        let ready = ready_event
            .as_ref()
            .map(|name| {
                use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW};
                Handle::new(
                    unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, wide(&name).as_ptr()) },
                    "open startup event",
                )
            })
            .transpose()?;
        let timed_out = ready_event
            .as_ref()
            .map(|name| {
                use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW};
                Handle::new(
                    unsafe {
                        OpenEventW(
                            EVENT_MODIFY_STATE,
                            0,
                            wide(format!("{name}-timeout")).as_ptr(),
                        )
                    },
                    "open command timeout event",
                )
            })
            .transpose()?;
        let workspace = std::fs::canonicalize(workspace)
            .map_err(|error| format!("resolve sandbox workspace: {error}"))?;
        if is_user_profile_root(&workspace, std::env::var_os("USERPROFILE").as_deref()) {
            return Err(
                "the sandbox cannot use the whole user profile as a workspace; select a specific project directory"
                    .to_string(),
            );
        }
        let mut profile = AppContainerProfile::create()?;
        let _temporary_redirect = if mode == "workspace-write" {
            ManagedTempRedirect::install(&profile, &temp_roots)?
        } else {
            None
        };
        let sid_text = sid_string(profile.sid.0)?;
        let mut ancestor_roots = vec![workspace.as_path()];
        ancestor_roots.extend(temp_roots.iter().map(PathBuf::as_path));
        ancestor_roots.extend(read_roots.iter().map(PathBuf::as_path));
        ancestor_roots.extend(runtime_roots.iter().map(PathBuf::as_path));
        if let Some(redirect) = _temporary_redirect.as_ref() {
            ancestor_roots.push(&redirect.link);
        }
        let journal = cleanup_state
            .as_ref()
            .map(|directory| {
                let mut roots = vec![workspace.clone()];
                roots.extend(temp_roots.iter().cloned());
                roots.extend(read_roots.iter().cloned());
                if let Some(redirect) = &_temporary_redirect {
                    if let Some(package) = redirect.link.parent().and_then(Path::parent) {
                        roots.push(package.to_path_buf());
                    }
                    roots.push(redirect.target.clone());
                }
                for root in &ancestor_roots {
                    roots.extend(root.ancestors().skip(1).map(Path::to_path_buf));
                }
                run_journal::Journal::begin(
                    directory,
                    &profile,
                    roots,
                    _temporary_redirect.as_ref(),
                )
            })
            .transpose()?;
        let mut ancestor_access = AncestorAccess::grant(&ancestor_roots, profile.sid.0)?;
        let mut workspace_grant =
            AclGrant::grant(&workspace, &sid_text, mode == "workspace-write")?;
        let mut temporary_grants = Vec::new();
        for root in temp_roots {
            if mode != "workspace-write" || !root.is_absolute() || !root.is_dir() {
                return Err("invalid managed temporary root".into());
            }
            temporary_grants.push(AclGrant::grant(&root, &sid_text, true)?);
        }
        if let Some(redirect) = _temporary_redirect.as_ref() {
            if let Some(package) = redirect.link.parent().and_then(Path::parent) {
                temporary_grants.push(AclGrant::grant(package, &sid_text, true)?);
            }
            temporary_grants.push(AclGrant::grant(&redirect.target, &sid_text, true)?);
        }
        let mut read_grants = Vec::new();
        for root in read_roots {
            if !root.is_absolute() || !root.is_dir() {
                return Err("invalid read-only source root".into());
            }
            read_grants.push(AclGrant::grant(&root, &sid_text, false)?);
        }
        let mut runtime_access = Vec::new();
        for root in &runtime_roots {
            if !root.is_absolute()
                || is_user_profile_root(root, std::env::var_os("USERPROFILE").as_deref())
            {
                return Err("invalid installed runtime root".into());
            }
            let cache = runtime_cache
                .as_ref()
                .ok_or("runtime permission state is not configured")?;
            runtime_access.push(super::runtime_read_access::RuntimeReadAccess::acquire(
                root,
                cache,
                |root, sid| update_workspace_acl_native(root, sid, true, false),
            )?);
        }
        // The policy root is an authorization boundary, not a cwd override.
        let cwd =
            std::env::current_dir().map_err(|e| format!("execution working directory: {e}"))?;
        // SandboxMode defines file effects. Package downloads need outbound
        // Internet access without granting any additional filesystem rights.
        let mut internet_sid = [0u32; 17];
        let mut sid_size = std::mem::size_of_val(&internet_sid) as u32;
        if unsafe {
            windows_sys::Win32::Security::CreateWellKnownSid(
                windows_sys::Win32::Security::WinCapabilityInternetClientSid,
                null_mut(),
                internet_sid.as_mut_ptr().cast(),
                &mut sid_size,
            )
        } == 0
        {
            return Err(last_error("create Internet client capability"));
        }
        let mut capabilities = vec![SID_AND_ATTRIBUTES {
            Sid: internet_sid.as_mut_ptr().cast(),
            Attributes: 4, // SE_GROUP_ENABLED (winnt.h)
        }];
        capabilities.extend(runtime_access.iter().map(|access| SID_AND_ATTRIBUTES {
            Sid: access.raw(),
            Attributes: 4,
        }));
        let exit = spawn_appcontainer(
            profile.sid.0,
            &capabilities,
            &cwd,
            &argv,
            ready.as_ref(),
            timed_out.as_ref(),
            command_timeout,
        )?;
        workspace_grant.revoke()?;
        for grant in temporary_grants.iter_mut().chain(read_grants.iter_mut()) {
            grant.revoke()?;
        }
        ancestor_access.revoke()?;
        drop(_temporary_redirect);
        profile.remove()?;
        if let Some(journal) = journal {
            journal.complete()?;
        }
        Ok(exit as i32)
    }

    fn parse_args(
        mut args: impl Iterator<Item = String>,
    ) -> Result<
        (
            String,
            PathBuf,
            Vec<PathBuf>,
            Vec<PathBuf>,
            Vec<PathBuf>,
            Option<PathBuf>,
            Option<String>,
            Option<PathBuf>,
            Option<u32>,
            Vec<String>,
        ),
        String,
    > {
        if args.next().as_deref() != Some("--mode") {
            return Err("expected --mode".to_string());
        }
        let mode = args.next().ok_or_else(|| "missing mode".to_string())?;
        if mode != "read-only" && mode != "workspace-write" {
            return Err(format!("unsupported mode {mode}"));
        }
        if args.next().as_deref() != Some("--workspace") {
            return Err("expected --workspace".to_string());
        }
        let workspace = PathBuf::from(args.next().ok_or_else(|| "missing workspace".to_string())?);
        let mut temp_roots = Vec::new();
        let mut read_roots = Vec::new();
        let mut runtime_roots = Vec::new();
        let mut runtime_cache = None;
        let mut ready_event = None;
        let mut cleanup_state = None;
        let mut command_timeout = None;
        let mut separator = args.next();
        while matches!(
            separator.as_deref(),
            Some(
                "--temp-root"
                    | "--read-root"
                    | "--runtime-root"
                    | "--runtime-cache"
                    | "--ready-event"
                    | "--cleanup-state"
                    | "--command-timeout-ms"
            )
        ) {
            if separator.as_deref() == Some("--command-timeout-ms") {
                let value = args
                    .next()
                    .and_then(|value| value.parse::<u32>().ok())
                    .filter(|value| *value > 0 && *value <= 2_147_483_647)
                    .ok_or("invalid command timeout")?;
                if command_timeout.replace(value).is_some() {
                    return Err("duplicate command timeout".into());
                }
                separator = args.next();
                continue;
            }
            if separator.as_deref() == Some("--cleanup-state") {
                let path = PathBuf::from(args.next().ok_or("missing cleanup state directory")?);
                if cleanup_state.is_some() || !path.is_absolute() {
                    return Err("invalid cleanup state directory".into());
                }
                cleanup_state = Some(path);
                separator = args.next();
                continue;
            }
            if separator.as_deref() == Some("--ready-event") {
                let name = args.next().ok_or("missing startup event")?;
                if ready_event.is_some()
                    || !name.starts_with("Local\\DSH-Sandbox-Ready-")
                    || name.len() > 160
                {
                    return Err("invalid startup event".into());
                }
                ready_event = Some(name);
                separator = args.next();
                continue;
            }
            if temp_roots.len() + read_roots.len() + runtime_roots.len() >= 32 {
                return Err("too many sandbox roots".into());
            }
            let path = PathBuf::from(args.next().ok_or("missing sandbox root")?);
            if separator.as_deref() == Some("--runtime-cache") {
                if runtime_cache.is_some() || !path.is_absolute() {
                    return Err("invalid runtime permission state directory".into());
                }
                runtime_cache = Some(path);
            } else if separator.as_deref() == Some("--runtime-root") {
                runtime_roots.push(path);
            } else if separator.as_deref() == Some("--temp-root") {
                temp_roots.push(path)
            } else {
                read_roots.push(path)
            }
            separator = args.next();
        }
        if separator.as_deref() != Some("--") {
            return Err("expected -- before command".to_string());
        }
        let argv: Vec<String> = args.collect();
        if argv.is_empty() {
            return Err("missing command".to_string());
        }
        Ok((
            mode,
            workspace,
            temp_roots,
            read_roots,
            runtime_roots,
            runtime_cache,
            ready_event,
            cleanup_state,
            command_timeout,
            argv,
        ))
    }

    fn spawn_appcontainer(
        sid: PSID,
        file_capabilities: &[SID_AND_ATTRIBUTES],
        cwd: &Path,
        argv: &[String],
        ready: Option<&Handle>,
        timeout_event: Option<&Handle>,
        command_timeout: Option<u32>,
    ) -> Result<u32, String> {
        let mut attributes = AttributeList::new()?;
        let capabilities = SECURITY_CAPABILITIES {
            AppContainerSid: sid,
            Capabilities: file_capabilities.as_ptr().cast_mut(),
            CapabilityCount: file_capabilities.len() as u32,
            Reserved: 0,
        };
        attributes.set_security_capabilities(&capabilities)?;
        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        startup.StartupInfo.hStdOutput = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        startup.StartupInfo.hStdError = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        startup.lpAttributeList = attributes.ptr;
        let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
        let mut command_line = wide(windows_command_line(argv));
        // CreateProcess accepts the verbatim path, but cmd.exe treats the
        // inherited `\\?\D:\...` spelling as a UNC current directory and
        // silently falls back to C:\Windows. Hand the child the ordinary
        // drive spelling after all authorization/canonical checks are done.
        let cwd_text = cwd.to_string_lossy();
        let cwd_text = if let Some(network) = cwd_text.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{network}")
        } else {
            cwd_text
                .strip_prefix(r"\\?\")
                .unwrap_or(cwd_text.as_ref())
                .to_string()
        };
        let cwd = wide(std::ffi::OsStr::new(&cwd_text));
        // A null environment lets AppContainer replace TEMP/TMP with its own
        // package directory, bypassing the Host's registered resource lease.
        // Preserve the scrubbed, per-execution environment explicitly.
        let mut variables = std::env::vars_os().collect::<Vec<_>>();
        variables.sort_by_key(|(name, _)| name.to_string_lossy().to_uppercase());
        let mut environment = Vec::<u16>::new();
        for (name, value) in variables {
            environment.extend(name.encode_wide());
            environment.push(b'=' as u16);
            environment.extend(value.encode_wide());
            environment.push(0);
        }
        environment.push(0);
        let job = create_kill_job()?;
        let ok = unsafe {
            CreateProcessW(
                null(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                environment.as_ptr().cast(),
                cwd.as_ptr(),
                &startup.StartupInfo,
                &mut process,
            )
        };
        if ok == 0 {
            return Err(last_error("CreateProcessW(AppContainer)"));
        }
        let process_handle = Handle(process.hProcess);
        let thread_handle = Handle(process.hThread);
        if unsafe { AssignProcessToJobObject(job.0, process_handle.0) } == 0 {
            let error = last_error("AssignProcessToJobObject");
            unsafe {
                windows_sys::Win32::System::Threading::TerminateProcess(process_handle.0, 125);
                WaitForSingleObject(process_handle.0, 5000);
            }
            return Err(error);
        }
        if unsafe { ResumeThread(thread_handle.0) } == u32::MAX {
            return Err(last_error("ResumeThread"));
        }
        if let Some(ready) = ready {
            if unsafe { windows_sys::Win32::System::Threading::SetEvent(ready.0) } == 0 {
                return Err(last_error("signal command startup"));
            }
        }
        // The command budget belongs to the actual child, after preparation
        // and before cleanup. Timeout stops that job while this runner remains
        // alive to revoke its grants and remove the profile.
        let wait =
            unsafe { WaitForSingleObject(process_handle.0, command_timeout.unwrap_or(INFINITE)) };
        let timed_out = wait == windows_sys::Win32::Foundation::WAIT_TIMEOUT;
        if timed_out {
            if let Some(event) = timeout_event {
                if unsafe { windows_sys::Win32::System::Threading::SetEvent(event.0) } == 0 {
                    return Err(last_error("signal command timeout"));
                }
            }
            if unsafe { windows_sys::Win32::System::JobObjects::TerminateJobObject(job.0, 124) }
                == 0
            {
                return Err(last_error("terminate timed-out command"));
            }
            if unsafe { WaitForSingleObject(process_handle.0, 5000) } != WAIT_OBJECT_0 {
                return Err("timed-out command did not settle".into());
            }
        } else if wait != WAIT_OBJECT_0 {
            return Err(last_error("WaitForSingleObject"));
        }
        let mut exit = 125;
        if unsafe { GetExitCodeProcess(process_handle.0, &mut exit) } == 0 {
            return Err(last_error("GetExitCodeProcess"));
        }
        Ok(if timed_out { 124 } else { exit })
    }

    fn create_kill_job() -> Result<Handle, String> {
        let job = Handle::new(
            unsafe { CreateJobObjectW(null(), null()) },
            "CreateJobObjectW",
        )?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            Err(last_error("SetInformationJobObject"))
        } else {
            Ok(job)
        }
    }

    fn windows_command_line(argv: &[String]) -> String {
        argv.iter()
            .map(|arg| quote_arg(arg))
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn quote_arg(arg: &str) -> String {
        if !arg.is_empty() && !arg.chars().any(|ch| ch == ' ' || ch == '\t' || ch == '"') {
            return arg.to_string();
        }
        let mut result = String::from("\"");
        let mut slashes = 0;
        for ch in arg.chars() {
            match ch {
                '\\' => slashes += 1,
                '"' => {
                    result.push_str(&"\\".repeat(slashes * 2 + 1));
                    result.push('"');
                    slashes = 0;
                }
                _ => {
                    result.push_str(&"\\".repeat(slashes));
                    slashes = 0;
                    result.push(ch);
                }
            }
        }
        result.push_str(&"\\".repeat(slashes * 2));
        result.push('"');
        result
    }

    fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
        value.as_ref().encode_wide().chain(Some(0)).collect()
    }

    fn last_error(operation: &str) -> String {
        format!("{operation} failed with Windows error {}", unsafe {
            GetLastError()
        })
    }
}
