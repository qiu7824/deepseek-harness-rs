//! Flutter update bridge. Network work never modifies the live installation.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use updater::{Distribution, Installation, Offer};

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Prepared {
    root: PathBuf,
    offer: Offer,
    archive: PathBuf,
    sha256: String,
    staged: Option<PathBuf>,
    inventory: BTreeMap<PathBuf, String>,
}

#[derive(Serialize, Deserialize)]
struct RestartPlan {
    prepared: Prepared,
    parent_pid: u32,
    parent_created: u64,
    parent_exe: PathBuf,
}

#[derive(Serialize, Deserialize)]
struct PendingRestart {
    helper_pid: u32,
    helper_created: u64,
    helper_exe: PathBuf,
    parent_pid: u32,
    parent_created: u64,
    version: String,
}

fn pending_restart(directory: &Path) -> Option<PendingRestart> {
    let pending: PendingRestart =
        serde_json::from_value(read_json(&directory.join("pending.json")).ok()?).ok()?;
    inspect_process(pending.helper_pid)
        .ok()
        .filter(|p| {
            p.creation_time == pending.helper_created
                && same_executable(&p.executable, &pending.helper_exe)
        })
        .map(|_| pending)
}

fn restart_lock(directory: &Path) -> Result<fs::File, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("restart.lock"))
        .map_err(|e| e.to_string())?;
    file.try_lock()
        .map_err(|_| "更新正在等待客户端退出，请稍后重试".to_string())?;
    Ok(file)
}

struct PendingCleanup<'a>(&'a Path);
impl Drop for PendingCleanup<'_> {
    fn drop(&mut self) {
        let path = self.0.join("pending.json");
        if read_json(&path)
            .ok()
            .is_some_and(|v| v["helper_pid"] == std::process::id())
        {
            let _ = fs::remove_file(path);
        }
    }
}

fn read_json(path: &Path) -> Result<serde_json::Value, String> {
    let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
    if metadata.len() > 16 * 1024 * 1024 {
        return Err("更新清单超过大小限制".into());
    }
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn identity(root: &Path) -> Result<Installation, String> {
    let manifest = read_json(&root.join("DESKTOP.json"))?;
    let get = |key: &str| {
        manifest[key]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("DESKTOP.json 缺少 {key}"))
    };
    let version = get("version")?;
    let platform = get("platform")?;
    let arch = get("arch")?;
    let native_platform = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let native_arch = std::env::consts::ARCH;
    if manifest["schemaVersion"] != 1 || platform != native_platform || arch != native_arch {
        return Err("桌面安装清单与当前平台或架构不匹配".into());
    }
    let host_root = PathBuf::from(get("hostRoot")?);
    if !updater::safe_relative(&host_root) {
        return Err("桌面 Host 目录无效".into());
    }
    let host = fs::canonicalize(root.join(&host_root)).map_err(|e| e.to_string())?;
    let canonical = fs::canonicalize(root).map_err(|e| e.to_string())?;
    if !host.starts_with(&canonical) {
        return Err("桌面 Host 目录包含外部链接".into());
    }
    let core = read_json(&host.join("PACKAGE.json"))?;
    if core["version"] != version
        || core["platform"] != platform
        || core["arch"] != arch
        || core["variant"] != "core"
    {
        return Err("桌面客户端与随附 Host 版本不一致，请使用完整安装包修复".into());
    }
    if version != PRODUCT_VERSION {
        return Err("桌面客户端与更新器版本不一致，请使用完整安装包修复".into());
    }
    Ok(Installation {
        version,
        platform,
        arch,
        variant: "flutter".into(),
        distribution: if cfg!(windows) && root.join("unins000.exe").is_file() {
            Distribution::Installer
        } else {
            Distribution::Portable
        },
    })
}

fn cache(root: &Path) -> PathBuf {
    let key = format!("{:x}", Sha256::digest(root.to_string_lossy().as_bytes()));
    active_home(root)
        .join("launcher/desktop-updates")
        .join(&key[..24])
}

