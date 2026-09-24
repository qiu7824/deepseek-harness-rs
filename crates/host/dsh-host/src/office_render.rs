//! WPS export and native PDF page rendering on authorized immutable inputs.
use dsh_attachment::{AttachmentStore, ImageMediaType};
use dsh_tools::{ToolBodyError, ToolDefinition, ToolOutputDefinition, ToolRuntime};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub(crate) fn install(
    ctx: &cordis::Context,
    resources: Arc<super::workspace_resources::Resources>,
) -> Result<(), String> {
    let tools = ctx
        .get_typed::<Arc<ToolRuntime>>("tools", false)
        .ok_or("Office tools unavailable")?;
    let context = ctx.clone();
    let office = super::office_preview::shared();
    tools.register(ctx,ToolDefinition{
  name:"office_render".into(),
  description:"Export DOCX/XLSX/PPTX through the installed WPS host automation bridge and render actual PDF pages to image attachments. Also renders an existing PDF. Input must be inside the current workspace or an immutable file attached to this session; original files are never edited. A workspace is required for rendered output resources. Does not require Python, LibreOffice or ffmpeg and does not launch WPS through the shell sandbox. Optional pages are one-based, up to 12; default first page. Inspect returned page images (or consult_model vision) and render remaining pages before claiming visual acceptance. Returns an owned candidate PDF resource; use workspace_scratch inspect/promote after validation to deliver it. An export or XML check alone is not visual verification.".into(),
  parameters:json!({"type":"object","properties":{"path":{"type":"string","minLength":1},"pages":{"type":"array","minItems":1,"maxItems":12,"items":{"type":"integer","minimum":1}}},"required":["path"],"additionalProperties":false}),
  output:ToolOutputDefinition{schema:json!({"type":"object"}),render:Arc::new(|_,v|{let mut parts=vec![dsh_llm::ContentBlock::Text{text:v.to_string()}];for page in v["pages"].as_array().into_iter().flatten(){parts.push(dsh_llm::ContentBlock::Text{text:format!("Rendered document page {} of {}",page["page"],v["pageCount"])});parts.push(dsh_llm::ContentBlock::Image{attachment:serde_json::from_value(page["attachment"].clone()).map_err(|e|e.to_string())?,
offloaded: None,
});}Ok(parts)}),presentation_meta:None},
  timeout_ms:Some(240000),is_concurrency_safe:Some(Arc::new(|_|false)),finalize_content:None,present_call:None,present_result:None,
  execute:Arc::new(move |args,run|{let args=args.clone();let context=context.clone();let resources=resources.clone();let office=office.clone();let execution=run.execution.clone();Box::pin(async move{render(&context,&resources,&office,&args,&execution).await.map_err(|e|ToolBodyError::coded(e,"OfficeRenderError","OFFICE_RENDER_FAILED"))})})
 })?;
    Ok(())
}
async fn source_path(root: &str, raw: &str) -> Result<PathBuf, String> {
    let root = tokio::fs::canonicalize(root)
        .await
        .map_err(|e| e.to_string())?;
    let raw = Path::new(raw);
    let path = tokio::fs::canonicalize(if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        root.join(raw)
    })
    .await
    .map_err(|e| e.to_string())?;
    if !path.starts_with(root) {
        return Err(
            "Document must be inside the workspace; attach or copy external inputs first".into(),
        );
    }
    dsh_workspace_resources::checked_path(&path)?;
    Ok(path)
}
async fn render(
    ctx: &cordis::Context,
    resources: &super::workspace_resources::Resources,
    office: &super::office_preview::OfficePreview,
    args: &Value,
    execution: &dsh_tools::ToolExecution,
) -> Result<Value, String> {
    let agent = execution
        .agent
        .as_ref()
        .ok_or("Document rendering requires a session")?;
    let workspace = agent
        .session()
        .header()
        .cwd
        .as_deref()
        .ok_or("Document rendering requires a workspace")?;
    let pages = args.get("pages").cloned().unwrap_or(json!([1]));
    let requested = pages
        .as_array()
        .filter(|p| !p.is_empty() && p.len() <= 12)
        .ok_or("pages requires 1–12 page numbers")?;
    let mut unique = std::collections::BTreeSet::new();
    for page in requested {
        let page = page
            .as_u64()
            .filter(|v| *v > 0 && *v <= u32::MAX as u64)
            .ok_or("pages requires positive page numbers")?;
        if !unique.insert(page) {
            return Err("Duplicate page number".into());
        }
    }
    let signal = execution.signal.lock().clone();
    if signal() {
        return Err("Document rendering cancelled".into());
    }
    let raw = args["path"].as_str().ok_or("path is required")?;
    let store = ctx
        .get_typed::<Arc<dyn AttachmentStore>>("attachments", false)
        .map(|service| service.as_ref().clone());
    let reference = store.as_ref().and_then(|store| {
        agent.session().with_events(|events| {
            events
                .iter()
                .flat_map(|event| {
                    dsh_attachment::file_references_for_event(&event.type_, &event.data)
                })
                .find(|reference| {
                    store.file_host_path(reference).as_deref() == Some(Path::new(raw))
                })
        })
    });
    let attachment_lease = match (&store, &reference) {
        (Some(store), Some(reference)) => Some(
            store
                .open_file(reference, Some(&signal))
                .await
                .map_err(|error| error.to_string())?,
        ),
        _ => None,
    };
    let path = if attachment_lease.is_some() {
        PathBuf::from(raw)
    } else {
        source_path(workspace, raw).await?
    };
    let ext = path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let (identity, pdf) = if ext == "pdf" {
        super::office_preview::snapshot_pdf(&path).await?
    } else {
        office.export(&path).await?
    };
    drop(attachment_lease);
    if signal() {
        return Err("Document rendering cancelled".into());
    }
    let store = resources.current_for(workspace)?;
    let mut lease = store.allocate(
        agent.id().as_str(),
        workspace,
        "candidate",
        "Document rendering",
    )?;
    let root = lease.path();
    let pdf_path = root.join("document.pdf");
    tokio::fs::copy(&pdf.path, &pdf_path)
        .await
        .map_err(|e| e.to_string())?;
    let result = render_pages(&pdf_path, &root, &pages, signal.clone()).await?;
    let attachments = ctx
        .get_typed::<Arc<dyn AttachmentStore>>("attachments", false)
        .ok_or("Image attachments unavailable")?;
    let mut images = Vec::new();
    for item in result["pages"]
        .as_array()
        .ok_or("PDF renderer returned no pages")?
    {
        if signal() {
            return Err("Document rendering cancelled".into());
        }
        let page = item["page"].as_u64().ok_or("Invalid rendered page")?;
        let file = root.join(format!("page-{page:04}.png"));
        let file = tokio::fs::File::open(&file)
            .await
            .map_err(|e| e.to_string())?;
        let image = attachments
            .save_image_stream(
                Box::pin(file),
                ImageMediaType::Png,
                Some(format!("document-page-{page}.png")),
                Some(&signal),
            )
            .await
            .map_err(|e| e.to_string())?;
        images.push(json!({"page":page,"attachment":image}));
    }
    let id = lease.id().to_string();
    lease.finish(true)?;
    store.retain(&id, false, true)?;
    Ok(
        json!({"sourceSha256":identity,"exporter":if ext=="pdf"{"original-pdf"}else{"WPS"},"renderer":"Windows.Data.Pdf","executionWorld":"host-document-bridge","pageCount":result["pageCount"],"pages":images,"allPagesRendered":result["pageCount"].as_u64()==Some(images.len() as u64),"visualReviewComplete":false,"pdf":{"resourceId":id,"path":pdf_path,"relativePath":"document.pdf"},"verification":"Images are actual rendered pages; inspect them before claiming layout acceptance. Structural parsing alone is not visual verification."}),
    )
}
#[cfg(windows)]
async fn render_pages(
    pdf: &Path,
    directory: &Path,
    pages: &Value,
    signal: dsh_tools::AbortPredicate,
) -> Result<Value, String> {
    let script = directory.join("render-pages.ps1");
    tokio::fs::write(&script, include_bytes!("pdf_pages.ps1"))
        .await
        .map_err(|e| e.to_string())?;
    let shell =
        PathBuf::from(std::env::var_os("SystemRoot").ok_or("Windows directory unavailable")?)
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut cmd = tokio::process::Command::new(shell);
    cmd.args([
        "-NoLogo",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ])
    .arg(&script)
    .env(
        "DSH_PDF_INPUT",
        crate::display_workspace_path(&pdf.to_string_lossy()),
    )
    .env(
        "DSH_PDF_DIRECTORY",
        crate::display_workspace_path(&directory.to_string_lossy()),
    )
    .env("DSH_PDF_PAGES", pages.to_string())
    .creation_flags(0x08000000)
    .kill_on_drop(true);
    let output = tokio::select! {
     value=tokio::time::timeout(Duration::from_secs(90),cmd.output())=>value.map_err(|_|"PDF rendering exceeded 90 seconds")?.map_err(|e|e.to_string())?,
     _=async{while !signal(){tokio::time::sleep(Duration::from_millis(50)).await}}=>return Err("PDF rendering cancelled".into())
    };
    if !output.status.success() {
        return Err(format!(
            "PDF rendering failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1200)
                .collect::<String>()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("PDF page metadata invalid: {e}"))
}
#[cfg(not(windows))]
async fn render_pages(
    _: &Path,
    _: &Path,
    _: &Value,
    _: dsh_tools::AbortPredicate,
) -> Result<Value, String> {
    Err("Native document rendering is currently available on Windows".into())
}
