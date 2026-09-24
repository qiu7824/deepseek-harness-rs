//! Detached workers must not retain the SSH control channel's pipe handles.
use std::path::Path;

#[cfg(windows)]
pub(crate) fn spawn(program: &Path, root: &Path, id: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{
            CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION,
            STARTUPINFOW,
        },
    };
    fn quote(value: &str) -> String {
        let mut out = String::from("\"");
        let mut slashes = 0;
        for c in value.chars() {
            if c == '\\' {
                slashes += 1;
                continue;
            }
            out.extend(std::iter::repeat_n(
                '\\',
                if c == '"' { slashes * 2 + 1 } else { slashes },
            ));
            slashes = 0;
            out.push(c);
        }
        out.extend(std::iter::repeat_n('\\', slashes * 2));
        out.push('"');
        out
    }
    let program_text = program
        .to_str()
        .ok_or("worker program path is not Unicode")?;
    let root_text = root.to_str().ok_or("worker state path is not Unicode")?;
    let command = format!(
        "{} --worker {} {}",
        quote(program_text),
        quote(root_text),
        quote(id)
    );
    let mut command: Vec<u16> = command.encode_utf16().chain(Some(0)).collect();
    if command.len() > 32767 {
        return Err("worker command line exceeds the Windows limit".into());
    }
    let program: Vec<u16> = program.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // No inheritable handles and no STARTF_USESTDHANDLES: the persistent
    // worker cannot keep this invocation's stdout/stderr EOF open.
    let created = unsafe {
        CreateProcessW(
            program.as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        )
    };
    if created == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn spawn(program: &Path, root: &Path, id: &str) -> Result<(), String> {
    let mut command = std::process::Command::new(program);
    command
        .arg("--worker")
        .arg(root)
        .arg(id)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    command.spawn().map(|_| ()).map_err(|e| e.to_string())
}
