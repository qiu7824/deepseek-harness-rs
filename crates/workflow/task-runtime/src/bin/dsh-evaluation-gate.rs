//! Validate a release evidence manifest without running or repeating builds.
fn main() {
    let result = run();
    match result {
        Ok(true) => {}
        Ok(false) => std::process::exit(2),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1)
        }
    }
}
fn run() -> Result<bool, String> {
    let argument = std::env::args()
        .nth(1)
        .ok_or("Usage: dsh-evaluation-gate --catalog | <release-evidence.json>")?;
    if argument == "--catalog" {
        println!(
            "{}",
            serde_json::to_string_pretty(&dsh_task_runtime::evaluation::catalog())
                .map_err(|e| e.to_string())?
        );
        return Ok(true);
    }
    let path = std::path::Path::new(&argument);
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 8 * 1024 * 1024 {
        return Err("Evidence manifest exceeds 8 MiB".into());
    }
    let report: dsh_task_runtime::evaluation::ReleaseEvidence =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let summary = report.evaluate(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| std::path::Path::new(".")),
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?
    );
    Ok(summary.release_allowed)
}
