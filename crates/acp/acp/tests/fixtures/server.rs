use std::io;
use std::sync::Arc;

fn main() {
    let runtime = dsh_acp::runtime(Arc::new(dsh_acp::AcpSessions::default()));
    runtime
        .serve(io::BufReader::new(io::stdin().lock()), io::stdout().lock())
        .expect("serve ACP fixture");
}
