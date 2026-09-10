use super::*;

struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.starts_with(std::env::temp_dir()) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

#[tokio::test]
async fn delivery_requires_existing_regular_files_in_the_owning_workspace() {
    let fixture =
        Fixture(std::env::temp_dir().join(format!("dsh-present-{}", uuid::Uuid::new_v4())));
    let work = fixture.0.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("result.txt"), "result").unwrap();
    std::fs::write(fixture.0.join("outside.txt"), "outside").unwrap();
    std::fs::create_dir(work.join("directory")).unwrap();
    let ctx = Context::root();
    let fs = dsh_fs_local::LocalFileSystem::install(
        &ctx,
        dsh_fs_local::Config {
            cwd: Some(work.to_string_lossy().into_owned()),
            diff_basis_max_bytes: None,
        },
    )
    .unwrap();
    let cwd = work.to_string_lossy();
    let signal: dsh_tools::AbortPredicate = Arc::new(|| false);
    let files = json!([{"path":"result.txt","description":"Primary result"}]);
    assert_eq!(
        existing_files(fs.as_ref(), &cwd, &files, signal.clone())
            .await
            .unwrap(),
        files
    );
    for path in ["missing.txt", "directory", "../outside.txt", ""] {
        assert!(
            existing_files(fs.as_ref(), &cwd, &json!([{"path":path}]), signal.clone())
                .await
                .is_err(),
            "{path}"
        );
    }
    assert!(
        existing_files(fs.as_ref(), &cwd, &json!([]), signal.clone())
            .await
            .is_err()
    );
    assert!(
        existing_files(
            fs.as_ref(),
            &cwd,
            &json!(vec![json!({"path":"result.txt"}); 9]),
            signal
        )
        .await
        .is_err()
    );
    assert!(
        existing_files(fs.as_ref(), &cwd, &files, Arc::new(|| true))
            .await
            .is_err()
    );
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(work.join("result.txt"), work.join("link.txt"));
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(work.join("result.txt"), work.join("link.txt"));
    #[cfg(any(unix, windows))]
    if linked.is_ok() {
        assert!(
            existing_files(
                fs.as_ref(),
                &cwd,
                &json!([{"path":"link.txt"}]),
                Arc::new(|| false)
            )
            .await
            .is_err()
        );
    }
}

#[test]
fn a_closed_or_superseded_turn_cannot_receive_a_delivery() {
    let event = |seq, kind: &str, turn| SessionEvent {
        type_: kind.into(),
        seq: dsh_session::SessionSeq::new(seq).unwrap(),
        time: 0,
        data: json!({"turn":turn}),
        ignorable: None,
        surface_op: None,
        source_event_seqs: None,
    };
    assert_eq!(open_turn(&[]), None);
    assert_eq!(open_turn(&[event(0, "turn/start", 1)]), Some(1));
    assert_eq!(
        open_turn(&[event(0, "turn/start", 1), event(1, "turn/end", 1)]),
        None
    );
    assert_eq!(
        open_turn(&[
            event(0, "turn/start", 1),
            event(1, "turn/end", 1),
            event(2, "turn/start", 2)
        ]),
        Some(2)
    );
}
