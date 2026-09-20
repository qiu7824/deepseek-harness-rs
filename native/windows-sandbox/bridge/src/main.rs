mod args;
mod environment;
mod launcher;
#[cfg(windows)]
mod pool;
mod toolchain;
#[cfg(windows)]
mod windows;

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--build-info") {
        println!(
            "{}",
            serde_json::json!({"backend":"windows-native","protocolVersion":1,"version":env!("CARGO_PKG_VERSION"),"revision":env!("DSH_NATIVE_REVISION"),"dirty":env!("DSH_NATIVE_DIRTY")!="false","engineRevision":"bb5054fe47abe73ecbbd454751066a28c89f4bb9"})
        );
        return;
    }
    let result = args::Request::parse(std::env::args().skip(1)).and_then(|request| {
        #[cfg(windows)]
        {
            windows::run(request)
        }
        #[cfg(not(windows))]
        {
            let _ = request;
            anyhow::bail!("Windows native sandbox is unavailable on this platform")
        }
    });
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("[DSH_NATIVE_SANDBOX_FAILED] {error:#}");
            std::process::exit(125);
        }
    }
}
