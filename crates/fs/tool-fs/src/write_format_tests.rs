use super::*;
use serde_json::json;

#[tokio::test]
async fn text_write_cannot_publish_html_disguised_as_an_office_document() {
    let root = std::env::temp_dir().join(format!("write-format-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let ctx = Context::root();
    SystemPrompt::install(&ctx, Default::default()).unwrap();
    let tools = dsh_tools::ToolRuntime::install(&ctx, Default::default()).unwrap();
    dsh_fs_local::LocalFileSystem::install(
        &ctx,
        dsh_fs_local::Config {
            cwd: Some(root.to_string_lossy().into()),
            diff_basis_max_bytes: None,
        },
    )
    .unwrap();
    Service::install(&ctx).unwrap();
    for name in ["人员(含职务).doc", "预览.docx", "电话.XLSX", "report.pdf"] {
        let result=tools.execute(dsh_tools::ToolExecutionInput{call_id:dsh_llm::call_id(uuid::Uuid::new_v4().to_string()),root_call_id:None,name:"write".into(),arguments:json!({"file_path":name,"content":"<html><table><tr><td>00138</td></tr></table></html>"}),agent:None,parent:None,signal:Arc::new(||false)}).await;
        assert!(result.is_error);
        assert_eq!(
            result.error.as_ref().unwrap().info.as_ref().unwrap().code,
            "BINARY_FORMAT_REQUIRED"
        );
        assert_eq!(
            result.meta.as_ref().unwrap()["executionReceipt"]["effects"],
            "none"
        );
        assert!(!root.join(name).exists());
    }
    let result=tools.execute(dsh_tools::ToolExecutionInput{call_id:dsh_llm::call_id("html-is-text"),root_call_id:None,name:"write".into(),arguments:json!({"file_path":"preview.html","content":"<table><tr><td>00138</td></tr></table>"}),agent:None,parent:None,signal:Arc::new(||false)}).await;
    assert!(!result.is_error, "{:?}", result.error);
    assert!(root.join("preview.html").exists());
    std::fs::remove_dir_all(root).unwrap();
}
