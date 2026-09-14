//! A short-lived loopback server exposes exactly one workspace video to Chromium.
use axum::{
    body::Body,
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
};
use std::{path::PathBuf, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub(super) struct VideoServer {
    pub url: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for VideoServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
pub(super) async fn serve(path: PathBuf) -> Result<VideoServer, String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let path = Arc::new(path);
    let app = axum::Router::new().route(
        &format!("/{token}"),
        get(move |headers: HeaderMap| {
            let path = path.clone();
            async move { response(&path, &headers).await }
        }),
    );
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(VideoServer {
        url: format!("http://{addr}/{token}"),
        task,
    })
}
fn range(value: Option<&str>, len: u64) -> Result<(u64, u64, bool), ()> {
    if len == 0 {
        return Err(());
    }
    let Some(value) = value else {
        return Ok((0, len - 1, false));
    };
    let value = value.strip_prefix("bytes=").ok_or(())?;
    if value.contains(',') {
        return Err(());
    }
    let (start, end) = value.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        return Ok((len.saturating_sub(suffix), len - 1, true));
    }
    let start = start.parse::<u64>().map_err(|_| ())?;
    let end = if end.is_empty() {
        len - 1
    } else {
        end.parse::<u64>().map_err(|_| ())?.min(len - 1)
    };
    if start > end {
        return Err(());
    }
    Ok((start, end, true))
}
async fn response(path: &std::path::Path, headers: &HeaderMap) -> Response {
    let empty = |status| {
        Response::builder()
            .status(status)
            .body(Body::empty())
            .unwrap()
    };
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return empty(StatusCode::NOT_FOUND);
    };
    let Ok(meta) = file.metadata().await else {
        return empty(StatusCode::NOT_FOUND);
    };
    let Ok((start, end, partial)) = range(
        headers.get("range").and_then(|v| v.to_str().ok()),
        meta.len(),
    ) else {
        return Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header("content-range", format!("bytes */{}", meta.len()))
            .body(Body::empty())
            .unwrap();
    };
    if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let stream = futures::stream::try_unfold(
        (file, end - start + 1),
        |(mut file, remaining)| async move {
            if remaining == 0 {
                return Ok::<_, std::io::Error>(None);
            }
            let mut bytes = vec![0; remaining.min(64 * 1024) as usize];
            let read = file.read(&mut bytes).await?;
            if read == 0 {
                return Ok(None);
            }
            bytes.truncate(read);
            Ok(Some((bytes, (file, remaining - read as u64))))
        },
    );
    let media = match path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "webm" => "video/webm",
        "ogg" | "ogv" => "video/ogg",
        "mov" => "video/quicktime",
        _ => "video/mp4",
    };
    let mut response = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header("content-type", media)
        .header("accept-ranges", "bytes")
        .header("cache-control", "no-store")
        .header("content-length", end - start + 1);
    if partial {
        response = response.header(
            "content-range",
            format!("bytes {start}-{end}/{}", meta.len()),
        );
    }
    response.body(Body::from_stream(stream)).unwrap()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ranges_support_seek_suffix_and_reject_invalid_requests() {
        assert_eq!(range(Some("bytes=10-19"), 100), Ok((10, 19, true)));
        assert_eq!(range(Some("bytes=-20"), 100), Ok((80, 99, true)));
        assert_eq!(range(Some("bytes=90-"), 100), Ok((90, 99, true)));
        assert!(range(Some("bytes=100-"), 100).is_err());
        assert!(range(Some("bytes=0-1,4-5"), 100).is_err());
        assert!(range(None, 0).is_err());
    }
}
