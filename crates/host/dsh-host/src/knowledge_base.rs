//! Host wiring for local knowledge bases: the SQLite store under the data
//! root, the model tools and runtime-context note, and the
//! `/__dsh-knowledge/*` management route for the Web and desktop clients.

use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use dsh_knowledge_base::{KnowledgeError, KnowledgeStore};
use serde_json::{Value, json};

/// Largest request body: one base64-encoded document at the extractor limit.
const MAX_UPLOAD_BODY: usize = 90 * 1024 * 1024;

/// Open the store and register the model tools.
pub fn install(ctx: &cordis::Context, data_root: &Path) -> Option<Arc<KnowledgeStore>> {
    match KnowledgeStore::open(data_root.join("knowledge").join("knowledge.sqlite")) {
        Ok(store) => {
            if let Err(error) = dsh_knowledge_base::register(ctx, store.clone()) {
                eprintln!("knowledge base tools were not registered: {error}");
            }
            Some(store)
        }
        Err(error) => {
            eprintln!("knowledge base storage is unavailable: {error}");
            None
        }
    }
}

fn text<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn operation(store: &KnowledgeStore, name: &str, args: &Value) -> Result<Value, KnowledgeError> {
    let id = text(args, "id");
    match name {
        "catalog" => Ok(json!({
            "bases": store.bases()?,
            "extensions": dsh_knowledge_base::extract::supported_extensions(),
            "maxDocumentBytes": dsh_knowledge_base::extract::MAX_DOCUMENT_BYTES,
        })),
        "create" => {
            Ok(json!({"base": store.create_base(text(args, "name"), text(args, "description"))?}))
        }
        "update" => Ok(json!({"base": store.update_base(
            id,
            args.get("name").and_then(Value::as_str),
            args.get("description").and_then(Value::as_str),
            args.get("enabled").and_then(Value::as_bool),
        )?})),
        "delete" => {
            store.delete_base(id)?;
            Ok(json!({"id": id, "deleted": true}))
        }
        "documents" => Ok(json!({"documents": store.documents(id)?})),
        "upload" => {
            let data = base64::engine::general_purpose::STANDARD
                .decode(text(args, "data"))
                .map_err(|_| KnowledgeError::new("bad_request", "文件内容编码无效"))?;
            Ok(json!({"document": store.add_document(id, text(args, "name"), None, &data)?}))
        }
        "importPath" => {
            let path = text(args, "path").trim();
            if path.is_empty() || !Path::new(path).is_absolute() {
                return Err(KnowledgeError::new(
                    "bad_request",
                    "请填写文件或文件夹的完整路径",
                ));
            }
            Ok(json!({"report": store.import_path(id, Path::new(path))?}))
        }
        "deleteDocument" => {
            store.delete_document(id)?;
            Ok(json!({"id": id, "deleted": true}))
        }
        "search" => {
            let scope = args.get("baseIds").and_then(Value::as_array).map(|ids| {
                ids.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            });
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(10) as usize;
            Ok(json!({"results": store.search(text(args, "query"), scope.as_deref(), limit)?}))
        }
        _ => Err(KnowledgeError::new("not_found", "未知的知识库操作")),
    }
}

fn status_of(error: &KnowledgeError) -> u16 {
    match error.code {
        "not_found" => 404,
        "duplicate" | "duplicate_name" => 409,
        "storage" | "io" => 500,
        _ => 400,
    }
}

pub fn attach(
    store: Option<Arc<KnowledgeStore>>,
    server: &Arc<dsh_host_webserver::WebServer>,
    allow_remote: bool,
) -> dsh_host_webserver::RouteDisposer {
    server.register(dsh_host_webserver::WebRoute {
        kind: dsh_host_webserver::WebRouteKind::Prefix,
        path: "/__dsh-knowledge".into(),
        handler: Arc::new(move |request| {
            let store = store.clone();
            Box::pin(async move {
                let allowed = request.method() == http::Method::POST
                    && super::trusted_web_request(&request, allow_remote);
                let name = request
                    .uri()
                    .path()
                    .trim_start_matches("/__dsh-knowledge/")
                    .to_string();
                let (status, body) = if !allowed {
                    (403, json!({"error": "forbidden", "code": "forbidden"}))
                } else if let Some(store) = store {
                    match axum::body::to_bytes(
                        axum::body::Body::new(request.into_body()),
                        MAX_UPLOAD_BODY,
                    )
                    .await
                    {
                        Err(_) => (413, json!({"error": "文件过大", "code": "too_large"})),
                        Ok(bytes) => match serde_json::from_slice::<Value>(if bytes.is_empty() {
                            b"{}"
                        } else {
                            &bytes
                        }) {
                            Err(_) => {
                                (400, json!({"error": "请求格式无效", "code": "bad_request"}))
                            }
                            Ok(args) => {
                                let outcome = tokio::task::spawn_blocking(move || {
                                    operation(&store, &name, &args)
                                })
                                .await
                                .unwrap_or_else(|error| {
                                    Err(KnowledgeError::new("storage", error.to_string()))
                                });
                                match outcome {
                                    Ok(value) => (200, value),
                                    Err(error) => (
                                        status_of(&error),
                                        json!({"error": error.message, "code": error.code}),
                                    ),
                                }
                            }
                        },
                    }
                } else {
                    (
                        503,
                        json!({"error": "知识库存储不可用", "code": "unavailable"}),
                    )
                };
                Ok(http::Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .header("cache-control", "no-store")
                    .body(axum::body::Body::from(body.to_string()))
                    .unwrap())
            })
        }),
    })
}
