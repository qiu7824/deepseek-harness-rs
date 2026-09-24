use dsh_remote_execution::{Connection, RemoteRuntime, protocol::Execution};
use serde_json::json;
use std::{sync::Arc, time::Duration};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires an isolated SSH server and explicitly authorized helper fixture"]
async fn real_ssh_disconnect_recovery_permissions_and_confirmed_cancellation() {
    let config: Connection =
        serde_json::from_str(&std::env::var("DSH_REMOTE_TEST_CONNECTION").unwrap()).unwrap();
    let root = std::path::PathBuf::from(std::env::var("DSH_REMOTE_TEST_STATE").unwrap());
    let python = std::env::var("DSH_REMOTE_TEST_PYTHON").unwrap();
    let ctx = cordis::Context::root();
    let local = dsh_subprocess_local::LocalSubprocessRuntime::install(&ctx);
    let remote = RemoteRuntime::install(&ctx, root, local);
    let row = remote.connect(config.clone()).await.unwrap();
    let denied = remote
        .call(
            &config.id,
            "file",
            json!({"op":"write","path":"denied.txt","content":"no"}),
            "read-only",
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(denied.contains("read-only"), "{denied}");
    assert!(
        remote
            .call(
                &config.id,
                "file",
                json!({"op":"read","path":"../outside.txt"}),
                "read-only",
                None,
                None
            )
            .await
            .is_err()
    );
    let execution=Execution {execution_id:String::new(),context_id:row.handshake.context_id.clone(),workspace:row.handshake.workspace.clone(),permission_mode:"danger-full-access".into(),argv:vec![python.clone(),"-c".into(),"from pathlib import Path; p=Path('effect.txt'); p.write_text(p.read_text()+'x' if p.exists() else 'x'); print('SSH_EXECUTED')".into()],cwd:row.handshake.workspace.clone(),timeout_ms:10000,env:vec![],stdin:None};
    // The fixture drops the first execute response only after delivering it.
    let unknown = remote
        .submit(&config.id, execution.clone(), None)
        .await
        .unwrap_err();
    assert!(unknown.contains("REMOTE_UNKNOWN"), "{unknown}");
    let recovered = remote
        .submit(&config.id, execution.clone(), None)
        .await
        .unwrap();
    assert!(unknown.contains(&recovered.0));
    let wait = |id: String, cancel: bool| {
        let remote = remote.clone();
        let connection = config.id.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(20), async move {
                loop {
                    let value = remote
                        .query(&connection, &id, "read-only", 0, 0, cancel)
                        .await
                        .unwrap();
                    if ["completed", "failed", "cancelled", "timed_out"]
                        .contains(&value["state"].as_str().unwrap_or(""))
                    {
                        break value;
                    }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            })
            .await
            .unwrap()
        }
    };
    let result = wait(recovered.0.clone(), false).await;
    assert_eq!(result["state"], "completed");
    assert_eq!(result["exitCode"], 0);
    let read = remote
        .call(
            &config.id,
            "file",
            json!({"op":"read","path":"effect.txt"}),
            "read-only",
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(read["base64"], "eA==");
    let fresh = remote
        .submit(&config.id, execution.clone(), None)
        .await
        .unwrap();
    assert_ne!(fresh.0, recovered.0);
    assert_eq!(wait(fresh.0, false).await["exitCode"], 0);
    let read = remote
        .call(
            &config.id,
            "file",
            json!({"op":"read","path":"effect.txt"}),
            "read-only",
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(read["base64"], "eHg=");
    let mut long = execution;
    long.argv=vec![python,"-c".into(),"import time; from pathlib import Path; time.sleep(30); Path('late.txt').write_text('must not appear')".into()];
    long.timeout_ms = 60000;
    let running = remote.submit(&config.id, long.clone(), None).await.unwrap();
    let cancelled = wait(running.0, true).await;
    assert_eq!(cancelled["state"], "cancelled");
    assert!(
        remote
            .call(
                &config.id,
                "file",
                json!({"op":"stat","path":"late.txt"}),
                "read-only",
                None,
                None
            )
            .await
            .unwrap()
            .is_null()
    );
    assert!(
        remote
            .submit(&config.id, long, Some(Arc::new(|| true)))
            .await
            .unwrap_err()
            .contains("REMOTE_NOT_STARTED")
    );
    println!(
        "SSH_ACCEPTANCE_OK: unknown dispatch recovered once, subsequent command is fresh, readonly and cancellation enforced"
    );
}
