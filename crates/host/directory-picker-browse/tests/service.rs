//! Rust port of `packages/host/directory-picker-browse/tests/service.spec.ts`
//! behaviors over a real temporary directory tree.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cordis::Context;
use dsh_host_directory_picker::{
    AbortSignal, DirectoryPicker, DirectoryPickerBrowseCapability, DirectoryPickerCapability,
    DirectoryPickerErrorCode, DirectoryPickerListError,
};
use dsh_host_directory_picker_browse::{
    BrowseDirectoryPicker, Config, ListingCandidate, ancestry_crumbs, bounded_insert,
    create_directory, fully_qualified, home_dir, race_abort, windows_drive_entries,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
#[test]
fn windows_drive_shortcuts_are_sorted_and_omit_the_current_drive() {
    let entries = windows_drive_entries(
        [
            PathBuf::from("Z:\\"),
            PathBuf::from("D:\\"),
            PathBuf::from("d:\\"),
            PathBuf::from("C:\\"),
        ],
        Some(Path::new("C:\\Users\\Administrator")),
    );
    let paths: Vec<_> = entries.into_iter().map(|entry| entry.path).collect();
    assert_eq!(paths, vec!["D:\\", "Z:\\"]);
}

/// One unique temporary directory root per call, removed by the guard.
struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dsh-browse-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).expect("temp root");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run<F: std::future::Future>(future: F) -> F::Output {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(future)
}

/// Windows allows directory symlinks unprivileged but denies file symlinks
/// without the SeCreateSymbolicLinkPrivilege; the file-link row only feeds
/// POSIX coverage and is filtered out anyway.
fn try_symlink_dir(target: &Path, link: &Path) {
    #[cfg(windows)]
    {
        let _ = std::os::windows::fs::symlink_dir(target, link);
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).expect("dir symlink");
    }
}

fn try_symlink_file(target: &Path, link: &Path) {
    #[cfg(windows)]
    {
        let _ = std::os::windows::fs::symlink_file(target, link);
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).expect("file symlink");
    }
}

/// Build the fixture tree the TS beforeAll constructs.
fn fixture() -> TempRoot {
    let root = TempRoot::new();
    std::fs::create_dir(root.path().join("projects")).unwrap();
    std::fs::create_dir(root.path().join("projects").join("harness")).unwrap();
    std::fs::create_dir(root.path().join(".hidden-dir")).unwrap();
    std::fs::write(root.path().join("notes.txt"), "not a directory").unwrap();
    try_symlink_dir(&root.path().join("projects"), &root.path().join("linked"));
    try_symlink_dir(&root.path().join("gone"), &root.path().join("broken"));
    try_symlink_file(
        &root.path().join("notes.txt"),
        &root.path().join("file-link"),
    );
    root
}

fn browse(capability: &DirectoryPickerCapability) -> &DirectoryPickerBrowseCapability {
    let DirectoryPickerCapability::Browse(browse) = capability else {
        panic!("browse backend must advertise the browse capability");
    };
    browse
}

fn install(ctx: &Context, config: Config) -> Arc<BrowseDirectoryPicker> {
    BrowseDirectoryPicker::install(ctx, config)
}

#[test]
fn lists_directories_only_flags_hidden_rows_follows_symlinks_skips_broken_links_sorts_by_name() {
    run(async {
        let root = fixture();
        let ctx = Context::root();
        let backend = install(&ctx, Config::default());
        let capability = browse(&backend.capability()).clone();

        let listing = (capability.list)(
            Some(root.path().to_string_lossy().into_owned()),
            AbortSignal::new(),
        )
        .await
        .expect("list");
        assert_eq!(Path::new(&listing.path), root.path());
        assert_eq!(listing.home, home_dir());
        let names: Vec<&str> = listing.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec![".hidden-dir", "linked", "projects"]);
        let hidden: Vec<bool> = listing.entries.iter().map(|e| e.hidden).collect();
        assert_eq!(hidden, vec![true, false, false]);
        // Every entry path is absolute and host-joined — clients never join
        // segments.
        assert!(
            listing
                .entries
                .iter()
                .all(|entry| Path::new(&entry.path) == root.path().join(&entry.name)),
            "entry paths are host-joined absolutes"
        );
        // Well under the default bound: the complete level, not a cut one.
        assert!(!listing.truncated);
    });
}

