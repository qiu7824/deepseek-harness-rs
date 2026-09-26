//! Knowledge base storage: one SQLite file holding bases, documents, their
//! text chunks and a full-text index over segmented chunk terms.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::{Mutex, MutexGuard};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::extract;
use crate::text;

/// Target chunk size in characters and the overlap between neighbours.
pub const CHUNK_CHARS: usize = 800;
pub const CHUNK_OVERLAP: usize = 120;
/// Upper bounds of one folder import.
pub const MAX_IMPORT_FILES: usize = 500;
const MAX_IMPORT_DEPTH: usize = 8;
const SKIPPED_DIRECTORIES: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    ".venv",
    "__pycache__",
    "dist",
    "build",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnowledgeError {
    pub code: &'static str,
    pub message: String,
}

impl KnowledgeError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for KnowledgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<rusqlite::Error> for KnowledgeError {
    fn from(error: rusqlite::Error) -> Self {
        Self::new("storage", format!("知识库存储错误：{error}"))
    }
}

type Result<T> = std::result::Result<T, KnowledgeError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub document_count: u64,
    pub chunk_count: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentView {
    pub id: String,
    pub base_id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub bytes: u64,
    pub chars: u64,
    pub chunk_count: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub base_id: String,
    pub base_name: String,
    pub document_id: String,
    pub document_name: String,
    pub chunk: u64,
    pub text: String,
    pub snippet: String,
    pub score: f64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub added: Vec<DocumentView>,
    pub skipped: Vec<SkippedFile>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFile {
    pub name: String,
    pub reason: String,
}

fn now() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default();
    // Civil-from-days (Howard Hinnant), UTC.
    let days = millis.div_euclid(86_400_000);
    let rest = millis.rem_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1_000 % 60,
        rest % 1_000
    )
}

fn new_id(prefix: &str) -> String {
    format!(
        "{prefix}_{}",
        &uuid::Uuid::new_v4().simple().to_string()[..16]
    )
}

const SCHEMA: &str = r#"
            PRAGMA foreign_keys = ON;
            CREATE TABLE IF NOT EXISTS bases (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              description TEXT NOT NULL DEFAULT '',
              enabled INTEGER NOT NULL DEFAULT 1,
              created_at TEXT NOT NULL,
              updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS documents (
              id TEXT PRIMARY KEY,
              base_id TEXT NOT NULL REFERENCES bases(id) ON DELETE CASCADE,
              name TEXT NOT NULL,
              source TEXT,
              bytes INTEGER NOT NULL,
              chars INTEGER NOT NULL,
              sha256 TEXT NOT NULL,
              chunk_count INTEGER NOT NULL,
              created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS documents_base ON documents(base_id);
            CREATE TABLE IF NOT EXISTS chunks (
              id INTEGER PRIMARY KEY,
              document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
              base_id TEXT NOT NULL,
              ordinal INTEGER NOT NULL,
              text TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS chunks_document ON chunks(document_id);
            CREATE VIRTUAL TABLE IF NOT EXISTS chunk_index USING fts5(terms, tokenize = 'unicode61 remove_diacritics 0');
            "#;

/// Connections are opened per operation and closed right after, so the store
/// never keeps the database file open between requests (Windows cannot
/// remove or replace an open file). The in-process gate serializes them.
pub struct KnowledgeStore {
    path: PathBuf,
    gate: Mutex<()>,
}

/// One open connection, held together with the store's gate.
struct Session<'a> {
    _gate: MutexGuard<'a, ()>,
    connection: Connection,
}

impl std::ops::Deref for Session<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}

impl std::ops::DerefMut for Session<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        &mut self.connection
    }
}

