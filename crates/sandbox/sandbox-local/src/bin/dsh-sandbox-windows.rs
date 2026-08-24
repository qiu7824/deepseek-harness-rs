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
    use std::ffi::{OsStr, c_void};
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;
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
        CREATE_SUSPENDED, CreateMutexW, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, ReleaseMutex,
        ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute,
        WaitForSingleObject,
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
            })
        }
    }
    impl Drop for AppContainerProfile {
        fn drop(&mut self) {
            unsafe {
                DeleteAppContainerProfile(self.name.as_ptr());
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
        let name = wide("Global\\DSH-Sandbox-Acl-Updates");
        let handle = Handle::new(
            unsafe { CreateMutexW(null(), 0, name.as_ptr()) },
            "CreateMutexW",
        )?;
        if unsafe { WaitForSingleObject(handle.0, INFINITE) } != WAIT_OBJECT_0 {
            return Err(last_error("WaitForSingleObject(ACL mutex)"));
        }
        Ok(MutexGuard(handle))
    }

    struct AclGrant {
        workspace: PathBuf,
        sid: String,
        armed: bool,
    }
    impl AclGrant {
        fn grant(workspace: &Path, sid: &str, writable: bool) -> Result<Self, String> {
            update_workspace_acl(workspace, sid, true, writable)?;
            Ok(Self {
                workspace: workspace.to_path_buf(),
                sid: sid.to_string(),
                armed: true,
            })
        }

        fn revoke(&mut self) {
            if !self.armed {
                return;
            }
            let _ = update_workspace_acl(&self.workspace, &self.sid, false, false);
            self.armed = false;
        }
    }
    impl Drop for AclGrant {
        fn drop(&mut self) {
            self.revoke();
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

    fn should_update_direct_files(workspace: &Path, profile: Option<&OsStr>) -> bool {
        !is_user_profile_root(workspace, profile)
    }

    fn update_workspace_acl(
        workspace: &Path,
        sid: &str,
        grant: bool,
        writable: bool,
    ) -> Result<(), String> {
        let _lock = lock_acl_updates()?;
        const SCRIPT: &str = r#"
$ErrorActionPreference='Stop'
$path=$env:DSH_SANDBOX_ACL_PATH
$sid=$env:DSH_SANDBOX_ACL_SID
$action=$env:DSH_SANDBOX_ACL_ACTION
$updateDirectFiles=$env:DSH_SANDBOX_ACL_DIRECT_FILES -eq '1'
$profileRoot=$env:DSH_SANDBOX_ACL_PROFILE_ROOT -eq '1'
$rights=if ($env:DSH_SANDBOX_ACL_WRITABLE -eq '1') {
  [Security.AccessControl.FileSystemRights]::Modify
} else {
  [Security.AccessControl.FileSystemRights]::ReadAndExecute
}
$identity=[Security.Principal.SecurityIdentifier]::new($sid)
$inheritance=if ($profileRoot) {
  [Security.AccessControl.InheritanceFlags]::None
} else {
  [Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit'
}
$rule=[Security.AccessControl.FileSystemAccessRule]::new(
  $identity,
  $rights,
  $inheritance,
  [Security.AccessControl.PropagationFlags]::None,
  [Security.AccessControl.AccessControlType]::Allow)
$acl=Get-Acl -LiteralPath $path
if ($action -eq 'grant') { $acl.AddAccessRule($rule) | Out-Null }
else { $acl.RemoveAccessRuleAll($rule) }
Set-Acl -LiteralPath $path -AclObject $acl
# Adding an inheritable ACE does not retroactively update existing children.
# The Node sidecar root is deliberately tiny, so update its direct files in
# the same PowerShell process without broad recursive host access.
if ($updateDirectFiles) {
  Get-ChildItem -LiteralPath $path -File | ForEach-Object {
    $childAcl=Get-Acl -LiteralPath $_.FullName
    $childRule=[Security.AccessControl.FileSystemAccessRule]::new(
      $identity, $rights, [Security.AccessControl.AccessControlType]::Allow)
    if ($action -eq 'grant') { $childAcl.AddAccessRule($childRule) | Out-Null }
    else { $childAcl.RemoveAccessRuleAll($childRule) }
    Set-Acl -LiteralPath $_.FullName -AclObject $childAcl
  }
}
"#;
        let profile = std::env::var_os("USERPROFILE");
        let profile_root = is_user_profile_root(workspace, profile.as_deref());
        let mut child = Command::new(r"C:\Program Files\PowerShell\7\pwsh.exe")
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                SCRIPT,
            ])
            .env("DSH_SANDBOX_ACL_PATH", workspace)
            .env("DSH_SANDBOX_ACL_SID", sid)
            .env(
                "DSH_SANDBOX_ACL_ACTION",
                if grant { "grant" } else { "revoke" },
            )
            .env("DSH_SANDBOX_ACL_WRITABLE", if writable { "1" } else { "0" })
            .env(
                "DSH_SANDBOX_ACL_DIRECT_FILES",
                if should_update_direct_files(workspace, profile.as_deref()) {
                    "1"
                } else {
                    "0"
                },
            )
            .env(
                "DSH_SANDBOX_ACL_PROFILE_ROOT",
                if profile_root { "1" } else { "0" },
            )
            .spawn()
            .map_err(|error| format!("launch ACL updater: {error}"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let status = loop {
            if let Some(status) = child
                .try_wait()
                .map_err(|error| format!("wait for ACL updater: {error}"))?
            {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(
                    "ACL updater timed out after 15 seconds; select a narrower project workspace"
                        .to_string(),
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        };
        if status.success() {
            Ok(())
        } else {
            Err(format!("ACL updater exited with {status}"))
        }
    }

    pub fn run() -> Result<i32, String> {
        run_args(std::env::args().skip(1))
    }

    pub fn run_args(args: impl Iterator<Item = String>) -> Result<i32, String> {
        let (mode, workspace, argv) = parse_args(args)?;
        if is_user_profile_root(&workspace, std::env::var_os("USERPROFILE").as_deref()) {
            return Err(
                "the sandbox cannot use the whole user profile as a workspace; select a specific project directory"
                    .to_string(),
            );
        }
        let profile = AppContainerProfile::create()?;
        let sid_text = sid_string(profile.sid.0)?;
        let _acl = AclGrant::grant(&workspace, &sid_text, mode == "workspace-write")?;
        let exit = spawn_appcontainer(profile.sid.0, &workspace, &argv)?;
        Ok(exit as i32)
    }

    fn parse_args(
        mut args: impl Iterator<Item = String>,
    ) -> Result<(String, PathBuf, Vec<String>), String> {
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
        if args.next().as_deref() != Some("--") {
            return Err("expected -- before command".to_string());
        }
        let argv: Vec<String> = args.collect();
        if argv.is_empty() {
            return Err("missing command".to_string());
        }
        Ok((mode, workspace, argv))
    }

    fn spawn_appcontainer(sid: PSID, cwd: &Path, argv: &[String]) -> Result<u32, String> {
        let mut attributes = AttributeList::new()?;
        let capabilities = SECURITY_CAPABILITIES {
            AppContainerSid: sid,
            Capabilities: null_mut(),
            CapabilityCount: 0,
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
        let cwd = wide(cwd.as_os_str());
        let ok = unsafe {
            CreateProcessW(
                null(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_SUSPENDED | EXTENDED_STARTUPINFO_PRESENT,
                null(),
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
        let job = create_kill_job()?;
        if unsafe { AssignProcessToJobObject(job.0, process_handle.0) } == 0 {
            return Err(last_error("AssignProcessToJobObject"));
        }
        if unsafe { ResumeThread(thread_handle.0) } == u32::MAX {
            return Err(last_error("ResumeThread"));
        }
        if unsafe { WaitForSingleObject(process_handle.0, INFINITE) } != WAIT_OBJECT_0 {
            return Err(last_error("WaitForSingleObject"));
        }
        let mut exit = 125;
        if unsafe { GetExitCodeProcess(process_handle.0, &mut exit) } == 0 {
            return Err(last_error("GetExitCodeProcess"));
        }
        Ok(exit)
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

    #[cfg(test)]
    mod tests {
        use super::should_update_direct_files;
        use std::ffi::OsString;
        use std::path::Path;

        #[test]
        fn user_profile_root_skips_direct_file_acl_updates() {
            let profile = OsString::from(r"C:\Users\Administrator");
            assert!(!should_update_direct_files(
                Path::new("C:/Users/Administrator"),
                Some(profile.as_os_str())
            ));
        }

        #[test]
        fn project_below_user_profile_keeps_direct_file_acl_updates() {
            let profile = OsString::from(r"C:\Users\Administrator");
            assert!(should_update_direct_files(
                Path::new(r"C:\Users\Administrator\project"),
                Some(profile.as_os_str())
            ));
        }
    }
}