#[test]
fn cuts_a_level_at_max_entries_keeping_the_name_sorted_head_and_flags_the_cut() {
    run(async {
        let root = fixture();
        let ctx = Context::root();
        let backend = install(&ctx, Config { max_entries: 1 });
        let capability = browse(&backend.capability()).clone();

        let cut = (capability.list)(
            Some(root.path().to_string_lossy().into_owned()),
            AbortSignal::new(),
        )
        .await
        .expect("list");
        let names: Vec<&str> = cut.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec![".hidden-dir"]);
        assert!(cut.truncated);

        // Exactly at the bound is complete, not truncated.
        let exact = (capability.list)(
            Some(root.path().join("projects").to_string_lossy().into_owned()),
            AbortSignal::new(),
        )
        .await
        .expect("list");
        let names: Vec<&str> = exact.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["harness"]);
        assert!(!exact.truncated);

        // A level that fits the window but exceeds the bound (two rows, bound
        // one): the in-window extra row proves the cut without any eviction.
        std::fs::create_dir(root.path().join("projects").join("harness").join("a")).unwrap();
        std::fs::create_dir(root.path().join("projects").join("harness").join("b")).unwrap();
        let in_window = (capability.list)(
            Some(
                root.path()
                    .join("projects")
                    .join("harness")
                    .to_string_lossy()
                    .into_owned(),
            ),
            AbortSignal::new(),
        )
        .await
        .expect("list");
        let names: Vec<&str> = in_window.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a"]);
        assert!(in_window.truncated);
    });
}

#[test]
fn stops_the_scan_with_the_caller_an_aborted_signal_resolves_as_aborted() {
    run(async {
        let root = fixture();
        let ctx = Context::root();
        let backend = install(&ctx, Config::default());
        let capability = browse(&backend.capability()).clone();

        // The abort surfaces as aborted, not dressed as an unreadable
        // directory — and rejects even before any level row is read.
        let gone = AbortSignal::new();
        gone.abort();
        let outcome = (capability.list)(
            Some(root.path().to_string_lossy().into_owned()),
            gone.clone(),
        )
        .await;
        assert!(matches!(outcome, Err(DirectoryPickerListError::Aborted)));

        // Aborted against a missing target: the abandoned open rejects on
        // its own and there is nothing to close.
        let missing = root.path().join("no-such-dir");
        let outcome = (capability.list)(Some(missing.to_string_lossy().into_owned()), gone).await;
        assert!(matches!(outcome, Err(DirectoryPickerListError::Aborted)));

        // A live signal leaves a normal listing untouched — the reads and
        // the symlink probes race it without ever losing.
        let live = AbortSignal::new();
        let complete = (capability.list)(
            Some(root.path().to_string_lossy().into_owned()),
            live.clone(),
        )
        .await
        .expect("list");
        assert!(!complete.truncated);
        assert!(complete.entries.iter().any(|e| e.name == "linked"));

        // A live signal changes nothing about ordinary failures.
        let failure = (capability.list)(Some(missing.to_string_lossy().into_owned()), live).await;
        match failure {
            Err(DirectoryPickerListError::Unreadable(error)) => {
                assert_eq!(error.code, DirectoryPickerErrorCode::DirectoryUnreadable);
            }
            other => panic!("expected unreadable, got {other:?}"),
        }
    });
}

#[test]
fn race_abort_follows_the_operation_until_the_signal_wins() {
    run(async {
        // No signal / settled operations: plain passthrough.
        let live = AbortSignal::new();
        assert_eq!(race_abort(async { "ok" }, &live).await, Ok("ok"));
        // Failure passthrough keeps the operation's own error.
        assert_eq!(
            race_abort(async { Err::<(), &str>("raw failure") }, &live).await,
            Ok(Err("raw failure"))
        );
        // The abort wins over a pending operation.
        let aborted = AbortSignal::new();
        let pending = futures::future::pending::<()>();
        let raced = race_abort(pending, &aborted);
        aborted.abort();
        assert_eq!(raced.await, Err(()));
    });
}

