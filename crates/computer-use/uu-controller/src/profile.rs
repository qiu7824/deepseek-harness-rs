//! Installed-client identity and signing-data discovery. No version allowlist is used.
use sha2::{Digest, Sha256};

pub struct ClientProfile {
    pub version: String,
    pub build: String,
    pub hash: String,
    pub signing_key_rva: u32,
    pub exact_profile: bool,
}

const KNOWN_CLIENTS: &[(&str, &str, u32)] = &[
    (
        "4.39.2.1561",
        "1587925c43fe9f5841ea6292b64894dde3dd59bb01832f0f288f910b553c2590",
        0x3b08118,
    ),
    (
        "4.38.3.9325",
        "2a3263062c9cbfe0dcaf81d9ec95cca480802a86b7d2fc89ef033f86a61b3853",
        0x3b06e78,
    ),
];
// Fingerprint only: the vendor signing data remains in the installed client.
const SIGNING_DATA_SHA256: &str =
    "40cb3a32f9f6f90d9962fb9736685bb881979e848b5956649b6dae3cc957528c";

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(at..at.checked_add(2)?)?.try_into().ok()?,
    ))
}
fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

struct Section<'a> {
    rva: u32,
    bytes: &'a [u8],
    readonly_data: bool,
}

fn sections(bytes: &[u8]) -> Option<Vec<Section<'_>>> {
    if bytes.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(bytes, 60)? as usize;
    if bytes.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    let count = u16_at(bytes, pe.checked_add(6)?)? as usize;
    if !(1..=96).contains(&count) {
        return None;
    }
    let optional = u16_at(bytes, pe.checked_add(20)?)? as usize;
    let table = pe.checked_add(24)?.checked_add(optional)?;
    let mut result = Vec::with_capacity(count);
    for index in 0..count {
        let row = table.checked_add(index.checked_mul(40)?)?;
        let rva = u32_at(bytes, row.checked_add(12)?)?;
        let size = u32_at(bytes, row.checked_add(16)?)? as usize;
        let raw = u32_at(bytes, row.checked_add(20)?)? as usize;
        let flags = u32_at(bytes, row.checked_add(36)?)?;
        let data = bytes.get(raw..raw.checked_add(size)?)?;
        rva.checked_add(u32::try_from(size).ok()?)?;
        result.push(Section {
            rva,
            bytes: data,
            readonly_data: flags & 0x40000040 == 0x40000040 && flags & 0xa0000000 == 0,
        });
    }
    Some(result)
}

fn locate_signing_rva(bytes: &[u8], digest: &str, preferred: Option<u32>) -> Result<u32, String> {
    let sections = sections(bytes).ok_or("UU 客户端 PE 结构无效")?;
    let matches = |key: &[u8]| key.len() == 24 && format!("{:x}", Sha256::digest(key)) == digest;
    if let Some(rva) = preferred {
        for section in &sections {
            if let Some(offset) = rva.checked_sub(section.rva).map(|n| n as usize) {
                if section.readonly_data
                    && section
                        .bytes
                        .get(offset..offset.saturating_add(24))
                        .is_some_and(matches)
                {
                    return Ok(rva);
                }
            }
        }
    }
    let mut found = None;
    for section in sections.iter().filter(|section| section.readonly_data) {
        let mut offset = 0usize;
        for record in section.bytes.split_inclusive(|byte| *byte == 0) {
            if record.len() == 25 && record[24] == 0 && matches(&record[..24]) {
                let rva = section
                    .rva
                    .checked_add(offset as u32)
                    .ok_or("UU 接口数据位置溢出")?;
                if found.is_some_and(|previous| previous != rva) {
                    return Err("UU 接口签名数据位置不唯一，无法确定调用布局".into());
                }
                found = Some(rva);
            }
            offset += record.len();
        }
    }
    found.ok_or("未能在当前 UU 客户端中定位接口签名数据，请检查客户端文件是否完整".into())
}

fn profile_identity(hash: &str, detected_version: Option<[u16; 4]>) -> (String, Option<u32>, bool) {
    if let Some((version, _, rva)) = KNOWN_CLIENTS
        .iter()
        .find(|(_, known_hash, _)| *known_hash == hash)
    {
        return ((*version).into(), Some(*rva), true);
    }
    let version = detected_version
        .map(|v| format!("{}.{}.{}.{}", v[0], v[1], v[2], v[3]))
        .unwrap_or_else(|| "unknown".into());
    let preferred = KNOWN_CLIENTS
        .iter()
        .find(|(known_version, _, _)| *known_version == version)
        .map(|(_, _, rva)| *rva);
    (version, preferred, false)
}