fn locked(cache: &Path) -> Result<fs::File, String> {
    fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cache.join("update.lock"))
        .map_err(|e| e.to_string())?;
    file.try_lock()
        .map_err(|_| "另一个窗口正在处理更新，请稍后重试".to_string())?;
    Ok(file)
}

fn persist<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if path.exists() {
        fs::remove_file(path).map_err(|e| e.to_string())?;
    }
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

fn inventory(root: &Path) -> Result<BTreeMap<PathBuf, String>, String> {
    updater::files(root)?
        .into_iter()
        .map(|path| {
            let portable_path = path.to_string_lossy().replace('\\', "/");
            if !updater::safe_relative(Path::new(&portable_path)) {
                return Err("更新清单包含不安全路径".into());
            }
            updater::digest(&root.join(&path)).map(|hash| (path, hash))
        })
        .collect()
}

fn verify_stage(staged: &Path, offer: &Offer) -> Result<BTreeMap<PathBuf, String>, String> {
    let manifest = read_json(&staged.join("DESKTOP.json"))?;
    let version = parse_version(&offer.release.tag_name).ok_or("发布版本无效")?;
    if manifest["schemaVersion"] != 1
        || manifest["version"].as_str().and_then(parse_version) != Some(version.clone())
        || manifest["platform"] != offer.installation.platform
        || manifest["arch"] != offer.installation.arch
        || manifest["entry"] != "dsh_desktop.exe"
        || manifest["hostRoot"] != "host"
    {
        return Err("桌面更新包产品、平台、架构或版本不匹配".into());
    }
    let core = read_json(&staged.join("host/PACKAGE.json"))?;
    if core["version"].as_str().and_then(parse_version) != Some(version)
        || core["platform"] != offer.installation.platform
        || core["arch"] != offer.installation.arch
        || core["variant"] != "core"
    {
        return Err("桌面更新包与随附 Host 不匹配".into());
    }
    for path in [
        "dsh_desktop.exe",
        "flutter_windows.dll",
        "data/app.so",
        "data/icudtl.dat",
        "host/deepseek-harness-rs.exe",
        "host/dsh-launcher.exe",
    ] {
        if !staged.join(path).is_file() {
            return Err(format!("桌面更新包缺少 {path}"));
        }
    }
    let actual = inventory(staged)?;
    let declared = manifest["files"]
        .as_object()
        .ok_or("桌面更新包文件清单缺失")?;
    if actual.len() != declared.len() + 1 {
        return Err("桌面更新包文件清单与内容不一致".into());
    }
    for (path, hash) in &actual {
        if path == Path::new("DESKTOP.json") {
            continue;
        }
        let key = path.to_string_lossy().replace('\\', "/");
        let row = declared.get(&key).ok_or("桌面更新包包含清单外文件")?;
        if row["sha256"] != *hash
            || row["bytes"].as_u64()
                != Some(
                    fs::metadata(staged.join(path))
                        .map_err(|e| e.to_string())?
                        .len(),
                )
        {
            return Err(format!("桌面更新文件校验失败：{key}"));
        }
    }
    Ok(actual)
}

fn load_prepared(root: &Path, directory: &Path) -> Result<Prepared, String> {
    let prepared: Prepared = serde_json::from_value(read_json(&directory.join("prepared.json"))?)
        .map_err(|e| e.to_string())?;
    if prepared.root != root
        || !prepared.archive.starts_with(directory)
        || prepared.offer.installation.variant != "flutter"
        || updater::digest(&prepared.archive)? != prepared.sha256
    {
        return Err("已下载更新的身份或校验已失效，请重新检查和下载".into());
    }
    if let Some(staged) = &prepared.staged {
        if !staged.starts_with(directory)
            || verify_stage(staged, &prepared.offer)? != prepared.inventory
        {
            return Err("已准备更新的文件已改变，请重新下载".into());
        }
    }
    Ok(prepared)
}

