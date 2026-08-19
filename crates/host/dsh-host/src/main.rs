//! The runnable DeepSeek Harness Host binary (M6 skeleton): compose the
//! core spine, mount the invariant companions, print a boot report, and
//! exit.

use cordis::Context;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ctx = Context::root();
    let spine = dsh_host::compose_host(&ctx)?;
    dsh_host::mount_companions(&spine)?;
    // Allow the optional-service fibers to settle before the report.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let report = dsh_host::boot_report(&spine).await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    spine.shutdown().await?;
    Ok(())
}