#[cfg(windows)]
fn file_version(path: &std::path::Path) -> Option<[u16; 4]> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VS_FIXEDFILEINFO, VerQueryValueW,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let size = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), std::ptr::null_mut()) };
    if !(52..=1024 * 1024).contains(&size) {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, size, data.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    let mut fixed = std::ptr::null_mut();
    let mut len = 0u32;
    if unsafe {
        VerQueryValueW(
            data.as_ptr().cast(),
            windows_sys::w!("\\"),
            &mut fixed,
            &mut len,
        )
    } == 0
        || fixed.is_null()
        || len < std::mem::size_of::<VS_FIXEDFILEINFO>() as u32
    {
        return None;
    }
    let info = unsafe { std::ptr::read_unaligned(fixed.cast::<VS_FIXEDFILEINFO>()) };
    if info.dwSignature != 0xfeef04bd {
        return None;
    }
    Some([
        (info.dwFileVersionMS >> 16) as u16,
        info.dwFileVersionMS as u16,
        (info.dwFileVersionLS >> 16) as u16,
        info.dwFileVersionLS as u16,
    ])
}

#[cfg(windows)]
pub fn load(path: &std::path::Path) -> Result<ClientProfile, String> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| "未找到 UU 客户端")?;
    const MAX_CLIENT_BYTES: u64 = 512 * 1024 * 1024;
    let mut bytes = Vec::new();
    file.take(MAX_CLIENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "读取 UU 客户端失败")?;
    if bytes.len() as u64 > MAX_CLIENT_BYTES {
        return Err("UU 客户端文件超过大小限制".into());
    }
    let hash = format!("{:x}", Sha256::digest(&bytes));
    let (version, preferred, exact_profile) = profile_identity(&hash, file_version(path));
    let signing_key_rva = locate_signing_rva(&bytes, SIGNING_DATA_SHA256, preferred)?;
    let build = version
        .rsplit('.')
        .next()
        .filter(|value| value.bytes().all(|b| b.is_ascii_digit()))
        .unwrap_or("0")
        .into();
    Ok(ClientProfile {
        version,
        build,
        hash,
        signing_key_rva,
        exact_profile,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    const FIXTURE_KEY: &[u8; 24] = b"fixture-signing-data-012";
    fn fixture(rva: u32, offset: usize) -> Vec<u8> {
        let mut bytes = vec![0; 0x600];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&0x80u32.to_le_bytes());
        bytes[0x80..0x84].copy_from_slice(b"PE\0\0");
        bytes[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        let row = 0x98;
        bytes[row + 12..row + 16].copy_from_slice(&rva.to_le_bytes());
        bytes[row + 16..row + 20].copy_from_slice(&0x400u32.to_le_bytes());
        bytes[row + 20..row + 24].copy_from_slice(&0x200u32.to_le_bytes());
        bytes[row + 36..row + 40].copy_from_slice(&0x40000040u32.to_le_bytes());
        bytes[0x200 + offset..0x200 + offset + 24].copy_from_slice(FIXTURE_KEY);
        bytes
    }
    fn digest() -> String {
        format!("{:x}", Sha256::digest(FIXTURE_KEY))
    }
    #[test]
    fn known_and_resigned_clients_keep_the_installed_version() {
        let known = profile_identity(KNOWN_CLIENTS[0].1, None);
        assert_eq!(known, ("4.39.2.1561".into(), Some(0x3b08118), true));
        let resigned = profile_identity("different-signature", Some([4, 39, 2, 1561]));
        assert_eq!(resigned, ("4.39.2.1561".into(), Some(0x3b08118), false));
        assert_eq!(
            profile_identity("old", Some([4, 38, 3, 9325])).1,
            Some(0x3b06e78)
        );
    }
    #[test]
    fn future_versions_are_identified_without_assuming_an_old_layout() {
        assert_eq!(
            profile_identity("future", Some([5, 1, 2, 9001])),
            ("5.1.2.9001".into(), None, false)
        );
        assert_eq!(
            profile_identity("missing-resource", None),
            ("unknown".into(), None, false)
        );
    }
    #[test]
    fn relocated_signing_data_is_discovered_and_stale_rva_is_not_read() {
        let bytes = fixture(0x1000, 0x131);
        assert_eq!(
            locate_signing_rva(&bytes, &digest(), Some(0x1050)).unwrap(),
            0x1131
        );
        assert_eq!(
            locate_signing_rva(&bytes, &digest(), Some(0x1131)).unwrap(),
            0x1131
        );
    }
    #[test]
    fn signing_discovery_checks_data_fingerprint_section_and_uniqueness() {
        let bytes = fixture(0x1000, 0x131);
        assert!(locate_signing_rva(&bytes, "different-signing-data", Some(0x1131)).is_err());
        let mut writable = bytes.clone();
        writable[0xbc..0xc0].copy_from_slice(&0xc0000040u32.to_le_bytes());
        assert!(locate_signing_rva(&writable, &digest(), Some(0x1131)).is_err());
        let mut duplicate = bytes;
        duplicate[0x401..0x419].copy_from_slice(FIXTURE_KEY);
        assert!(
            locate_signing_rva(&duplicate, &digest(), None)
                .unwrap_err()
                .contains("不唯一")
        );
    }
    #[test]
    fn truncated_and_overflowing_pe_images_fail_without_panicking() {
        let bytes = fixture(0x1000, 0x131);
        for len in [0, 1, 63, 0x90, 0xa0, 0x400] {
            assert!(locate_signing_rva(&bytes[..len], &digest(), None).is_err());
        }
        let mut invalid = bytes;
        invalid[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(locate_signing_rva(&invalid, &digest(), None).is_err());
    }
}