fn offer_state(
    current: &Installation,
    offer: Option<&Offer>,
    prepared: Option<&Prepared>,
) -> serde_json::Value {
    let matching = prepared.filter(|p| offer.is_some_and(|o| o.asset.name == p.offer.asset.name));
    serde_json::json!({"phase":if matching.is_some() {"ready"} else if offer.is_some() {"available"} else {"current"},
        "currentVersion":current.version, "channel":if parse_version(&current.version).is_some_and(|v| v.pre.is_empty()) {"stable"} else {"preview"},
        "version":offer.map(|o| o.release.tag_name.as_str()), "asset":offer.map(|o| o.asset.name.as_str()),
        "bytes":offer.map(|o| o.asset.size), "distribution":current.distribution,
        "restartSupported":cfg!(windows), "releaseUrl":UPDATE_RELEASES_URL})
}

fn prepare(root: &Path, directory: &Path, mirror: bool) -> Result<serde_json::Value, String> {
    let current = identity(root)?;
    let offer =
        updater::check_installation(current.clone())?.ok_or("当前已经是此渠道的最新版本")?;
    let archive = updater::download(&offer, directory, mirror, |status| {
        println!(
            "{}",
            serde_json::json!({"type":"progress","message":status})
        );
    })?;
    let sha256 = updater::digest(&archive)?;
    let mut staged = None;
    let mut contents = BTreeMap::new();
    if cfg!(windows) && offer.installation.distribution == Distribution::Portable {
        let unpack = directory.join(format!(
            "stage-{}-{}",
            std::process::id(),
            now_unix_millis()
        ));
        let result: Result<(), String> = (|| {
            updater::extract(&archive, &unpack)?;
            let entries = fs::read_dir(&unpack)
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            if entries.len() != 1 || !entries[0].file_type().map_err(|e| e.to_string())?.is_dir() {
                return Err("桌面便携包根目录无效".into());
            }
            let target = entries[0].path();
            contents = verify_stage(&target, &offer)?;
            staged = Some(target);
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&unpack);
        }
        result?;
    }
    let prepared = Prepared {
        root: root.to_path_buf(),
        offer: offer.clone(),
        archive,
        sha256,
        staged,
        inventory: contents,
    };
    persist(&directory.join("prepared.json"), &prepared)?;
    Ok(offer_state(&current, Some(&offer), Some(&prepared)))
}

/// A live Host owns more than session.running: it may own PTYs, commands,
/// remote jobs and plugin children. Without an atomic Host quiescence contract
/// updates require an explicitly stopped runtime, and never terminate it.
#[cfg(windows)]
fn ensure_runtime_stopped(root: &Path, desktop_pid: u32) -> Result<(), String> {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error().to_string());
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = Process32FirstW(snapshot, &mut entry);
        let mut occupied = None;
        while found != 0 {
            let pid = entry.th32ProcessID;
            if pid != std::process::id() && pid != desktop_pid {
                match inspect_process(pid) {
                    Ok(process) if executable_within(&process.executable, root) => {
                        occupied = Some(process);
                        break;
                    }
                    Err(error) => {
                        let name = String::from_utf16_lossy(&entry.szExeFile)
                            .trim_end_matches('\0')
                            .to_ascii_lowercase();
                        if matches!(
                            name.as_str(),
                            "deepseek-harness-rs.exe"
                                | "dsh-launcher.exe"
                                | "dsh_desktop.exe"
                                | "node.exe"
                        ) {
                            CloseHandle(snapshot);
                            return Err(format!(
                                "无法确认运行时进程 {name}（PID {pid}）的安装位置：{error}。请先停止该进程再重试；已下载更新会保留。"
                            ));
                        }
                    }
                    _ => {}
                }
            }
            found = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        if let Some(process) = occupied {
            return Err(format!(
                "本地运行时仍在运行（{}，PID {}）。请先结束会话、命令、终端和后台任务并停止本地 Host，然后重试；已下载更新会保留。",
                process
                    .executable
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                process.pid
            ));
        }
    }
    Ok(())
}

