#[tokio::main]
async fn main() {
    match dsh_remote_execution::helper::run_cli(std::env::args().skip(1).collect()).await {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
