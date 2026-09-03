use std::path::{Path, PathBuf};

use cordis::Context;
use dsh_credentials::{CredentialProvider, credential_ref};
use dsh_credentials_local::document::{parse_credentials_document, render_document};
use dsh_credentials_local::index::{Config, LocalCredentialProvider};

fn versioned_fixture() -> &'static str {
    concat!(
        "version: 1\n",
        "refs:\n",
        "  DSH_COMPAT_KEY: fixture-ref\n",
        "records:\n",
        "  llm-pi-ai/openai-codex:\n",
        "    kind: grant\n",
        "    payload:\n",
        "      access: fixture-grant\n",
    )
}

async fn temp_credentials_path(label: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "dsh-credentials-local-{label}-{}",
        uuid::Uuid::new_v4()
    ));
    tokio::fs::create_dir_all(&directory).await.unwrap();
    directory.join(".credentials.yaml")
}

async fn write_owner_only(path: &Path, text: &str) {
    dsh_atomic_write::write_file_atomic(
        path,
        text.as_bytes(),
        dsh_atomic_write::WriteFileAtomicOptions {
            mode: 0o600,
            dir_mode: Some(0o700),
        },
    )
    .await
    .unwrap();
}

async fn cleanup(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::remove_dir_all(parent).await;
    }
}

#[test]
fn parses_versioned_refs_without_treating_metadata_as_credentials() {
    let parsed = parse_credentials_document(versioned_fixture(), "fixture.yaml").unwrap();
    assert_eq!(
        parsed.get("DSH_COMPAT_KEY").map(String::as_str),
        Some("fixture-ref")
    );
    assert_eq!(parsed.len(), 1);
}

#[test]
fn edits_a_nested_ref_without_changing_the_records_section() {
    let reference = credential_ref("DSH_COMPAT_KEY");
    let rendered = render_document(Some(versioned_fixture()), &reference, Some("replacement"));

    let parsed = parse_credentials_document(&rendered, "fixture.yaml").unwrap();
    assert_eq!(
        parsed.get("DSH_COMPAT_KEY").map(String::as_str),
        Some("replacement")
    );
    assert!(rendered.contains(
        "records:\n  llm-pi-ai/openai-codex:\n    kind: grant\n    payload:\n      access: fixture-grant\n"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn provider_reads_updates_and_reopens_the_versioned_document() {
    let path = temp_credentials_path("versioned").await;
    write_owner_only(&path, versioned_fixture()).await;
    let reference = credential_ref("DSH_COMPAT_KEY");

    let first_context = Context::root();
    let first = LocalCredentialProvider::install(
        &first_context,
        Config {
            path: Some(path.to_string_lossy().into_owned()),
            watch: Some(false),
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(
        first.resolve(&reference).await.unwrap().value,
        "fixture-ref"
    );
    first.set(&reference, "rotated-fixture").await.unwrap();
    first.drain().await;

    let persisted = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(persisted.starts_with("version: 1\nrefs:\n"));
    assert!(persisted.contains("access: fixture-grant"));

    let second_context = Context::root();
    let second = LocalCredentialProvider::install(
        &second_context,
        Config {
            path: Some(path.to_string_lossy().into_owned()),
            watch: Some(false),
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(
        second.resolve(&reference).await.unwrap().value,
        "rotated-fixture"
    );
    second.drain().await;
    cleanup(&path).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn migrates_the_recognized_flat_document_once_and_reopens_it() {
    let path = temp_credentials_path("flat-migration").await;
    write_owner_only(
        &path,
        "# preserved annotation\nDSH_COMPAT_KEY: fixture-ref\n",
    )
    .await;
    let reference = credential_ref("DSH_COMPAT_KEY");

    let first_context = Context::root();
    let first = LocalCredentialProvider::install(
        &first_context,
        Config {
            path: Some(path.to_string_lossy().into_owned()),
            watch: Some(false),
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(
        first.resolve(&reference).await.unwrap().value,
        "fixture-ref"
    );
    first.drain().await;

    let migrated = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(
        migrated,
        "version: 1\nrefs:\n  # preserved annotation\n  DSH_COMPAT_KEY: fixture-ref\n"
    );

    let second_context = Context::root();
    let second = LocalCredentialProvider::install(
        &second_context,
        Config {
            path: Some(path.to_string_lossy().into_owned()),
            watch: Some(false),
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(
        second.resolve(&reference).await.unwrap().value,
        "fixture-ref"
    );
    second.drain().await;
    cleanup(&path).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn new_writes_use_the_versioned_layout() {
    let path = temp_credentials_path("fresh-write").await;
    let reference = credential_ref("DSH_COMPAT_KEY");
    let context = Context::root();
    let provider = LocalCredentialProvider::install(
        &context,
        Config {
            path: Some(path.to_string_lossy().into_owned()),
            watch: Some(false),
            ..Config::default()
        },
    )
    .unwrap();

    provider.set(&reference, "fixture-ref").await.unwrap();
    provider.drain().await;
    assert_eq!(
        tokio::fs::read_to_string(&path).await.unwrap(),
        "version: 1\nrefs:\n  DSH_COMPAT_KEY: fixture-ref\n"
    );
    cleanup(&path).await;
}