fn executable_within(executable: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let executable = executable
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_ascii_lowercase();
        let root = root
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .trim_end_matches('\\')
            .to_ascii_lowercase();
        executable.starts_with(&format!("{root}\\"))
    }
    #[cfg(not(windows))]
    {
        executable.starts_with(root)
    }
}

#[cfg(not(windows))]
fn ensure_runtime_stopped(_root: &Path, _desktop_pid: u32) -> Result<(), String> {
    Err("当前平台请使用已下载的完整安装包更新".into())
}

fn arm(root: &Path, directory: &Path, parent_pid: u32) -> Result<serde_json::Value, String> {
    if !cfg!(windows) {
        return Err("当前平台请使用已下载的完整安装包更新".into());
    }
    let parent = inspect_process(parent_pid).map_err(|e| e.to_string())?;
    if !same_executable(&parent.executable, &root.join("dsh_desktop.exe")) {
        return Err("更新请求不来自当前桌面客户端".into());
    }
    if let Some(pending) = pending_restart(directory) {
        if pending.parent_pid == parent.pid && pending.parent_created == parent.creation_time {
            return Ok(serde_json::json!({"phase":"restarting","version":pending.version}));
        }
        return Err("另一个客户端正在等待重启安装，请稍后重试".into());
    }
    // The lifecycle lock complements the PID identity during helper startup
    // and shutdown, where a process alone is not an installation transaction.
    let gate = restart_lock(directory)?;
    let prepared = load_prepared(root, directory)?;
    let current = identity(root)?;
    if current.version != prepared.offer.installation.version
        || current.distribution != prepared.offer.installation.distribution
    {
        return Err("安装类型或版本已改变，请重新检查更新".into());
    }
    ensure_runtime_stopped(root, parent_pid)?;
    let plan = RestartPlan {
        prepared,
        parent_pid,
        parent_created: parent.creation_time,
        parent_exe: parent.executable,
    };
    let path = directory.join("restart.json");
    persist(&path, &plan)?;
    let helper = directory.join(format!("update-helper-{}.exe", std::process::id()));
    fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, &helper)
        .map_err(|e| e.to_string())?;
    let helper_pid = spawn_helper(&helper, &path)?;
    let helper_process =
        wait_for_process_identity(helper_pid, &helper).map_err(|e| e.to_string())?;
    persist(
        &directory.join("pending.json"),
        &PendingRestart {
            helper_pid: helper_process.pid,
            helper_created: helper_process.creation_time,
            helper_exe: helper_process.executable,
            parent_pid: parent.pid,
            parent_created: parent.creation_time,
            version: plan.prepared.offer.release.tag_name.clone(),
        },
    )?;
    drop(gate);
    Ok(serde_json::json!({"phase":"restarting","version":plan.prepared.offer.release.tag_name}))
}

#[cfg(windows)]
fn spawn_helper(helper: &Path, plan: &Path) -> Result<u32, String> {
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW,
    };
    // No inheritable handles: otherwise the detached helper can keep the
    // caller's JSON pipe open and prevent the desktop client from closing.
    let application = wide_null(helper.as_os_str());
    let mut command = wide_null(OsStr::new(&format!(
        "\"{}\" --apply-desktop-update \"{}\"",
        helper.display(),
        plan.display()
    )));
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            CREATE_NO_WINDOW,
            ptr::null(),
            ptr::null(),
            &startup,
            &mut process,
        )
    } == 0
    {
        return Err(io::Error::last_os_error().to_string());
    }
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(process.dwProcessId)
}

#[cfg(not(windows))]
fn spawn_helper(_helper: &Path, _plan: &Path) -> Result<u32, String> {
    Err("当前平台请使用已下载的完整安装包更新".into())
}

