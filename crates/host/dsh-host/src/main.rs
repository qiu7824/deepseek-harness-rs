//! The runnable DeepSeek Harness Host binary.

use cordis::Context;

#[cfg(windows)]
#[global_allocator]
static GLOBAL_ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = Context::root();
    let port = std::env::args()
        .skip_while(|arg| arg != "--port")
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(58080);
    let spine = dsh_host::compose_persistent_host_at_port(&ctx, std::env::current_dir()?, Some("default"), port)?;
    dsh_host::mount_companions(&spine)?;
    // Allow the optional-service fibers to settle before the report.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let report = dsh_host::boot_report(&spine).await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    // Launcher-owned process: remain alive until the launcher terminates us.
    // The launcher owns shutdown and sends a process termination signal; waiting
    // on ctrl_c alone is unreliable for a hidden Windows child process.
    std::future::pending::<()>().await;
    spine.shutdown().await?;
    Ok(())
}
