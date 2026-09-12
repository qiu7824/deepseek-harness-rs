use sha2::Digest;
use std::io::Write;

#[test]
fn image_zip_import_runs_through_the_async_cli_entrypoint() {
    let root = std::env::temp_dir().join(format!("dsh-cli-image-import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let image = vec![
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6,
        0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 240,
        31, 0, 5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let digest = format!("{:x}", sha2::Sha256::digest(&image));
    let source = root.join("with-image.zip");
    let mut archive = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
    let options = zip::write::SimpleFileOptions::default();
    archive.start_file("session.jsonl", options).unwrap();
    writeln!(archive, "{}", serde_json::json!({"type":"session","version":3,"id":"cli-image-import","createdAt":1,"delegationDepth":0,"isSeeded":false})).unwrap();
    writeln!(archive, "{}", serde_json::json!({"seq":0,"time":1,"type":"user/message","surfaceOp":"append","data":{"id":"image-message","role":"user","source":{"kind":"user"},"content":[{"type":"image","attachment":{"attachmentId":format!("sha256:{digest}"),"mediaType":"image/png","bytes":image.len(),"width":1,"height":1,"name":"frame.png"}}]}})).unwrap();
    archive
        .start_file(format!("media/sha256:{digest}.png"), options)
        .unwrap();
    archive.write_all(&image).unwrap();
    archive.finish().unwrap();
    let destination = root.join("home");
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_dsh"))
        .args(["history", "import"])
        .arg(&source)
        .arg("--to")
        .arg(&destination)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("imported_sessions=1"));
    let object = destination
        .join("attachments/v1/objects")
        .join(&digest[..2])
        .join(&digest);
    assert_eq!(std::fs::read(object).unwrap(), image);
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(root).unwrap();
}