fn command(arguments: &[std::ffi::OsString]) -> Result<serde_json::Value, String> {
    let operation = arguments
        .first()
        .and_then(|s| s.to_str())
        .ok_or("缺少更新操作")?;
    let root = fs::canonicalize(Path::new(arguments.get(1).ok_or("缺少桌面安装目录")?))
        .map_err(|e| e.to_string())?;
    let directory = cache(&root);
    let _lock = locked(&directory)?;
    match operation {
        "status" => {
            let current = identity(&root)?;
            let prepared = load_prepared(&root, &directory)
                .ok()
                .filter(|p| is_newer_version(&p.offer.release.tag_name, &current.version));
            let mut state = offer_state(
                &current,
                prepared.as_ref().map(|p| &p.offer),
                prepared.as_ref(),
            );
            if prepared.is_none() {
                state["phase"] = "idle".into();
            }
            state["lastResult"] =
                read_json(&directory.join("result.json")).unwrap_or(serde_json::Value::Null);
            Ok(state)
        }
        "check" => {
            let current = identity(&root)?;
            let offer = updater::check_installation(current.clone())?;
            let prepared = load_prepared(&root, &directory).ok();
            let mut state = offer_state(&current, offer.as_ref(), prepared.as_ref());
            state["lastResult"] =
                read_json(&directory.join("result.json")).unwrap_or(serde_json::Value::Null);
            Ok(state)
        }
        "prepare" if pending_restart(&directory).is_some() => {
            Err("更新正在等待重启，不能重复下载或更换更新包".into())
        }
        "prepare" => prepare(
            &root,
            &directory,
            arguments.get(2).is_some_and(|v| v == "mirror"),
        ),
        "restart" => arm(
            &root,
            &directory,
            arguments
                .get(2)
                .and_then(|v| v.to_str())
                .and_then(|v| v.parse().ok())
                .ok_or("缺少桌面进程标识")?,
        ),
        "package" => {
            let prepared = load_prepared(&root, &directory)?;
            Ok(serde_json::json!({"path":prepared.archive}))
        }
        _ => Err("未知更新操作".into()),
    }
}

pub fn cli(arguments: &[std::ffi::OsString]) {
    let result = match command(arguments) {
        Ok(state) => serde_json::json!({"type":"result","ok":true,"state":state}),
        Err(error) => serde_json::json!({"type":"result","ok":false,"error":error}),
    };
    println!("{result}");
}

