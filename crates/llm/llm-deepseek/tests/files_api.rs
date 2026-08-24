use dsh_llm_deepseek::{FilesErrorCode, classify_files_status, parse_file_object};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[test]
fn validates_provider_file_objects_strictly() {
    let parsed = parse_file_object(&json!({
        "id": "file-123", "object": "file", "bytes": 42, "created_at": 1234,
        "filename": "image.webp", "purpose": "user_data", "expires_at": 9999
    }))
    .expect("valid file");
    assert_eq!(parsed.id.as_str(), "file-123");
    assert_eq!(parsed.bytes, 42);
    assert_eq!(parsed.expires_at, Some(9999));
    for invalid in [
        json!({}),
        json!({"id":"", "object":"file", "bytes":42, "created_at":1, "filename":"x", "purpose":"user_data"}),
        json!({"id":"file", "object":"file", "bytes":-1, "created_at":1, "filename":"x", "purpose":"user_data"}),
        json!({"id":"file", "object":"file", "bytes":1, "created_at":1, "filename":"x", "purpose":"wrong"}),
    ] {
        assert!(parse_file_object(&invalid).is_err(), "{invalid}");
    }
}

#[test]
fn classifies_files_http_failures_for_recovery_policy() {
    assert_eq!(classify_files_status(401), FilesErrorCode::Auth);
    assert_eq!(classify_files_status(403), FilesErrorCode::Auth);
    assert_eq!(classify_files_status(429), FilesErrorCode::RateLimit);
    assert_eq!(classify_files_status(500), FilesErrorCode::Server);
    assert_eq!(classify_files_status(400), FilesErrorCode::FilesApi);
}

#[tokio::test]
async fn files_client_uploads_and_deletes_over_the_real_http_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    let server = tokio::spawn(async move {
        for (expected_method, response) in [
            (
                "POST /files ",
                json!({"id":"file-1","object":"file","bytes":3,"created_at":1,"filename":"pixel.webp","purpose":"user_data","expires_at":7200}),
            ),
            (
                "DELETE /files/file-1 ",
                json!({"id":"file-1","object":"file","deleted":true}),
            ),
        ] {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut bytes = vec![0; 32 * 1024];
            let read = socket.read(&mut bytes).await.expect("read");
            let request = String::from_utf8_lossy(&bytes[..read]);
            assert!(request.starts_with(expected_method), "{request}");
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-key")
            );
            if expected_method.starts_with("POST") {
                assert!(request.contains("name=\"purpose\""));
                assert!(request.contains("user_data"));
                assert!(request.contains("expires_after[seconds]"));
            }
            let body = response.to_string();
            socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(), body
            ).as_bytes()).await.expect("write");
        }
    });
    let client = dsh_llm_deepseek::DeepSeekFilesClient::new(
        format!("http://{address}"),
        "test-key",
        std::time::Duration::from_secs(2),
    );
    let file = client
        .upload(vec![1, 2, 3], "image/webp", "pixel.webp", 7200)
        .await
        .expect("upload");
    assert_eq!(file.id.as_str(), "file-1");
    client.delete(&file.id).await.expect("delete");
    server.await.expect("server");
}
