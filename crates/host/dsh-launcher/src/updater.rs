//! Release selection, bounded verified downloads and installation handoff.
use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

const MAX_DOWNLOAD: u64 = 2 * 1024 * 1024 * 1024;
#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum Distribution {
    Installer,
    Portable,
}
#[derive(Clone, Debug)]
pub struct Installation {
    pub version: String,
    pub variant: String,
    pub platform: String,
    pub arch: String,
    pub distribution: Distribution,
}
#[derive(Clone, Debug)]
pub struct Offer {
    pub release: Release,
    pub asset: Asset,
    pub checksums: Asset,
    pub installation: Installation,
}
#[derive(Deserialize)]
struct Mirror {
    name: String,
    share_url: String,
    #[serde(default)]
    code: String,
}
#[derive(Deserialize)]
struct Mirrors {
    version: String,
    files: Vec<Mirror>,
}

fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(seconds)))
            .build(),
    )
}
pub fn installation(root: &Path) -> Result<Installation, String> {
    let package: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("PACKAGE.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let get = |key: &str| {
        package[key]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| format!("PACKAGE.json 缺少 {key}"))
    };
    let variant = get("variant")?;
    if !["core", "skin", "free"].contains(&variant.as_str()) {
        return Err("未知安装类型，未切换到其他版本".into());
    }
    let version = get("version")?;
    if version != PRODUCT_VERSION {
        return Err(format!(
            "本地启动器与安装清单版本不一致：{PRODUCT_VERSION} / {version}，请使用完整安装包修复"
        ));
    }
    let platform = get("platform")?;
    let arch = get("arch")?;
    let native = if cfg!(windows) {
        root.join("unins000.exe").is_file()
    } else {
        root.starts_with("/opt/deepseek-harness-rs")
            || root.starts_with("/usr/local/lib/deepseek-harness-rs")
    };
    Ok(Installation {
        version,
        variant,
        platform,
        arch,
        distribution: if native {
            Distribution::Installer
        } else {
            Distribution::Portable
        },
    })
}
pub fn select(releases: Vec<Release>, current: Installation) -> Result<Option<Offer>, String> {
    let Some(release) = releases
        .into_iter()
        .filter(|r| !r.draft && is_newer_version(&r.tag_name, &current.version))
        .max_by_key(|r| parse_version(&r.tag_name))
    else {
        return Ok(None);
    };
    let suffix = match (&current.distribution, current.platform.as_str()) {
        (Distribution::Installer, "windows") => "-setup.exe",
        (Distribution::Installer, "linux") => ".deb",
        (Distribution::Installer, "macos") => ".pkg",
        (Distribution::Portable, "windows") => "-portable.zip",
        (Distribution::Portable, "linux" | "macos") => "-portable.tar.gz",
        _ => return Err("当前平台没有受支持的更新包".into()),
    };
    let version = parse_version(&release.tag_name).ok_or("发布版本无效")?;
    let name = format!(
        "deepseek-harness-rs-v{version}-{}-{}-{}{suffix}",
        current.platform, current.arch, current.variant
    );
    let find = |name: &str| -> Result<Asset, String> {
        let matches: Vec<_> = release
            .assets
            .iter()
            .filter(|a| a.name == name)
            .cloned()
            .collect();
        if matches.len() != 1 {
            Err(format!("新版本缺少唯一的 {name}，保留当前安装"))
        } else {
            Ok(matches[0].clone())
        }
    };
    let asset = find(&name)?;
    let checksums = find("SHA256SUMS.txt")?;
    Ok(Some(Offer {
        release,
        asset,
        checksums,
        installation: current,
    }))
}
pub fn check(root: &Path) -> Result<Option<Offer>, String> {
    let current = installation(root)?;
    let mut response = agent(15)
        .get(UPDATE_RELEASES_API)
        .header("User-Agent", "deepseek-harness-rs-updater")
        .call()
        .map_err(|e| e.to_string())?;
    let releases = response
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_json()
        .map_err(|e| e.to_string())?;
    select(releases, current)
}
fn asset_url(asset: &Asset, tag: &str) -> Result<(), String> {
    let url = url::Url::parse(&asset.browser_download_url).map_err(|_| "更新地址无效")?;
    let expected = format!(
        "/qiu7824/deepseek-harness-rs/releases/download/{tag}/{}",
        asset.name
    );
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != expected
        || url.query().is_some()
    {
        return Err("更新资产不属于当前官方发布".into());
    }
    Ok(())
}
fn checksum(text: &str, name: &str) -> Result<String, String> {
    let rows: Vec<_> = text
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let hash = words.next()?;
            let file = words.next()?.trim_start_matches('*');
            (file == name
                && hash.len() == 64
                && hash.bytes().all(|v| v.is_ascii_hexdigit())
                && words.next().is_none())
            .then(|| hash.to_ascii_lowercase())
        })
        .collect();
    if rows.len() == 1 {
        Ok(rows[0].clone())
    } else {
        Err("安装包缺少唯一的 SHA256 校验值".into())
    }
}
fn digest(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
fn encrypt(value: &str) -> String {
    use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
    let cipher = aes::Aes128::new(GenericArray::from_slice(b"lanZouY-disk-app"));
    let mut bytes = value.as_bytes().to_vec();
    let pad = 16 - bytes.len() % 16;
    bytes.extend(std::iter::repeat_n(pad as u8, pad));
    for block in bytes.chunks_mut(16) {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}
fn ilanzou(mirror: &Mirror, expected_name: &str) -> Result<String, String> {
    let share = url::Url::parse(&mirror.share_url).map_err(|_| "蓝奏分享地址无效")?;
    if share.scheme() != "https"
        || !matches!(share.host_str(), Some("ilanzou.com" | "www.ilanzou.com"))
        || !share.username().is_empty()
        || share.password().is_some()
    {
        return Err("蓝奏分享来源不受支持".into());
    }
    let key = share
        .path()
        .strip_prefix("/s/")
        .filter(|v| {
            !v.is_empty()
                && v.len() <= 128
                && v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
        .ok_or("蓝奏分享标识无效")?;
    let timestamp = now_unix_millis().to_string();
    let stamp = encrypt(&timestamp);
    let uuid = format!("{}-{}", std::process::id(), timestamp);
    let payload = [
        ("devType", "6"),
        ("devModel", "Chrome"),
        ("uuid", &uuid),
        ("extra", "2"),
        ("timestamp", &stamp),
        ("code", &mirror.code),
        ("shareId", key),
        ("type", "0"),
        ("offset", "1"),
        ("limit", "60"),
    ];
    let mut response = agent(30)
        .post("https://apix.ilanzou.com/unproved/recommend/list")
        .header("Origin", "https://www.ilanzou.com")
        .header("Referer", &mirror.share_url)
        .send_form(payload)
        .map_err(|e| format!("蓝奏解析失败：{e}"))?;
    let value: serde_json::Value = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_json()
        .map_err(|e| e.to_string())?;
    let files = value["list"][0]["fileList"]
        .as_array()
        .filter(|v| v.len() == 1)
        .ok_or("蓝奏分享未返回单个文件")?;
    if value["code"] != 200 || files[0]["fileName"].as_str() != Some(expected_name) {
        return Err("蓝奏分享与发布文件不匹配".into());
    }
    let id = files[0]["fileId"]
        .as_str()
        .map(str::to_string)
        .or_else(|| files[0]["fileId"].as_u64().map(|v| v.to_string()))
        .ok_or("蓝奏文件标识缺失")?;
    let mut url = url::Url::parse("https://apix.ilanzou.com/unproved/file/redirect").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("downloadId", encrypt(&format!("{id}|"))),
        ("enable", "0".into()),
        ("devType", "6".into()),
        ("uuid", uuid),
        ("timestamp", stamp),
        ("auth", encrypt(&format!("{id}|{timestamp}"))),
        ("shareId", key.into()),
    ]);
    Ok(url.into())
}
fn download_url(offer: &Offer, prefer_mirror: bool) -> Result<String, String> {
    asset_url(&offer.asset, &offer.release.tag_name)?;
    if !prefer_mirror {
        return Ok(offer.asset.browser_download_url.clone());
    }
    let asset = offer
        .release
        .assets
        .iter()
        .find(|a| a.name == "mirrors.json")
        .ok_or("这个版本尚未发布蓝奏线路，请选择 GitHub 线路")?;
    asset_url(asset, &offer.release.tag_name)?;
    let mut response = agent(20)
        .get(&asset.browser_download_url)
        .call()
        .map_err(|e| e.to_string())?;
    let mirrors: Mirrors = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_json()
        .map_err(|e| e.to_string())?;
    if parse_version(&mirrors.version) != parse_version(&offer.release.tag_name) {
        return Err("蓝奏清单版本与发布版本不同".into());
    }
    let mirror = mirrors
        .files
        .iter()
        .find(|m| m.name == offer.asset.name)
        .ok_or("蓝奏清单缺少当前平台和安装类型")?;
    ilanzou(mirror, &offer.asset.name)
}
pub fn download(
    offer: &Offer,
    cache: &Path,
    mirror: bool,
    status: impl Fn(String),
) -> Result<PathBuf, String> {
    if offer.asset.size == 0 || offer.asset.size > MAX_DOWNLOAD {
        return Err("安装包大小超出更新限制".into());
    }
    asset_url(&offer.checksums, &offer.release.tag_name)?;
    let mut response = agent(30)
        .get(&offer.checksums.browser_download_url)
        .call()
        .map_err(|e| e.to_string())?;
    let sums = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let expected = checksum(&sums, &offer.asset.name)?;
    fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    let file = cache.join(&offer.asset.name);
    if file.is_file() && digest(&file)? == expected {
        return Ok(file);
    }
    let partial = cache.join(format!("{}.{}.part", offer.asset.name, now_unix_millis()));
    let result = (|| {
        let url = download_url(offer, mirror)?;
        let mut response = agent(900)
            .get(&url)
            .call()
            .map_err(|e| format!("下载失败：{e}"))?;
        let mut reader = response.body_mut().as_reader();
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial)
            .map_err(|e| e.to_string())?;
        let mut buf = [0u8; 64 * 1024];
        let mut total = 0u64;
        let mut last = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > offer.asset.size {
                return Err("下载大小超过发布清单".into());
            }
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            let percent = total * 100 / offer.asset.size;
            if percent != last {
                status(format!("正在下载 {}：{percent}%", offer.release.tag_name));
                last = percent
            }
        }
        out.sync_all().map_err(|e| e.to_string())?;
        drop(out);
        if total != offer.asset.size || digest(&partial)? != expected {
            return Err("安装包完整性校验失败，未执行安装".into());
        }
        if file.exists() {
            fs::remove_file(&file).map_err(|e| e.to_string())?
        }
        fs::rename(&partial, &file).map_err(|e| e.to_string())?;
        Ok(file.clone())
    })();
    if partial.exists() {
        let _ = fs::remove_file(partial);
    }
    result
}

