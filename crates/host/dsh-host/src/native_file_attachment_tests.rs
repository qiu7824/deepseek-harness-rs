//! Explicit Windows integration over admitted session files and native helpers.
use dsh_attachment::AttachmentStore;
use dsh_sandbox::{ConfinedSandboxMode, SandboxMode, SandboxPolicy, SandboxProvider};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

fn docx(index: usize) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in [
        ("[Content_Types].xml", r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#.to_owned()),
        ("_rels/.rels", r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="document" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.to_owned()),
        ("word/document.xml", format!(r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Synthetic attachment {index}</w:t></w:r></w:p></w:body></w:document>"#)),
    ] {
        archive.start_file(name,zip::write::SimpleFileOptions::default()).unwrap();
        archive.write_all(content.as_bytes()).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

async fn execute(
    provider: &dyn SandboxProvider,
    policy: &dsh_sandbox::SandboxExecutionPolicy,
    python: &Path,
    code: &str,
    extra: Vec<String>,
) -> String {
    provider.prepare(policy).await.unwrap();
    let mut argv = vec![
        python.to_string_lossy().into_owned(),
        "-c".into(),
        code.into(),
    ];
    argv.extend(extra);
    let confined = provider
        .confine(
            &argv,
            &SandboxPolicy {
                mode: ConfinedSandboxMode::WorkspaceWrite,
                workspace_root: policy.workspace_root.clone(),
                read_only_roots: policy.read_only_roots.clone(),
                session_id: policy.session_id.clone(),
            },
        )
        .unwrap();
    let output = tokio::time::timeout(
        Duration::from_secs(120),
        tokio::process::Command::new(&confined.argv[0])
            .args(&confined.argv[1..])
            .current_dir(&policy.workspace_root)
            .creation_flags(0x08000000)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .expect("native helper deadline")
    .unwrap();
    assert!(
        output.status.success(),
        "native attachment execution: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

async fn admit(
    store: &dyn AttachmentStore,
    session: &dsh_session::Session,
    index: usize,
) -> PathBuf {
    let reference = store
        .save_file_stream(
            Box::pin(Cursor::new(docx(index))),
            format!("附件 {index}.docx"),
            None,
        )
        .await
        .unwrap();
    let path = store.file_host_path(&reference).unwrap();
    session.append("user/message",json!({"id":format!("file-input-{index}"),"role":"user","source":{"kind":"user"},"content":[{"type":"file","attachment":reference}]}),Some(dsh_session::SurfaceIntent{surface_op:dsh_session::SurfaceOp::Append,source_event_seqs:None})).unwrap();
    path
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires explicitly provisioned native helpers, sandbox state and Python runtime"]
async fn admitted_docx_and_forty_attachments_keep_python_launch_and_exact_read_scope() {
    let binary =
        PathBuf::from(std::env::var_os("DSH_NATIVE_ACCEPTANCE_BINARY").expect("native helper"));
    let state = std::env::var("DSH_NATIVE_ACCEPTANCE_HOME").expect("isolated native pool");
    let workspace = std::env::var("DSH_NATIVE_ACCEPTANCE_WORKSPACE").expect("owned workspace");
    let python = PathBuf::from(
        std::env::var_os("DSH_NATIVE_ACCEPTANCE_PYTHON").expect("actual Python executable"),
    );
    let root = PathBuf::from(
        std::env::var_os("DSH_NATIVE_ATTACHMENT_FIXTURE_ROOT")
            .expect("synthetic fixture directory"),
    )
    .join(format!("native-session-files-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    let hash = |p: &Path| format!("{:x}", Sha256::digest(std::fs::read(p).unwrap()));
    let helpers = binary.parent().unwrap();
    let config = json!({"version":1,"backend":"windows-native","runner":binary,"stateDirectory":state,"sha256":hash(&binary),"commandRunnerSha256":hash(&helpers.join("dsh-command-runner.exe")),"setupSha256":hash(&helpers.join("dsh-windows-sandbox-setup.exe")),"workspaces":[workspace]});
    std::fs::write(
        home.join("windows-sandbox.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let ctx = cordis::Context::root();
    let files = dsh_attachment_local::LocalAttachmentStore::install(
        &ctx,
        dsh_attachment_local::Config {
            dsh_home: Some(home.to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    let sessions = dsh_session::SessionStore::install(&ctx);
    let metadata = Some(dsh_session::CreateSessionOptions {
        meta: Some(dsh_session::CreateSessionMeta {
            cwd: Some(workspace.clone()),
            ..Default::default()
        }),
        ..Default::default()
    });
    let owner = sessions
        .create(
            &ctx,
            Some(dsh_session::session_id("native-files-owner")),
            metadata.clone(),
        )
        .await
        .unwrap();
    let other = sessions
        .create(
            &ctx,
            Some(dsh_session::session_id("native-files-other")),
            metadata,
        )
        .await
        .unwrap();
    let policy = dsh_sandbox_policy::SandboxPolicyService::install(
        &ctx,
        dsh_sandbox_policy::Config {
            mode: Some(SandboxMode::WorkspaceWrite),
            workspace_root: Some(workspace.clone()),
        },
    );
    let provider = dsh_sandbox_local::LocalSandboxProvider::install_with_runtimes_at_home(
        &ctx,
        Default::default(),
        vec![python.parent().unwrap().to_path_buf()],
        root.join("cache"),
        home.clone(),
    );
    let protected = home.clone();
    let _private = dsh_sandbox::roots::register_private_roots(Arc::new(move || {
        vec![protected.to_string_lossy().into_owned()]
    }));
    let foreign = admit(files.as_ref(), &other, 999).await;
    let mut own = vec![];
    let mut reports = vec![];
    const READ_CHECK: &str = r#"import json,sys,pathlib,zipfile
paths=json.loads(sys.argv[1]); foreign=pathlib.Path(sys.argv[2]); denied=0
for value in paths:
 p=pathlib.Path(value)
 with zipfile.ZipFile(p) as z: assert b'Synthetic attachment' in z.read('word/document.xml')
 try:
  with p.open('r+b'): pass
 except PermissionError: denied+=1
try:
 with foreign.open('rb') as f: f.read(1)
 foreign_denied=False
except PermissionError: foreign_denied=True
assert denied==len(paths), (denied,len(paths))
assert foreign_denied
print(json.dumps({'readCount':len(paths),'writesDenied':denied,'foreignDenied':foreign_denied}))
"#;
    for count in [1, 40] {
        while own.len() < count {
            own.push(admit(files.as_ref(), &owner, own.len()).await);
        }
        let selected = policy
            .try_resolve(&dsh_sandbox_policy::SandboxPolicyRequest {
                session: Some(Arc::new(owner.clone())),
                mode: None,
            })
            .unwrap();
        assert_eq!(selected.read_only_roots.len(), count);
        for path in &own {
            assert!(
                selected
                    .read_only_roots
                    .iter()
                    .any(|root| Path::new(root) == path)
            );
        }
        assert!(
            selected
                .read_only_roots
                .iter()
                .all(|root| Path::new(root).is_file())
        );
        let output = execute(provider.as_ref(), &selected, &python, "print(1)", vec![]).await;
        assert_eq!(output.trim(), "1");
        let checked = execute(
            provider.as_ref(),
            &selected,
            &python,
            READ_CHECK,
            vec![
                serde_json::to_string(&own).unwrap(),
                foreign.to_string_lossy().into_owned(),
            ],
        )
        .await;
        let report: Value = serde_json::from_str(checked.trim()).unwrap();
        assert_eq!(report["readCount"], count);
        reports.push(report);
    }
    let selected = policy
        .try_resolve(&dsh_sandbox_policy::SandboxPolicyRequest {
            session: Some(Arc::new(other.clone())),
            mode: None,
        })
        .unwrap();
    assert_eq!(selected.read_only_roots.len(), 1);
    let checked = execute(
        provider.as_ref(),
        &selected,
        &python,
        READ_CHECK,
        vec![
            serde_json::to_string(&vec![foreign]).unwrap(),
            own[0].to_string_lossy().into_owned(),
        ],
    )
    .await;
    let report: Value = serde_json::from_str(checked.trim()).unwrap();
    assert_eq!(report["readCount"], 1);
    reports.push(report);
    let report = json!({"passed":true,"python":python,"helper":binary,"workspace":workspace,"singleAndFortyAttachmentLaunches":true,"cases":reports});
    std::fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("{}", json!({"fixture":root,"result":report}));
}