pub fn apply(path: &Path) -> Result<(), String> {
    let plan: RestartPlan = serde_json::from_value(read_json(path)?).map_err(|e| e.to_string())?;
    let directory = path.parent().ok_or("更新计划目录无效")?;
    let root = fs::canonicalize(&plan.prepared.root).map_err(|e| e.to_string())?;
    if cache(&root) != directory
        || !same_executable(&plan.parent_exe, &root.join("dsh_desktop.exe"))
    {
        return Err("更新计划与桌面安装不匹配".into());
    }
    // Arm owns this lock briefly; wait for its pending-PID record to commit.
    let locking_deadline = std::time::Instant::now() + Duration::from_secs(5);
    let _restart_lock = loop {
        match restart_lock(directory) {
            Ok(lock) => break lock,
            Err(error) if std::time::Instant::now() >= locking_deadline => return Err(error),
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    };
    let _pending_cleanup = PendingCleanup(directory);
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while inspect_process(plan.parent_pid).is_ok_and(|p| {
        p.creation_time == plan.parent_created && same_executable(&p.executable, &plan.parent_exe)
    }) {
        if std::time::Instant::now() >= deadline {
            let error = "桌面客户端未退出，更新取消，已下载更新会保留";
            persist(
                &directory.join("result.json"),
                &serde_json::json!({"ok":false,"error":error,"version":plan.prepared.offer.release.tag_name}),
            )?;
            return Err(error.into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _lock = locked(directory)?;
    let result = (|| {
        let prepared = load_prepared(&root, directory)?;
        if prepared.sha256 != plan.prepared.sha256 {
            return Err("重启期间更新计划已改变".into());
        }
        ensure_runtime_stopped(&root, 0)?;
        if prepared.offer.installation.distribution == Distribution::Installer {
            let status = Command::new(&prepared.archive)
                .arg(format!("/DIR={}", root.display()))
                .arg("/NORESTART")
                .current_dir(&root)
                .status()
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err(format!(
                    "安装程序未完成（退出码 {:?}），已下载更新会保留",
                    status.code()
                ));
            }
        } else {
            let staged = prepared.staged.ok_or("桌面更新尚未准备完成")?;
            let apply = updater::ApplyPlan {
                root: root.clone(),
                staged,
                backup: directory.join(format!("rollback-{}", now_unix_millis())),
                parent_pid: 0,
                parent_created: 0,
                parent_exe: PathBuf::new(),
                archive: prepared.archive,
                sha256: prepared.sha256,
                version: prepared.offer.release.tag_name,
                variant: "flutter".into(),
            };
            updater::install_files(&apply)?;
        }
        Ok(())
    })();
    persist(
        &directory.join("result.json"),
        &serde_json::json!({"ok":result.is_ok(),"error":result.as_ref().err(),"version":plan.prepared.offer.release.tag_name,"time":now_unix_millis()}),
    )?;
    if result.is_ok() {
        let _ = fs::remove_file(directory.join("prepared.json"));
    }
    let mut command = Command::new(root.join("dsh_desktop.exe"));
    command.current_dir(&root);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command.spawn().map_err(|e| e.to_string())?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "dsh-desktop-update-{}-{}",
                std::process::id(),
                now_unix_millis()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            if self.0.starts_with(std::env::temp_dir()) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    fn offer() -> Offer {
        Offer {
            release: updater::Release {
                tag_name: "v0.1.3-alpha.100".into(),
                draft: false,
                prerelease: true,
                assets: vec![],
            },
            asset: updater::Asset {
                name: "deepseek-harness-rs-v0.1.3-alpha.100-windows-x86_64-flutter-portable.zip"
                    .into(),
                browser_download_url: String::new(),
                size: 1,
            },
            checksums: updater::Asset {
                name: "SHA256SUMS.txt".into(),
                browser_download_url: String::new(),
                size: 1,
            },
            installation: Installation {
                version: PRODUCT_VERSION.into(),
                variant: "flutter".into(),
                platform: "windows".into(),
                arch: "x86_64".into(),
                distribution: Distribution::Portable,
            },
        }
    }
    fn stage(path: &Path) {
        fs::create_dir_all(path.join("data")).unwrap();
        fs::create_dir_all(path.join("host")).unwrap();
        for file in [
            "dsh_desktop.exe",
            "flutter_windows.dll",
            "data/app.so",
            "data/icudtl.dat",
            "host/deepseek-harness-rs.exe",
            "host/dsh-launcher.exe",
        ] {
            fs::write(path.join(file), b"verified fixture").unwrap();
        }
        let core = serde_json::json!({"version":"0.1.3-alpha.100","variant":"core","platform":"windows","arch":"x86_64"});
        persist(&path.join("host/PACKAGE.json"), &core).unwrap();
        let files: serde_json::Map<_, _> = inventory(path)
            .unwrap()
            .into_iter()
            .map(|(file, hash)| {
                let bytes = fs::metadata(path.join(&file)).unwrap().len();
                (
                    file.to_string_lossy().replace('\\', "/"),
                    serde_json::json!({"sha256":hash,"bytes":bytes}),
                )
            })
            .collect();
        persist(&path.join("DESKTOP.json"), &serde_json::json!({"schemaVersion":1,"version":"0.1.3-alpha.100","platform":"windows","arch":"x86_64","entry":"dsh_desktop.exe","hostRoot":"host","files":files})).unwrap();
    }
    #[test]
    fn stage_manifest_rejects_tampering_and_extra_files() {
        let fixture = Fixture::new();
        stage(&fixture.0);
        assert!(verify_stage(&fixture.0, &offer()).is_ok());
        fs::write(fixture.0.join("data/app.so"), b"tampered").unwrap();
        assert!(verify_stage(&fixture.0, &offer()).is_err());
        fs::write(fixture.0.join("data/app.so"), b"verified fixture").unwrap();
        fs::write(fixture.0.join("unexpected.exe"), b"extra").unwrap();
        assert!(verify_stage(&fixture.0, &offer()).is_err());
    }
    #[test]
    fn installation_rejects_mismatched_core_and_foreign_platform() {
        let fixture = Fixture::new();
        stage(&fixture.0);
        let mut desktop = read_json(&fixture.0.join("DESKTOP.json")).unwrap();
        desktop["version"] = PRODUCT_VERSION.into();
        desktop["platform"] = (if cfg!(windows) { "windows" } else { "linux" }).into();
        desktop["arch"] = std::env::consts::ARCH.into();
        persist(&fixture.0.join("DESKTOP.json"), &desktop).unwrap();
        assert!(identity(&fixture.0).unwrap_err().contains("版本不一致"));
        let mut core = read_json(&fixture.0.join("host/PACKAGE.json")).unwrap();
        core["version"] = PRODUCT_VERSION.into();
        core["platform"] = desktop["platform"].clone();
        core["arch"] = desktop["arch"].clone();
        persist(&fixture.0.join("host/PACKAGE.json"), &core).unwrap();
        assert!(identity(&fixture.0).is_ok());
        desktop["platform"] = "other-platform".into();
        persist(&fixture.0.join("DESKTOP.json"), &desktop).unwrap();
        assert!(identity(&fixture.0).is_err());
    }
    #[test]
    fn cache_lock_prevents_cross_process_duplicate_work() {
        let fixture = Fixture::new();
        let first = locked(&fixture.0).unwrap();
        assert!(locked(&fixture.0).is_err());
        drop(first);
        assert!(locked(&fixture.0).is_ok());
    }
    #[test]
    fn failed_install_keeps_desktop_and_host_payload_and_user_drafts() {
        let fixture = Fixture::new();
        let root = fixture.0.join("current");
        let staged = fixture.0.join("staged");
        stage(&staged);
        fs::create_dir_all(root.join("host")).unwrap();
        fs::write(root.join("dsh_desktop.exe"), b"old desktop").unwrap();
        fs::write(root.join("host/deepseek-harness-rs.exe"), b"old host").unwrap();
        fs::write(root.join("drafts.json"), b"user draft").unwrap();
        fs::create_dir(root.join("flutter_windows.dll")).unwrap();
        let plan = updater::ApplyPlan {
            root: root.clone(),
            staged,
            backup: fixture.0.join("rollback"),
            parent_pid: 0,
            parent_created: 0,
            parent_exe: PathBuf::new(),
            archive: PathBuf::new(),
            sha256: String::new(),
            version: String::new(),
            variant: "flutter".into(),
        };
        assert!(updater::install_files(&plan).is_err());
        assert_eq!(
            fs::read(root.join("dsh_desktop.exe")).unwrap(),
            b"old desktop"
        );
        assert_eq!(
            fs::read(root.join("host/deepseek-harness-rs.exe")).unwrap(),
            b"old host"
        );
        assert_eq!(fs::read(root.join("drafts.json")).unwrap(), b"user draft");
        assert!(!root.join("DESKTOP.json").exists());
    }
    #[cfg(windows)]
    #[test]
    fn process_root_matching_handles_verbatim_and_case_without_neighbor_prefixes() {
        assert!(executable_within(
            Path::new(r"e:\APP\host\deepseek-harness-rs.exe"),
            Path::new(r"\\?\E:\app")
        ));
        assert!(!executable_within(
            Path::new(r"E:\app-other\deepseek-harness-rs.exe"),
            Path::new(r"E:\app")
        ));
    }
    #[cfg(windows)]
    #[test]
    fn exited_process_with_an_open_child_handle_is_not_running() {
        let command =
            PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/cmd.exe");
        let mut child = Command::new(command)
            .args(["/D", "/C", "exit", "0"])
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        child.wait().unwrap();
        assert!(inspect_process(child.id()).is_err());
    }
}
