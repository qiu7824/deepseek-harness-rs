//! Test child-helper binary for `dsh-native-command`: scripted stdio/exit/
//! sleep behavior so the runner tests never invoke a shell.

use std::io::Write;
use std::process::exit;
use std::thread::sleep;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(mode) = args.first().map(String::as_str) else {
        eprintln!("native-child: missing mode");
        exit(2);
    };
    match mode {
        "echo-out" => {
            let _ = std::io::stdout().write_all(args[1..].join(" ").as_bytes());
        }
        "echo-err" => {
            let _ = std::io::stderr().write_all(args[1..].join(" ").as_bytes());
        }
        "exit" => {
            let code: i32 = args.get(1).and_then(|value| value.parse().ok()).unwrap_or(0);
            exit(code);
        }
        "sleep-forever" => loop {
            sleep(Duration::from_millis(100));
        },
        _ => exit(2),
    }
}