impl KnowledgeStore {
    /// A store at `path`; the file is created on the first write.
    pub fn open(path: impl Into<PathBuf>) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            path: path.into(),
            gate: Mutex::new(()),
        }))
    }

    /// Whether any knowledge base was ever created here.
    pub fn exists(&self) -> bool {
        self.path.is_file()
    }

    fn connect(&self) -> Result<Session<'_>> {
        let gate = self.gate.lock();
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                KnowledgeError::new("storage", format!("无法创建知识库目录：{error}"))
            })?;
        }
        let connection = Connection::open(&self.path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(SCHEMA)?;
        Ok(Session {
            _gate: gate,
            connection,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn base_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<BaseView> {
        Ok(BaseView {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            enabled: row.get::<_, i64>(3)? != 0,
            created_at: row.get(4)?,
            updated_at: row.get(5)?,
            document_count: row.get::<_, i64>(6)? as u64,
            chunk_count: row.get::<_, i64>(7)? as u64,
        })
    }

    const BASE_SELECT: &'static str =
        "SELECT b.id, b.name, b.description, b.enabled, b.created_at, b.updated_at,
        (SELECT COUNT(*) FROM documents d WHERE d.base_id = b.id),
        (SELECT COALESCE(SUM(d.chunk_count), 0) FROM documents d WHERE d.base_id = b.id)
        FROM bases b";

    pub fn bases(&self) -> Result<Vec<BaseView>> {
        if !self.exists() {
            return Ok(Vec::new());
        }
        let connection = self.connect()?;
        let mut statement =
            connection.prepare(&format!("{} ORDER BY b.created_at", Self::BASE_SELECT))?;
        let rows = statement.query_map([], Self::base_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn base(&self, id: &str) -> Result<BaseView> {
        if !self.exists() {
            return Err(KnowledgeError::new("not_found", "知识库不存在或已删除"));
        }
        let connection = self.connect()?;
        connection
            .query_row(
                &format!("{} WHERE b.id = ?1", Self::BASE_SELECT),
                [id],
                Self::base_row,
            )
            .optional()?
            .ok_or_else(|| KnowledgeError::new("not_found", "知识库不存在或已删除"))
    }

    /// Resolve a base by exact id or case-insensitive name.
    pub fn find_base(&self, id_or_name: &str) -> Result<BaseView> {
        let bases = self.bases()?;
        bases
            .iter()
            .find(|base| base.id == id_or_name)
            .or_else(|| {
                bases
                    .iter()
                    .find(|base| base.name.eq_ignore_ascii_case(id_or_name.trim()))
            })
            .cloned()
            .ok_or_else(|| KnowledgeError::new("not_found", format!("找不到知识库：{id_or_name}")))
    }

    fn checked_name(&self, name: &str, except: Option<&str>) -> Result<String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 60 {
            return Err(KnowledgeError::new(
                "invalid_name",
                "名称需为 1 到 60 个字符",
            ));
        }
        if self
            .bases()?
            .iter()
            .any(|base| Some(base.id.as_str()) != except && base.name.eq_ignore_ascii_case(name))
        {
            return Err(KnowledgeError::new("duplicate_name", "已有同名知识库"));
        }
        Ok(name.to_string())
    }

    pub fn create_base(&self, name: &str, description: &str) -> Result<BaseView> {
        let name = self.checked_name(name, None)?;
        let id = new_id("kb");
        let stamp = now();
        self.connect()?.execute(
            "INSERT INTO bases (id, name, description, enabled, created_at, updated_at) VALUES (?1, ?2, ?3, 1, ?4, ?4)",
            params![id, name, description.trim().chars().take(500).collect::<String>(), stamp],
        )?;
        self.base(&id)
    }

    pub fn update_base(
        &self,
        id: &str,
        name: Option<&str>,
        description: Option<&str>,
        enabled: Option<bool>,
    ) -> Result<BaseView> {
        let current = self.base(id)?;
        let name = match name {
            Some(name) => self.checked_name(name, Some(id))?,
            None => current.name,
        };
        let description = description
            .map(|text| text.trim().chars().take(500).collect::<String>())
            .unwrap_or(current.description);
        let enabled = enabled.unwrap_or(current.enabled);
        self.connect()?.execute(
            "UPDATE bases SET name = ?2, description = ?3, enabled = ?4, updated_at = ?5 WHERE id = ?1",
            params![id, name, description, enabled as i64, now()],
        )?;
        self.base(id)
    }

    pub fn delete_base(&self, id: &str) -> Result<()> {
        self.base(id)?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM chunk_index WHERE rowid IN (SELECT id FROM chunks WHERE base_id = ?1)",
            [id],
        )?;
        transaction.execute("DELETE FROM chunks WHERE base_id = ?1", [id])?;
        transaction.execute("DELETE FROM documents WHERE base_id = ?1", [id])?;
        transaction.execute("DELETE FROM bases WHERE id = ?1", [id])?;
        transaction.commit()?;
        Ok(())
    }

    fn document_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DocumentView> {
        Ok(DocumentView {
            id: row.get(0)?,
            base_id: row.get(1)?,
            name: row.get(2)?,
            source: row.get(3)?,
            bytes: row.get::<_, i64>(4)? as u64,
            chars: row.get::<_, i64>(5)? as u64,
            chunk_count: row.get::<_, i64>(6)? as u64,
            created_at: row.get(7)?,
        })
    }

    pub fn documents(&self, base_id: &str) -> Result<Vec<DocumentView>> {
        self.base(base_id)?;
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT id, base_id, name, source, bytes, chars, chunk_count, created_at FROM documents WHERE base_id = ?1 ORDER BY created_at DESC",
        )?;
        let rows = statement.query_map([base_id], Self::document_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Extract, chunk and index one document. Identical content already in
    /// the base is rejected so repeated imports do not duplicate results.
    pub fn add_document(
        &self,
        base_id: &str,
        name: &str,
        source: Option<&str>,
        bytes: &[u8],
    ) -> Result<DocumentView> {
        self.base(base_id)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(KnowledgeError::new("invalid_name", "文件名不能为空"));
        }
        let content = extract::extract(name, bytes)
            .map_err(|message| KnowledgeError::new("unsupported", message))?;
        let sha = format!("{:x}", Sha256::digest(bytes));
        let pieces = text::chunks(&content, CHUNK_CHARS, CHUNK_OVERLAP);
        let id = new_id("doc");
        let mut connection = self.connect()?;
        let duplicate: Option<String> = connection
            .query_row(
                "SELECT name FROM documents WHERE base_id = ?1 AND sha256 = ?2",
                params![base_id, sha],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = duplicate {
            return Err(KnowledgeError::new(
                "duplicate",
                format!("相同内容的文档已在知识库中：{existing}"),
            ));
        }
        let transaction = connection.transaction()?;
        transaction.execute(
            "INSERT INTO documents (id, base_id, name, source, bytes, chars, sha256, chunk_count, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![id, base_id, name, source, bytes.len() as i64, content.chars().count() as i64, sha, pieces.len() as i64, now()],
        )?;
        {
            let mut insert_chunk = transaction.prepare(
                "INSERT INTO chunks (document_id, base_id, ordinal, text) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut insert_terms =
                transaction.prepare("INSERT INTO chunk_index (rowid, terms) VALUES (?1, ?2)")?;
            for (ordinal, piece) in pieces.iter().enumerate() {
                insert_chunk.execute(params![id, base_id, ordinal as i64, piece])?;
                let rowid = transaction.last_insert_rowid();
                // The document name is searchable in every chunk.
                insert_terms.execute(params![
                    rowid,
                    format!("{} {}", text::index_text(name), text::index_text(piece))
                ])?;
            }
        }
        transaction.execute(
            "UPDATE bases SET updated_at = ?2 WHERE id = ?1",
            params![base_id, now()],
        )?;
        transaction.commit()?;
        drop(connection);
        self.documents(base_id)?
            .into_iter()
            .find(|document| document.id == id)
            .ok_or_else(|| KnowledgeError::new("storage", "文档写入后无法读取"))
    }

    pub fn delete_document(&self, id: &str) -> Result<()> {
        if !self.exists() {
            return Err(KnowledgeError::new("not_found", "文档不存在或已删除"));
        }
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let removed = transaction.execute(
            "DELETE FROM chunk_index WHERE rowid IN (SELECT id FROM chunks WHERE document_id = ?1)",
            [id],
        )?;
        transaction.execute("DELETE FROM chunks WHERE document_id = ?1", [id])?;
        let documents = transaction.execute("DELETE FROM documents WHERE id = ?1", [id])?;
        transaction.commit()?;
        if documents == 0 && removed == 0 {
            return Err(KnowledgeError::new("not_found", "文档不存在或已删除"));
        }
        Ok(())
    }

    /// Ranked chunks of enabled bases (optionally limited to `base_ids`).
    /// Every query term must match; when nothing does, any-term matches are
    /// returned so a partial phrase still finds related passages.
    pub fn search(
        &self,
        query: &str,
        base_ids: Option<&[String]>,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let Some(strict) = text::match_expression(query) else {
            return Ok(Vec::new());
        };
        if !self.exists() {
            return Ok(Vec::new());
        }
        let limit = limit.clamp(1, 50);
        let mut hits = self.run_search(&strict, query, base_ids, limit)?;
        if hits.is_empty() && strict.contains(" AND ") {
            hits = self.run_search(&strict.replace(" AND ", " OR "), query, base_ids, limit)?;
        }
        Ok(hits)
    }

    fn run_search(
        &self,
        expression: &str,
        query: &str,
        base_ids: Option<&[String]>,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let connection = self.connect()?;
        let filter = match base_ids {
            Some(ids) if !ids.is_empty() => format!(
                " AND c.base_id IN ({})",
                ids.iter()
                    .map(|id| format!("'{}'", id.replace('\'', "''")))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            _ => String::new(),
        };
        let sql = format!(
            "SELECT c.base_id, b.name, c.document_id, d.name, c.ordinal, c.text, bm25(chunk_index) AS score
             FROM chunk_index
             JOIN chunks c ON c.id = chunk_index.rowid
             JOIN documents d ON d.id = c.document_id
             JOIN bases b ON b.id = c.base_id
             WHERE chunk_index MATCH ?1 AND b.enabled = 1{filter}
             ORDER BY score LIMIT ?2"
        );
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(params![expression, limit as i64], |row| {
            let text: String = row.get(5)?;
            Ok(SearchHit {
                base_id: row.get(0)?,
                base_name: row.get(1)?,
                document_id: row.get(2)?,
                document_name: row.get(3)?,
                chunk: row.get::<_, i64>(4)? as u64,
                snippet: text::snippet(&text, query, 160),
                text,
                score: -row.get::<_, f64>(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Import a file, or every supported file under a folder.
    pub fn import_path(&self, base_id: &str, path: &Path) -> Result<ImportReport> {
        self.base(base_id)?;
        let metadata = std::fs::metadata(path).map_err(|error| {
            KnowledgeError::new("not_found", format!("无法访问 {}：{error}", path.display()))
        })?;
        let mut files = Vec::new();
        let mut report = ImportReport::default();
        if metadata.is_file() {
            files.push(path.to_path_buf());
        } else {
            collect_files(path, 0, &mut files, &mut report);
        }
        for file in files {
            let name = file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| file.display().to_string());
            let display = file.display().to_string();
            let outcome = std::fs::read(&file)
                .map_err(|error| KnowledgeError::new("io", format!("无法读取：{error}")))
                .and_then(|bytes| self.add_document(base_id, &name, Some(&display), &bytes));
            match outcome {
                Ok(document) => report.added.push(document),
                Err(error) => report.skipped.push(SkippedFile {
                    name: display,
                    reason: error.message,
                }),
            }
        }
        Ok(report)
    }
}

fn collect_files(
    directory: &Path,
    depth: usize,
    files: &mut Vec<PathBuf>,
    report: &mut ImportReport,
) {
    if depth > MAX_IMPORT_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if files.len() >= MAX_IMPORT_FILES {
            report.truncated = true;
            return;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_symlink() || name.starts_with('.') {
            continue;
        }
        if kind.is_dir() {
            if !SKIPPED_DIRECTORIES.contains(&name.as_str()) {
                collect_files(&entry.path(), depth + 1, files, report);
            }
        } else if kind.is_file() && extract::supported(&name) {
            files.push(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(name: &str) -> (Arc<KnowledgeStore>, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "dsh-knowledge-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        (
            KnowledgeStore::open(dir.join("knowledge.sqlite")).unwrap(),
            dir,
        )
    }

    #[test]
    fn documents_are_indexed_searched_and_removed() {
        let (store, dir) = store("search");
        let base = store.create_base("产品手册", "内部资料").unwrap();
        assert_eq!(
            store.create_base("产品手册", "").unwrap_err().code,
            "duplicate_name"
        );
        let manual = "定时任务会在到点后把任务发送到原会话。\n\n知识库支持 Markdown、DOCX 和 PDF 文档。\n\nDeploy with cargo build --release.";
        let doc = store
            .add_document(&base.id, "说明.md", None, manual.as_bytes())
            .unwrap();
        assert_eq!(doc.chunk_count, 1);
        assert_eq!(
            store
                .add_document(&base.id, "副本.md", None, manual.as_bytes())
                .unwrap_err()
                .code,
            "duplicate"
        );

        let hits = store.search("定时任务怎么发送", None, 5).unwrap();
        assert_eq!(hits.len(), 1, "any-term fallback finds the passage");
        assert_eq!(hits[0].document_name, "说明.md");
        assert!(
            store.search("知识库 PDF", None, 5).unwrap()[0]
                .text
                .contains("PDF")
        );
        assert!(!store.search("cargo release", None, 5).unwrap().is_empty());
        assert!(store.search("完全无关的词语", None, 5).unwrap().is_empty());
        assert!(
            store.search("说明", None, 5).unwrap().len() == 1,
            "document names are searchable"
        );

        let listed = store.bases().unwrap();
        assert_eq!((listed[0].document_count, listed[0].chunk_count), (1, 1));
        store
            .update_base(&base.id, None, None, Some(false))
            .unwrap();
        assert!(
            store.search("定时任务", None, 5).unwrap().is_empty(),
            "disabled bases are not searched"
        );
        store
            .update_base(&base.id, Some("手册"), None, Some(true))
            .unwrap();
        assert_eq!(store.find_base("手册").unwrap().id, base.id);

        store.delete_document(&doc.id).unwrap();
        assert!(store.search("定时任务", None, 5).unwrap().is_empty());
        assert_eq!(
            store.delete_document(&doc.id).unwrap_err().code,
            "not_found"
        );
        store.delete_base(&base.id).unwrap();
        assert!(store.bases().unwrap().is_empty());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn searches_can_be_limited_to_bases_and_long_texts_split() {
        let (store, dir) = store("scope");
        let a = store.create_base("A", "").unwrap();
        let b = store.create_base("B", "").unwrap();
        store
            .add_document(&a.id, "a.txt", None, "苹果 apple".as_bytes())
            .unwrap();
        store
            .add_document(&b.id, "b.txt", None, "苹果 banana".as_bytes())
            .unwrap();
        assert_eq!(store.search("苹果", None, 5).unwrap().len(), 2);
        let only_b = store.search("苹果", Some(&[b.id.clone()]), 5).unwrap();
        assert_eq!(only_b.len(), 1);
        assert_eq!(only_b[0].base_name, "B");
        let long = format!(
            "{}\n\n目标段落包含独特词汇天青色。\n\n{}",
            "填充内容。".repeat(300),
            "结尾内容。".repeat(300)
        );
        let doc = store
            .add_document(&a.id, "long.txt", None, long.as_bytes())
            .unwrap();
        assert!(doc.chunk_count > 3);
        let hit = &store.search("天青色", None, 5).unwrap()[0];
        assert!(hit.snippet.contains("天青色") && hit.snippet.chars().count() <= 162);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn folder_imports_skip_hidden_build_and_unsupported_files() {
        let (store, dir) = store("import");
        let base = store.create_base("项目", "").unwrap();
        let root = dir.join("source");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("README.md"), "# 项目说明").unwrap();
        std::fs::write(root.join("docs/guide.txt"), "使用指南").unwrap();
        std::fs::write(root.join("docs/empty.txt"), "   ").unwrap();
        std::fs::write(root.join("logo.png"), [0x89, 0x50]).unwrap();
        std::fs::write(root.join("node_modules/pkg/index.js"), "x").unwrap();
        std::fs::write(root.join(".git/config"), "x").unwrap();
        let report = store.import_path(&base.id, &root).unwrap();
        let mut added: Vec<_> = report.added.iter().map(|doc| doc.name.clone()).collect();
        added.sort();
        assert_eq!(added, ["README.md", "guide.txt"]);
        assert_eq!(
            report.skipped.len(),
            1,
            "empty text is reported, unsupported files are not collected"
        );
        assert!(!report.truncated);
        let single = store
            .import_path(&base.id, &root.join("docs/guide.txt"))
            .unwrap();
        assert_eq!(single.skipped[0].reason.contains("相同内容"), true);
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn reads_never_create_the_file_and_nothing_stays_open() {
        let (store, dir) = store("handles");
        assert!(store.bases().unwrap().is_empty());
        assert!(store.search("任何", None, 5).unwrap().is_empty());
        assert_eq!(store.base("kb_x").unwrap_err().code, "not_found");
        assert!(!store.exists(), "reads leave no database behind");
        let base = store.create_base("手册", "").unwrap();
        store
            .add_document(&base.id, "a.md", None, "内容".as_bytes())
            .unwrap();
        // Between operations the file is closed, so it can be removed while
        // the store is alive (Windows refuses to delete open files).
        std::fs::remove_file(store.path()).unwrap();
        assert!(store.bases().unwrap().is_empty());
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        let stamp = now();
        assert_eq!(stamp.len(), 24);
        assert!(stamp.ends_with('Z') && stamp.as_bytes()[10] == b'T');
    }
}