#[test]
fn bounded_insert_keeps_the_window_name_sorted_and_bounded_reporting_evictions() {
    let candidate = |name: &str| ListingCandidate {
        name: name.to_string(),
        is_directory: true,
        is_symbolic_link: false,
    };
    let mut window = Vec::new();
    assert!(!bounded_insert(&mut window, candidate("m"), 2));
    assert!(!bounded_insert(&mut window, candidate("z"), 2));
    // A smaller name lands in place and pushes the current largest out.
    assert!(bounded_insert(&mut window, candidate("a"), 2));
    assert_eq!(
        window.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        vec!["a", "m"]
    );
    // A name at or beyond the full window's tail rejects on one comparison.
    assert!(bounded_insert(&mut window, candidate("t"), 2));
    assert_eq!(
        window.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        vec!["a", "m"]
    );
    assert!(bounded_insert(&mut window, candidate("m"), 2));
    assert_eq!(
        window.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
        vec!["a", "m"]
    );
}

#[test]
fn reports_the_ancestry_as_jump_target_crumbs_ending_at_the_listed_directory() {
    run(async {
        let root = fixture();
        let ctx = Context::root();
        let backend = install(&ctx, Config::default());
        let capability = browse(&backend.capability()).clone();
        let listing = (capability.list)(
            Some(root.path().join("projects").to_string_lossy().into_owned()),
            AbortSignal::new(),
        )
        .await
        .expect("list");
        let tail = listing.crumbs.last().expect("tail crumb");
        assert_eq!(tail.name, "projects");
        assert_eq!(Path::new(&tail.path), root.path().join("projects"));
        assert!(!tail.hidden);
        let parent = &listing.crumbs[listing.crumbs.len() - 2];
        assert_eq!(Path::new(&parent.path), root.path());
        assert_eq!(
            parent.name,
            root.path()
                .file_name()
                .expect("temp root has a name")
                .to_string_lossy()
        );
        // The chain starts at the filesystem root, whose crumb is labeled by
        // its full path.
        assert_eq!(listing.crumbs[0].name, listing.crumbs[0].path);
    });
}

#[test]
fn lists_the_virtual_computer_root_when_no_path_is_given() {
    run(async {
        let ctx = Context::root();
        let backend = install(&ctx, Config::default());
        let capability = browse(&backend.capability()).clone();
        let listing = (capability.list)(None, AbortSignal::new())
            .await
            .expect("initial listing");
        #[cfg(windows)]
        {
            assert!(listing.path.is_empty());
            assert!(listing.crumbs.is_empty());
            assert!(!listing.entries.is_empty());
            assert!(
                listing
                    .entries
                    .iter()
                    .all(|entry| entry.path.ends_with('\\'))
            );
        }
        #[cfg(not(windows))]
        assert_eq!(listing.path, home_dir());
    });
}

#[test]
fn returns_directory_unreadable_for_a_missing_target() {
    run(async {
        let root = fixture();
        let ctx = Context::root();
        let backend = install(&ctx, Config::default());
        let capability = browse(&backend.capability()).clone();
        let missing = root.path().join("no-such-dir");
        let failure = (capability.list)(
            Some(missing.to_string_lossy().into_owned()),
            AbortSignal::new(),
        )
        .await;
        match failure {
            Err(DirectoryPickerListError::Unreadable(error)) => {
                assert_eq!(error.code, DirectoryPickerErrorCode::DirectoryUnreadable);
                assert_eq!(Path::new(&error.path), missing);
            }
            other => panic!("expected unreadable, got {other:?}"),
        }
    });
}