fn safe_relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
        && !path.to_string_lossy().contains(':')
        && !path.to_string_lossy().contains('\\')
}
fn extract(archive: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir(target).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    let mut count = 0usize;
    if archive.extension().is_some_and(|v| v == "zip") {
        let mut zip = zip::ZipArchive::new(fs::File::open(archive).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
            let path = PathBuf::from(entry.name());
            if !safe_relative(&path) || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
            {
                return Err("更新包包含不安全路径".into());
            }
            count += 1;
            total += entry.size();
            if count > 30000 || total > MAX_DOWNLOAD * 3 {
                return Err("更新包展开大小超限".into());
            }
            let destination = target.join(path);
            if entry.is_dir() {
                fs::create_dir_all(destination).map_err(|e| e.to_string())?
            } else {
                fs::create_dir_all(destination.parent().unwrap()).map_err(|e| e.to_string())?;
                let mut out = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(destination)
                    .map_err(|e| e.to_string())?;
                io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
            }
        }
    } else {
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(archive).map_err(|e| e.to_string())?,
        ));
        for entry in archive.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().map_err(|e| e.to_string())?;
            let kind = entry.header().entry_type();
            if !safe_relative(&path) || !(kind.is_file() || kind.is_dir()) {
                return Err("更新包包含不安全路径或链接".into());
            }
            count += 1;
            total += entry.size();
            if count > 30000 || total > MAX_DOWNLOAD * 3 {
                return Err("更新包展开大小超限".into());
            }
            entry.unpack_in(target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
#[derive(Serialize, Deserialize)]
struct ApplyPlan {
    root: PathBuf,
    staged: PathBuf,
    backup: PathBuf,
    parent_pid: u32,
    parent_created: u64,
    parent_exe: PathBuf,
    archive: PathBuf,
    sha256: String,
    version: String,
    variant: String,
}
fn files(root: &Path) -> Result<Vec<PathBuf>, String> {
    fn visit(root: &Path, dir: &Path, rows: &mut Vec<PathBuf>) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if kind.is_symlink() {
                return Err("更新目录包含链接".into());
            }
            if kind.is_dir() {
                visit(root, &entry.path(), rows)?
            } else {
                rows.push(entry.path().strip_prefix(root).unwrap().to_path_buf());
            }
        }
        Ok(())
    }
    let mut rows = vec![];
    visit(root, root, &mut rows)?;
    rows.sort();
    Ok(rows)
}
pub fn launch_install(offer: &Offer, file: &Path, root: &Path) -> Result<bool, String> {
    if offer.installation.distribution == Distribution::Installer {
        #[cfg(windows)]
        Command::new(file)
            .arg(format!("/DIR={}", root.display()))
            .current_dir(root)
            .spawn()
            .map_err(|e| e.to_string())?;
        #[cfg(not(windows))]
        open_target(&file.to_string_lossy(), localized_copy())?;
        return Ok(false);
    }
    let cache = file.parent().ok_or("更新目录无效")?;
    let temp = cache.join(format!("unpack-{}", now_unix_millis()));
    extract(file, &temp)?;
    let entries = fs::read_dir(&temp)
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if entries.len() != 1 || !entries[0].file_type().map_err(|e| e.to_string())?.is_dir() {
        return Err("便携包根目录无效".into());
    }
    let staged = entries[0].path();
    let package: serde_json::Value =
        serde_json::from_slice(&fs::read(staged.join("PACKAGE.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if package["version"].as_str().and_then(parse_version) != parse_version(&offer.release.tag_name)
        || package["variant"] != offer.installation.variant
        || !staged.join(core_executable_name()).is_file()
    {
        return Err("便携包产品或版本不匹配".into());
    }
    let parent = inspect_process(std::process::id()).map_err(|e| e.to_string())?;
    let helper = temp.join(if cfg!(windows) {
        "update-helper.exe"
    } else {
        "update-helper"
    });
    fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, &helper)
        .map_err(|e| e.to_string())?;
    let plan = ApplyPlan {
        root: root.to_path_buf(),
        staged,
        backup: temp.join("rollback"),
        parent_pid: parent.pid,
        parent_created: parent.creation_time,
        parent_exe: parent.executable,
        archive: file.to_path_buf(),
        sha256: digest(file)?,
        version: package["version"].as_str().unwrap().into(),
        variant: offer.installation.variant.clone(),
    };
    let plan_file = temp.join("apply.json");
    fs::write(&plan_file, serde_json::to_vec(&plan).unwrap()).map_err(|e| e.to_string())?;
    let mut command = Command::new(helper);
    command
        .arg("--apply-update")
        .arg(plan_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command.spawn().map_err(|e| e.to_string())?;
    Ok(true)
}
fn install_files(plan: &ApplyPlan) -> Result<(), String> {
    let list = files(&plan.staged)?;
    let root = fs::canonicalize(&plan.root).map_err(|e| e.to_string())?;
    fs::create_dir(&plan.backup).map_err(|e| e.to_string())?;
    for rel in &list {
        let dest = root.join(rel);
        let mut parent = dest.parent().ok_or("安装路径无效")?;
        while !parent.exists() {
            parent = parent.parent().ok_or("安装路径无效")?;
        }
        if !fs::canonicalize(parent)
            .map_err(|e| e.to_string())?
            .starts_with(&root)
            || dest
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err("安装目标包含外部链接".into());
        }
        if dest.is_file() {
            let old = plan.backup.join(rel);
            fs::create_dir_all(old.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(dest, old).map_err(|e| e.to_string())?;
        }
    }
    let mut written = vec![];
    let result = (|| {
        for rel in &list {
            let dest = root.join(rel);
            fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
            written.push(rel);
            fs::copy(plan.staged.join(rel), &dest).map_err(|e| e.to_string())?;
            if digest(&dest)? != digest(&plan.staged.join(rel))? {
                return Err("安装文件校验失败".into());
            }
        }
        Ok(())
    })();
    if result.is_err() {
        for rel in written {
            let dest = root.join(rel);
            let old = plan.backup.join(rel);
            if old.is_file() {
                let _ = fs::copy(old, dest);
            } else {
                let _ = fs::remove_file(dest);
            }
        }
    }
    result
}

pub fn apply(path: &Path) -> Result<(), String> {
    let plan: ApplyPlan = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if digest(&plan.archive)? != plan.sha256 {
        return Err("安装前校验失败".into());
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while inspect_process(plan.parent_pid).is_ok_and(|p| {
        p.creation_time == plan.parent_created && same_executable(&p.executable, &plan.parent_exe)
    }) {
        if std::time::Instant::now() >= deadline {
            return Err("等待启动器退出超时".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let root = fs::canonicalize(&plan.root).map_err(|e| e.to_string())?;
    let result = install_files(&plan);
    let log = path.with_file_name("result.json");
    fs::write(log,serde_json::to_vec(&serde_json::json!({"ok":result.is_ok(),"version":plan.version,"error":result.as_ref().err()})).unwrap()).map_err(|e|e.to_string())?;
    let mut command = Command::new(root.join(if cfg!(windows) {
        "dsh-launcher.exe"
    } else {
        "dsh-launcher"
    }));
    command.current_dir(&root);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    command.spawn().map_err(|e| e.to_string())?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lanzou_signature_matches_the_public_aes_protocol() {
        assert_eq!(encrypt("1789290000000"), "F16A22A15ABAECC6326021AECF7FA260");
    }
    #[test]
    fn partial_replacement_restores_files_and_keeps_user_data() {
        let temp = std::env::temp_dir().join(format!(
            "dsh-update-transaction-{}-{}",
            std::process::id(),
            now_unix_millis()
        ));
        let root = temp.join("app");
        let staged = temp.join("stage");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&staged).unwrap();
        fs::write(root.join("a-existing"), b"old").unwrap();
        fs::write(root.join("settings.json"), b"user settings").unwrap();
        fs::create_dir(root.join("z-blocked")).unwrap();
        for file in ["a-existing", "b-new", "z-blocked"] {
            fs::write(staged.join(file), b"updated").unwrap();
        }
        let plan = ApplyPlan {
            root: root.clone(),
            staged,
            backup: temp.join("rollback"),
            parent_pid: 0,
            parent_created: 0,
            parent_exe: PathBuf::new(),
            archive: PathBuf::new(),
            sha256: String::new(),
            version: "fixture".into(),
            variant: "core".into(),
        };
        assert!(install_files(&plan).is_err());
        assert_eq!(fs::read(root.join("a-existing")).unwrap(), b"old");
        assert_eq!(
            fs::read(root.join("settings.json")).unwrap(),
            b"user settings"
        );
        assert!(!root.join("b-new").exists());
        assert!(root.join("z-blocked").is_dir());
        assert!(
            temp.canonicalize()
                .unwrap()
                .starts_with(std::env::temp_dir().canonicalize().unwrap())
        );
        fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn archive_traversal_is_rejected_before_writing_outside_stage() {
        let temp = std::env::temp_dir().join(format!(
            "dsh-update-archive-{}-{}",
            std::process::id(),
            now_unix_millis()
        ));
        fs::create_dir(&temp).unwrap();
        let zip = temp.join("bad.zip");
        let mut writer = zip::ZipWriter::new(fs::File::create(&zip).unwrap());
        writer
            .start_file("../outside", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"unsafe").unwrap();
        writer.finish().unwrap();
        assert!(extract(&zip, &temp.join("stage")).is_err());
        assert!(!temp.join("outside").exists());
        assert!(
            temp.canonicalize()
                .unwrap()
                .starts_with(std::env::temp_dir().canonicalize().unwrap())
        );
        fs::remove_dir_all(temp).unwrap();
    }
    fn current(distribution: Distribution) -> Installation {
        Installation {
            version: "0.1.3-alpha.17".into(),
            variant: "skin".into(),
            platform: "windows".into(),
            arch: "x86_64".into(),
            distribution,
        }
    }
    fn release() -> Release {
        Release{tag_name:"v0.1.3-alpha.18".into(),draft:false,assets:["deepseek-harness-rs-v0.1.3-alpha.18-windows-x86_64-skin-setup.exe","deepseek-harness-rs-v0.1.3-alpha.18-windows-x86_64-skin-portable.zip","SHA256SUMS.txt"].iter().map(|n|Asset{name:n.to_string(),browser_download_url:format!("https://github.com/qiu7824/deepseek-harness-rs/releases/download/v0.1.3-alpha.18/{n}"),size:123}).collect()}
    }
    #[test]
    fn preserves_installer_portable_and_variant() {
        assert!(
            select(vec![release()], current(Distribution::Installer))
                .unwrap()
                .unwrap()
                .asset
                .name
                .ends_with("skin-setup.exe")
        );
        assert!(
            select(vec![release()], current(Distribution::Portable))
                .unwrap()
                .unwrap()
                .asset
                .name
                .ends_with("skin-portable.zip")
        );
        let mut r = release();
        r.assets.remove(0);
        assert!(select(vec![r], current(Distribution::Installer)).is_err());
    }
    #[test]
    fn no_downgrade_or_draft() {
        let mut c = current(Distribution::Portable);
        c.version = "0.1.3-alpha.19".into();
        assert!(select(vec![release()], c).unwrap().is_none());
        let mut r = release();
        r.draft = true;
        assert!(
            select(vec![r], current(Distribution::Installer))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn checksum_and_paths_fail_closed() {
        let hash = "a".repeat(64);
        assert_eq!(
            checksum(&format!("{hash}  a.zip\n"), "a.zip").unwrap(),
            hash
        );
        assert!(checksum(&format!("{hash} a.zip\n{hash} a.zip"), "a.zip").is_err());
        for path in ["../a", "/a", "C:/a", "a/../../x", "a\\..\\b"] {
            assert!(!safe_relative(Path::new(path)), "{path}")
        }
        let mut a = release().assets.remove(0);
        a.browser_download_url = a
            .browser_download_url
            .replace("github.com/", "github.com.evil/");
        assert!(asset_url(&a, "v0.1.3-alpha.18").is_err());
    }
}
