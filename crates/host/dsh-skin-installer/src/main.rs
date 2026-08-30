use std::fs;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

const PAYLOAD_MARKER: &[u8] = b"\n__DSH_SKIN_PAYLOAD_V1_4F92C3A7__\n";
const MAX_FILES: usize = 4_096;
const MAX_FILE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 512 * 1024 * 1024;

fn validate_target(target: &Path) -> Result<(), String> {
    if !target.is_absolute() {
        return Err(format!(
            "skin target must be an absolute path: {}",
            target.display()
        ));
    }
    Ok(())
}

fn payload_offset(file: &mut fs::File) -> Result<u64, String> {
    let size = file.metadata().map_err(|error| error.to_string())?.len();
    if size < PAYLOAD_MARKER.len() as u64 {
        return Err("skin payload marker was not found".to_string());
    }
    let mut position = size;
    let mut suffix = Vec::new();
    let chunk_bytes = 64 * 1024_u64;
    while position > 0 {
        let start = position.saturating_sub(chunk_bytes);
        let length = usize::try_from(position - start).map_err(|error| error.to_string())?;
        file.seek(SeekFrom::Start(start))
            .map_err(|error| error.to_string())?;
        let mut chunk = vec![0_u8; length];
        file.read_exact(&mut chunk)
            .map_err(|error| error.to_string())?;
        chunk.extend_from_slice(&suffix);
        if let Some(index) = find_last(&chunk, PAYLOAD_MARKER) {
            return Ok(start + index as u64 + PAYLOAD_MARKER.len() as u64);
        }
        let keep = PAYLOAD_MARKER.len().saturating_sub(1).min(chunk.len());
        suffix = chunk[..keep].to_vec();
        position = start;
    }
    Err("skin payload marker was not found".to_string())
}

fn find_last(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

fn sanitize_entry(name: &str) -> Result<PathBuf, String> {
    let path = Path::new(name);
    if path.is_absolute() {
        return Err(format!("absolute skin payload path is forbidden: {name}"));
    }
    let mut safe = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(segment) => safe.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(format!("unsafe skin payload path: {name}"));
            }
        }
    }
    if safe.as_os_str().is_empty() {
        return Err("empty skin payload path".to_string());
    }
    Ok(safe)
}

fn install_payload(executable: &Path, target: &Path) -> Result<(usize, u64), String> {
    validate_target(target)?;
    let mut file = fs::File::open(executable).map_err(|error| error.to_string())?;
    let offset = payload_offset(&mut file)?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut payload = Vec::new();
    file.read_to_end(&mut payload)
        .map_err(|error| error.to_string())?;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(payload)).map_err(|error| error.to_string())?;
    if archive.len() > MAX_FILES {
        return Err(format!(
            "skin payload contains too many files: {}",
            archive.len()
        ));
    }
    fs::create_dir_all(target).map_err(|error| error.to_string())?;
    let mut files = 0_usize;
    let mut total_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let relative = sanitize_entry(entry.name())?;
        if relative
            .components()
            .next()
            .and_then(|component| match component {
                Component::Normal(segment) => Some(segment),
                _ => None,
            })
            != Some(std::ffi::OsStr::new("skins"))
        {
            return Err(format!(
                "skin payload entry is outside skins/: {}",
                entry.name()
            ));
        }
        if entry.size() > MAX_FILE_BYTES {
            return Err(format!("skin payload file is too large: {}", entry.name()));
        }
        total_bytes = total_bytes
            .checked_add(entry.size())
            .ok_or_else(|| "skin payload size overflow".to_string())?;
        if total_bytes > MAX_TOTAL_BYTES {
            return Err("skin payload exceeds the total extraction limit".to_string());
        }
        let output = target.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&output).map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut destination = fs::File::create(&output).map_err(|error| error.to_string())?;
        io::copy(&mut entry, &mut destination).map_err(|error| error.to_string())?;
        files += 1;
    }
    Ok((files, total_bytes))
}

fn default_target(executable: &Path) -> Result<PathBuf, String> {
    let root = executable
        .parent()
        .ok_or_else(|| "skin installer has no parent directory".to_string())?;
    Ok(root.join("web").join("dist"))
}

fn main() {
    let result = (|| {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let target = std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(|| default_target(&executable))?;
        let (files, bytes) = install_payload(&executable, &target)?;
        println!(
            "DeepSeek Harness-rs skins installed: {} files, {} bytes -> {}",
            files,
            bytes,
            target.join("skins").display()
        );
        Ok::<(), String>(())
    })();
    if let Err(error) = result {
        eprintln!("DeepSeek Harness-rs skin installer failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{PAYLOAD_MARKER, find_last, payload_offset, sanitize_entry, validate_target};
    use std::io::Write;
    use std::path::{Path, PathBuf};

    #[test]
    fn finds_the_last_payload_marker() {
        let mut bytes = b"prefix".to_vec();
        bytes.extend_from_slice(PAYLOAD_MARKER);
        bytes.extend_from_slice(b"payload");
        assert_eq!(find_last(&bytes, PAYLOAD_MARKER), Some(6));
    }

    #[test]
    fn payload_offset_reads_an_appended_archive() {
        let path = std::env::temp_dir().join(format!(
            "dsh-skin-installer-marker-{}-{:?}.bin",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut file = std::fs::File::create(&path).expect("create fixture");
        file.write_all(b"native-stub").expect("write stub");
        file.write_all(PAYLOAD_MARKER).expect("write marker");
        file.write_all(b"zip-data").expect("write payload");
        drop(file);
        let mut file = std::fs::File::open(&path).expect("open fixture");
        let offset = payload_offset(&mut file).expect("find marker");
        assert_eq!(
            offset,
            b"native-stub".len() as u64 + PAYLOAD_MARKER.len() as u64
        );
        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn extraction_paths_are_confined_to_the_skin_tree() {
        assert_eq!(
            sanitize_entry("skins/theme/skin.css").unwrap(),
            PathBuf::from("skins/theme/skin.css")
        );
        assert!(sanitize_entry("../outside.txt").is_err());
        assert!(sanitize_entry("/absolute.txt").is_err());
    }

    #[test]
    fn target_must_be_absolute() {
        assert!(validate_target(Path::new("relative/web/dist")).is_err());
        assert!(validate_target(Path::new("C:/skin/web/dist")).is_ok());
    }
}
