use super::*;
use base64::Engine;
use cordis::Plugin;
use dsh_agent::{AgentFactory, CreateAgentOptions};
use serde_json::{Value, json};

async fn call(
    host: &crate::HostSpine,
    agent: &Arc<dyn dsh_agent::Agent>,
    name: &str,
    arguments: Value,
) -> Arc<dsh_tools::ToolExecutionResult> {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        host.tools.execute(dsh_tools::ToolExecutionInput {
            call_id: dsh_llm::call_id(uuid::Uuid::new_v4().to_string()),
            root_call_id: None,
            name: name.into(),
            arguments,
            agent: Some(agent.clone()),
            parent: None,
            signal: Arc::new(|| false),
        }),
    )
    .await
    .expect("tool must settle without waiting for approval")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_private_boundary_real_tools_deny_secrets_other_sessions_and_allow_owned_files() {
    let root = std::env::temp_dir().join(format!("host-private-boundary-{}", uuid::Uuid::new_v4()));
    let home = root.join("custom-home");
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let ctx = cordis::Context::root();
    let host = crate::compose_persistent_host_at(&ctx, &home, None).unwrap();
    dsh_tool_fs::ToolFsPlugin
        .apply(&ctx, cordis::arc(json!({})))
        .await
        .unwrap();
    dsh_tool_fs_search::ToolFsSearchPlugin
        .apply(&ctx, cordis::arc(json!({"sampleOverCapGlobResults":false})))
        .await
        .unwrap();
    dsh_tool_str_replace_editor::apply(&ctx, Default::default()).unwrap();
    let make = |id: &str| CreateAgentOptions {
        session_id: Some(dsh_session::session_id(id)),
        meta: Some(dsh_session::CreateSessionMeta {
            cwd: Some(project.to_string_lossy().into_owned()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let a = host
        .agent_loop
        .create_agent(&ctx, make("boundary-a"))
        .await
        .unwrap()
        .agent;
    let b = host
        .agent_loop
        .create_agent(&ctx, make("boundary-b"))
        .await
        .unwrap()
        .agent;
    dsh_sandbox_policy::set_sandbox_mode(a.session(), dsh_sandbox::SandboxMode::DangerFullAccess)
        .unwrap();
    for agent in [&a, &b] {
        dsh_user_approval::set_approval_policy(
            agent.session(),
            dsh_user_approval::ApprovalPolicy::Never,
        )
        .unwrap();
    }
    let credential = home.join(dsh_credentials_local::CREDENTIALS_FILENAME);
    std::fs::write(&credential, "BOUNDARY_PRIVATE_SENTINEL").unwrap();
    let object = home.join("attachments/objects/foreign.txt");
    std::fs::create_dir_all(object.parent().unwrap()).unwrap();
    std::fs::write(&object, "BOUNDARY_PRIVATE_SENTINEL").unwrap();
    let mut paths = Vec::new();
    for agent in [&a, &b] {
        let allocated = call(
            &host,
            agent,
            "workspace_scratch",
            json!({"action":"allocate","kind":"script","label":"boundary fixture"}),
        )
        .await;
        assert!(!allocated.is_error, "{:?}", allocated.error);
        let allocated = allocated.value.clone().unwrap();
        let write = call(&host, agent, "workspace_scratch", json!({"action":"write","id":allocated["id"],"path":"proof.txt","content":"OWNED_BOUNDARY_PROOF"})).await;
        assert!(!write.is_error, "{:?}", write.error);
        paths.push(PathBuf::from(allocated["path"].as_str().unwrap()).join("proof.txt"));
    }
    for forbidden in [&credential, &object, &paths[1]] {
        for (tool, args) in [
            ("read", json!({"file_path":forbidden})),
            ("read_image", json!({"file_path":forbidden})),
            (
                "glob",
                json!({"path":forbidden,"pattern":"*","include_ignored":true}),
            ),
            ("grep", json!({"path":forbidden,"pattern":"."})),
            (
                "str_replace_editor",
                json!({"command":"view","path":forbidden}),
            ),
        ] {
            let result = call(&host, &a, tool, args).await;
            assert!(
                result.is_error,
                "{tool} admitted {forbidden:?}: {:?}",
                result.value
            );
            assert!(
                format!("{:?}", result.error).contains("FS_SANDBOX_DENIED"),
                "{tool}: {:?}",
                result.error
            );
            assert!(
                !serde_json::to_string(&result.value)
                    .unwrap()
                    .contains("BOUNDARY_PRIVATE_SENTINEL")
            );
        }
    }
    for (tool, args) in [
        ("read", json!({"file_path":paths[0]})),
        ("glob", json!({"path":paths[0].parent(),"pattern":"*.txt"})),
        (
            "grep",
            json!({"path":paths[0],"pattern":"OWNED_BOUNDARY_PROOF"}),
        ),
    ] {
        let result = call(&host, &a, tool, args).await;
        assert!(!result.is_error, "{tool}: {:?}", result.error);
        let value = result.value.as_ref().unwrap();
        match tool {
            "glob" => assert_eq!(value["paths"].as_array().unwrap().len(), 1),
            "grep" => assert_eq!(value["matches"].as_array().unwrap().len(), 1),
            _ => assert!(value.to_string().contains("OWNED_BOUNDARY_PROOF")),
        }
    }
    let recursive = call(
        &host,
        &a,
        "grep",
        json!({"path":root,"pattern":"BOUNDARY_PRIVATE_SENTINEL"}),
    )
    .await;
    assert!(recursive.is_error);
    let video = home.join("foreign.mp4");
    std::fs::write(&video, "synthetic video").unwrap();
    assert!(
        call(&host, &a, "read_video", json!({"path":video}))
            .await
            .is_error
    );
    let fs = ctx
        .get_typed::<Arc<dyn FileSystem>>("fs", false)
        .unwrap()
        .as_ref()
        .clone();
    let trusted = fs
        .resolve(&credential.to_string_lossy(), None)
        .await
        .unwrap();
    assert_eq!(
        fs.read_text(&trusted, None).await.unwrap(),
        "BOUNDARY_PRIVATE_SENTINEL"
    );
    let scoped = fs.for_tool(Some(a.id().as_str())).unwrap();
    assert!(scoped.read_text(&trusted, None).await.is_err());
    let local = project.join("public.txt");
    std::fs::write(&local, "public").unwrap();
    let target = scoped
        .resolve(&local.to_string_lossy(), None)
        .await
        .unwrap();
    assert_eq!(scoped.read_text(&target, None).await.unwrap(), "public");
    // Image IDs must have admitted content, then produce an executable owned copy.
    let attachments = ctx
        .get_typed::<Arc<dyn dsh_attachment::AttachmentStore>>("attachments", false)
        .unwrap()
        .as_ref()
        .clone();
    let image = attachments.save_image(&dsh_attachment::SaveImageAttachment {
        data: base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAABCAYAAAD0In+KAAAAEUlEQVR4nGP8z8Dwn4GBgQEADQUCAOAHawIAAAAASUVORK5CYII=").unwrap(),
        media_type: dsh_attachment::ImageMediaType::Png, name: Some("fixture.png".into()),
    }).await.unwrap();
    a.session()
        .append(
            "tool/call",
            json!({"arguments":{"reference":{"type":"image","attachment":image}}}),
            None,
        )
        .unwrap();
    let image_execution = dsh_tools::ToolExecution {
        schema: None,
        permission_preset: None,
        token: 0,
        call_id: dsh_llm::call_id("image-reference-boundary"),
        root_call_id: dsh_llm::call_id("image-reference-boundary"),
        name: "generate_image".into(),
        arguments: json!({}),
        agent: Some(a.clone()),
        parent: None,
        signal: parking_lot::Mutex::new(Arc::new(|| false)),
    };
    assert!(
        crate::image_generation::read_references(
            &ctx,
            &image_execution,
            &json!([image.attachment_id])
        )
        .await
        .is_err()
    );
    assert!(
        call(
            &host,
            &a,
            "read_image",
            json!({"file_path":image.attachment_id})
        )
        .await
        .is_error
    );
    a.session().append("user/message", json!({"id":"boundary-owned-image","role":"user","source":{"kind":"user"},"content":[{"type":"image","attachment":image}]}), Some(dsh_session::SurfaceIntent {surface_op:dsh_session::SurfaceOp::Append,source_event_seqs:None})).unwrap();
    assert_eq!(
        crate::image_generation::read_references(
            &ctx,
            &image_execution,
            &json!([image.attachment_id])
        )
        .await
        .unwrap()[0]
            .reference,
        image
    );
    drop(image_execution);
    let admitted = call(
        &host,
        &a,
        "read_image",
        json!({"file_path":image.attachment_id}),
    )
    .await;
    assert!(!admitted.is_error, "{:?}", admitted.error);
    let copied = PathBuf::from(
        admitted.value.as_ref().unwrap()["localPath"]
            .as_str()
            .unwrap(),
    );
    assert!(copied.is_file());
    let image_original = attachments.image_host_path(&image).unwrap();
    let admitted_target = scoped
        .resolve(&image_original.to_string_lossy(), None)
        .await
        .unwrap();
    assert!(
        scoped
            .read_bytes(&admitted_target, None, 4096)
            .await
            .is_ok()
    );
    assert!(
        scoped
            .write_text(&admitted_target, "change", None, None, None)
            .await
            .is_err()
    );
    assert!(scoped.authorize_write(&admitted_target).await.is_err());
    // A scratch catalogue can be offline or corrupt while the immutable input
    // store remains available. This must neither revoke admitted inputs nor
    // grant the unverified scratch paths.
    let scratch_index = home.join("scratch/objects");
    let offline_index = home.join("scratch/objects-offline");
    std::fs::rename(&scratch_index, &offline_index).unwrap();
    std::fs::write(&scratch_index, "unavailable scratch catalogue fixture").unwrap();
    assert!(
        scoped
            .read_bytes(&admitted_target, None, 4096)
            .await
            .is_ok()
    );
    assert!(
        scoped
            .resolve(&paths[1].to_string_lossy(), None)
            .await
            .is_err()
    );
    assert!(
        scoped
            .resolve(&paths[0].to_string_lossy(), None)
            .await
            .is_err()
    );
    assert!(
        scoped
            .resolve(&object.to_string_lossy(), None)
            .await
            .is_err()
    );
    std::fs::remove_file(&scratch_index).unwrap();
    std::fs::rename(&offline_index, &scratch_index).unwrap();
    assert!(
        fs.for_tool(Some(b.id().as_str()))
            .unwrap()
            .resolve(&copied.to_string_lossy(), None)
            .await
            .is_err()
    );
    // An alias re-targeted after resolve must be re-authorized at actual I/O.
    let alias = project.join("alias");
    make_directory_alias(&project, &alias);
    let stale = scoped
        .resolve(&alias.join("public.txt").to_string_lossy(), None)
        .await
        .unwrap();
    std::fs::remove_dir(&alias).unwrap();
    let own_data = paths[0].parent().unwrap();
    let saved_data = own_data.with_file_name("saved-data");
    std::fs::rename(own_data, &saved_data).unwrap();
    make_directory_alias(paths[1].parent().unwrap(), own_data);
    assert!(
        scoped
            .resolve(&paths[0].to_string_lossy(), None)
            .await
            .is_err()
    );
    std::fs::remove_dir(own_data).unwrap();
    std::fs::rename(&saved_data, own_data).unwrap();
    make_directory_alias(&home, &alias);
    std::fs::write(home.join("public.txt"), "BOUNDARY_PRIVATE_SENTINEL").unwrap();
    assert!(scoped.read_text(&stale, None).await.is_err());
    // A safe search root can still contain a junction into private storage.
    // Traversal and result filtering must not expose those descendants.
    for (tool, args) in [
        (
            "grep",
            json!({"path":project,"pattern":"BOUNDARY_PRIVATE_SENTINEL"}),
        ),
        (
            "glob",
            json!({"path":project,"pattern":"**/*","include_ignored":true}),
        ),
    ] {
        let result = call(&host, &a, tool, args).await;
        assert!(!format!("{:?}", result.content).contains("BOUNDARY_PRIVATE_SENTINEL"));
        if let Some(value) = &result.value {
            if tool == "grep" {
                assert!(value["matches"].as_array().unwrap().is_empty());
            } else {
                assert!(value["paths"].as_array().unwrap().iter().all(|path| {
                    !path
                        .as_str()
                        .unwrap()
                        .replace('\\', "/")
                        .contains("/alias/")
                }));
            }
        }
    }
    for alias_target in [
        alias.join(dsh_credentials_local::CREDENTIALS_FILENAME),
        credential.canonicalize().unwrap(),
    ] {
        assert!(
            scoped
                .resolve(&alias_target.to_string_lossy(), None)
                .await
                .is_err()
        );
    }
    #[cfg(windows)]
    {
        assert!(
            scoped
                .resolve(&credential.to_string_lossy().to_uppercase(), None)
                .await
                .is_err()
        );
        assert!(
            scoped
                .resolve(&short_path(&credential), None)
                .await
                .is_err()
        );
        assert!(
            scoped
                .resolve(&format!("{}::$DATA", credential.display()), None)
                .await
                .is_err()
        );
    }
    std::fs::remove_dir(&alias).unwrap();
    dsh_sandbox_policy::set_sandbox_mode(a.session(), dsh_sandbox::SandboxMode::ReadOnly).unwrap();
    assert!(
        call(&host, &a, "read", json!({"file_path":credential}))
            .await
            .is_error
    );
    host.shutdown().await.unwrap();
    drop(scoped);
    drop(fs);
    drop(attachments);
    drop(a);
    drop(b);
    drop(host);
    drop(ctx);
    assert!(
        root.canonicalize()
            .unwrap()
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(&root).unwrap();
}

fn make_directory_alias(target: &Path, alias: &Path) {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(alias)
            .arg(target)
            .output()
            .unwrap();
        assert!(output.status.success(), "junction fixture: {:?}", output);
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, alias).unwrap();
}

#[cfg(windows)]
fn short_path(path: &Path) -> String {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetShortPathNameW(path: *const u16, result: *mut u16, size: u32) -> u32;
    }
    let input: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut result = vec![0u16; 32768];
    let count =
        unsafe { GetShortPathNameW(input.as_ptr(), result.as_mut_ptr(), result.len() as u32) };
    assert!(count > 0 && count < result.len() as u32);
    String::from_utf16(&result[..count as usize]).unwrap()
}