#[test]
fn classifies_fully_qualified_paths_per_platform_drive_less_rooted_windows_forms_rejected() {
    assert!(fully_qualified("/home/x", "linux"));
    assert!(!fully_qualified("x/y", "darwin"));
    assert!(fully_qualified(r"C:\projects", "win32"));
    assert!(fully_qualified("C:/projects", "win32"));
    assert!(fully_qualified(r"\\server\share", "win32"));
    assert!(fully_qualified("//server/share/deep", "win32"));
    // Rooted but drive-less: isAbsolute accepts these, yet resolve() would
    // inject the process's current drive.
    assert!(!fully_qualified(r"\foo", "win32"));
    assert!(!fully_qualified("/foo", "win32"));
    assert!(!fully_qualified("C:relative", "win32"));
    // Incomplete UNC prefixes collapse to drive-relative roots under resolve().
    assert!(!fully_qualified(r"\\", "win32"));
    assert!(!fully_qualified(r"\\server", "win32"));
    assert!(!fully_qualified(r"\\server\", "win32"));
}

#[test]
fn rejects_non_absolute_paths_instead_of_rebasing_them_under_the_process_cwd() {
    run(async {
        let ctx = Context::root();
        let backend = install(&ctx, Config::default());
        let capability = browse(&backend.capability()).clone();
        for relative in ["", "projects", "./projects", ".."] {
            let list_failure =
                (capability.list)(Some(relative.to_string()), AbortSignal::new()).await;
            match list_failure {
                Err(DirectoryPickerListError::Unreadable(error)) => {
                    assert_eq!(error.code, DirectoryPickerErrorCode::DirectoryUnreadable);
                    assert_eq!(error.path, relative);
                }
                other => panic!("expected unreadable for {relative:?}, got {other:?}"),
            }
            let create_failure =
                (capability.create_directory)(relative.to_string(), "child".to_string()).await;
            match create_failure {
                Err(error) => {
                    assert_eq!(error.code, DirectoryPickerErrorCode::DirectoryCreateFailed);
                    assert_eq!(error.path, relative);
                }
                other => panic!("expected create-failed for {relative:?}, got {other:?}"),
            }
        }
    });
}

#[test]
fn creates_one_child_directory_and_surfaces_it_in_the_next_listing() {
    run(async {
        let root = fixture();
        let ctx = Context::root();
        let backend = install(&ctx, Config::default());
        let capability = browse(&backend.capability()).clone();
        let created = (capability.create_directory)(
            root.path().to_string_lossy().into_owned(),
            "fresh".to_string(),
        )
        .await
        .expect("create");
        assert_eq!(Path::new(&created), root.path().join("fresh"));
        let listing = (capability.list)(
            Some(root.path().to_string_lossy().into_owned()),
            AbortSignal::new(),
        )
        .await
        .expect("list");
        assert!(listing.entries.iter().any(|e| e.name == "fresh"));
    });
}

#[test]
fn refuses_an_existing_child_with_directory_exists() {
    run(async {
        let root = fixture();
        let failure =
            create_directory(&root.path().to_string_lossy().into_owned(), "projects").await;
        match failure {
            Err(error) => assert_eq!(error.code, DirectoryPickerErrorCode::DirectoryExists),
            other => panic!("expected directory-exists, got {other:?}"),
        }
    });
}

#[test]
fn refuses_non_segment_names_and_other_filesystem_failures_with_directory_create_failed() {
    run(async {
        let root = fixture();
        for name in ["", "  ", ".", "..", "a/b", r"a\b"] {
            let failure = create_directory(&root.path().to_string_lossy().into_owned(), name).await;
            match failure {
                Err(error) => {
                    assert_eq!(
                        error.code,
                        DirectoryPickerErrorCode::DirectoryCreateFailed,
                        "name {name:?}"
                    );
                }
                other => panic!("expected create-failed for {name:?}, got {other:?}"),
            }
        }
        // Missing parent is a real failure, not a level to invent.
        let missing_parent = create_directory(
            &root
                .path()
                .join("no-such-dir")
                .to_string_lossy()
                .into_owned(),
            "child",
        )
        .await;
        match missing_parent {
            Err(error) => {
                assert_eq!(error.code, DirectoryPickerErrorCode::DirectoryCreateFailed)
            }
            other => panic!("expected create-failed, got {other:?}"),
        }
    });
}

#[test]
fn ancestry_crumbs_chain_starts_at_the_filesystem_root() {
    let crumbs = ancestry_crumbs(Path::new("/a/b/c"));
    assert_eq!(crumbs.len(), 4);
    assert_eq!(crumbs[0].name, crumbs[0].path); // root labeled by full path
    assert_eq!(crumbs[3].name, "c");
}
