//! Local knowledge bases: users import documents (text, Markdown, code,
//! Office files, PDF) into named bases; their text is chunked and indexed in
//! SQLite FTS5 with CJK bigram segmentation, and the model searches enabled
//! bases through `knowledge_search`.

pub mod extract;
pub mod store;
pub mod text;
pub mod tools;

pub use store::{
    BaseView, DocumentView, ImportReport, KnowledgeError, KnowledgeStore, SearchHit, SkippedFile,
};
pub use tools::{LIST_TOOL, SEARCH_TOOL, context_note, register};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_schemas_stay_inside_the_supported_subset() {
        dsh_tools::assert_supported_json_schema(&tools::search_parameters()).unwrap();
        dsh_tools::assert_supported_json_schema(&tools::list_parameters()).unwrap();
    }

    #[test]
    fn tools_search_list_and_describe_enabled_bases() {
        let dir = std::env::temp_dir().join(format!(
            "dsh-knowledge-tools-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let store = KnowledgeStore::open(dir.join("knowledge.sqlite")).unwrap();
        assert_eq!(context_note(&store), "", "no bases, no note");
        let base = store.create_base("运维{{手册}}", "部署与回滚").unwrap();
        assert_eq!(context_note(&store), "", "empty bases are not advertised");
        store
            .add_document(
                &base.id,
                "回滚.md",
                None,
                "回滚步骤：先停止服务，再恢复上一版本。".as_bytes(),
            )
            .unwrap();
        let note = context_note(&store);
        assert!(
            note.contains("- 运维手册 (1 documents): 部署与回滚"),
            "{note}"
        );
        assert!(note.contains(SEARCH_TOOL));

        let found = tools::run_search(
            &store,
            &serde_json::json!({"query": "怎么回滚", "base": "运维{{手册}}"}),
        )
        .unwrap();
        assert_eq!(found["results"][0]["document"], "回滚.md");
        let missing = tools::run_search(&store, &serde_json::json!({"query": "数据库"})).unwrap();
        assert!(missing["results"].as_array().unwrap().is_empty() && missing["note"].is_string());
        let listed = tools::run_list(&store, &serde_json::json!({"base": base.id})).unwrap();
        assert_eq!(listed["documents"][0]["name"], "回滚.md");
        assert_eq!(listed["knowledgeBases"][0]["documentCount"], 1);

        store
            .update_base(&base.id, None, None, Some(false))
            .unwrap();
        assert_eq!(context_note(&store), "");
        let disabled = tools::run_search(
            &store,
            &serde_json::json!({"query": "回滚", "base": base.id}),
        )
        .unwrap_err();
        assert_eq!(disabled.code, "disabled");
        drop(store);
        let _ = std::fs::remove_dir_all(dir);
    }
}
