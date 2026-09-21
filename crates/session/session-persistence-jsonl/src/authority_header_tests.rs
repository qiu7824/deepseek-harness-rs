use super::*;
use dsh_session::{SESSION_FORMAT_VERSION, session_id};

struct TestRoot(PathBuf);

impl TestRoot {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("dsh-header-authority-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        Self(root)
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        if !self.0.exists() {
            return;
        }
        let resolved = self.0.canonicalize().unwrap();
        assert_eq!(
            resolved.parent(),
            Some(std::env::temp_dir().canonicalize().unwrap().as_path())
        );
        assert!(
            resolved
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("dsh-header-authority-")
        );
        std::fs::remove_dir_all(resolved).unwrap();
    }
}

fn header() -> SessionHeader {
    SessionHeader {
        version: SESSION_FORMAT_VERSION,
        id: session_id("target"),
        created_at: 1,
        cwd: None,
        parent_session: Some(session_id("parent")),
        is_seeded: false,
        origin: Some("subagent".into()),
        delegation_depth: Some(1),
        agent_preset: None,
    }
}

fn header_bytes(header: &SessionHeader) -> Vec<u8> {
    let mut text =
        serde_json::to_vec(&to_header_line(header, Some(SessionLogOffset::ZERO)).unwrap()).unwrap();
    text.push(b'\n');
    text
}

#[tokio::test]
async fn authority_snapshot_reads_only_the_target_header_and_never_its_body() {
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let root = TestRoot::new();
        let ctx = Context::root();
        let backend = JsonlSessionPersistence::install(
            &ctx,
            JsonlConfig {
                root: root.0.to_string_lossy().into_owned(),
                compression,
                ..Default::default()
            },
        )
        .unwrap();
        let header = header();
        let id = header.id.clone();
        let mut bytes = backend
            .encode_materialization(&header, SessionLogOffset::ZERO, &[])
            .unwrap();
        bytes.extend(vec![0xff; 1024 * 1024]);
        let file = log_path(&root.0.to_string_lossy(), None, &id, compression);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, &bytes).unwrap();
        let snapshot = SessionPersistenceApi::read_snapshot(backend.as_ref(), &id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            snapshot.header, header,
            "a control authority read cannot parse a corrupt or oversized conversation body"
        );
        assert_eq!(
            std::fs::read(&file).unwrap(),
            bytes,
            "metadata validation must not repair or mutate the log"
        );
        drop(backend);
        drop(ctx);
    }
}

#[test]
fn streaming_writer_window_is_accepted_and_a_larger_valid_window_is_rejected() {
    let root = TestRoot::new();
    let path = root.0.join("header.zstd");
    let plaintext = header_bytes(&header());
    let frame = compress_zstd_frame(&plaintext).unwrap();
    assert_eq!(
        frame[4] & 0x20,
        0,
        "the repository writer uses an explicit streaming window"
    );
    assert_eq!(
        frame[5], 0x58,
        "default streaming writer advertises a 2 MiB window"
    );
    std::fs::write(&path, &frame).unwrap();
    let actual = read_authority_header(&path, JsonlCompression::Zstd)
        .unwrap()
        .unwrap();
    assert_eq!(parse_header_meta(&actual), Some(header()));

    let mut oversized_window = frame;
    oversized_window[5] = ((MAX_AUTHORITY_WINDOW_LOG + 1 - 10) << 3) as u8;
    assert_eq!(
        decompress_zstd_frame(&oversized_window).unwrap(),
        plaintext,
        "the negative fixture is valid Zstandard, not malformed compressed data"
    );
    std::fs::write(&path, oversized_window).unwrap();
    let error = read_authority_header(&path, JsonlCompression::Zstd).unwrap_err();
    assert!(
        error.contains("memory") || error.contains("window"),
        "oversized window must be refused before allocation: {error}"
    );
}

#[test]
fn plain_and_compressed_headers_keep_the_independent_plaintext_limit() {
    let root = TestRoot::new();
    let mut large = header();
    large.cwd = Some("x".repeat(MAX_AUTHORITY_HEADER_BYTES as usize));
    let plaintext = header_bytes(&large);
    assert!(plaintext.len() as u64 > MAX_AUTHORITY_HEADER_BYTES);
    for compression in [JsonlCompression::None, JsonlCompression::Zstd] {
        let path = root.0.join(format!("large{}", log_suffix(compression)));
        let bytes = if compression == JsonlCompression::Zstd {
            compress_zstd_frame(&plaintext).unwrap()
        } else {
            plaintext.clone()
        };
        std::fs::write(&path, bytes).unwrap();
        let error = read_authority_header(&path, compression).unwrap_err();
        assert!(
            error.contains("exceeds 256 KiB"),
            "large plaintext must hit the output limit independently of the valid writer window: {error}"
        );
    }
}
