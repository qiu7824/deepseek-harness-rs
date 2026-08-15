//! Test child-helper binary: spawned by `tests/spawn.rs` to exercise the
//! local subprocess seam deterministically across platforms. Not part of the
//! published API; it exists so integration tests can spawn a real child with
//! scripted stdio/exit/signal behavior without depending on system shells.

use std::io::{Read, Write};
use std::process::exit;
use std::thread::sleep;
use std::time::Duration;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--dsh-child") {
        args.remove(0);
    }
    let Some(mode) = args.first().map(String::as_str) else {
        eprintln!("child: missing mode");
        exit(2);
    };
    let rest = &args[1..];
    match mode {
        "stdout" => {
            print!("{}", rest.join(" "));
        }
        "stderr" => {
            eprint!("{}", rest.join(" "));
        }
        "both" => {
            // `both <text> <count>`: write `<text>\n` `<count>` times to
            // BOTH stdout and stderr, flushing so pipes observe the bytes.
            let text = rest.first().map(String::as_str).unwrap_or("line");
            let count: u64 = rest.get(1).and_then(|value| value.parse().ok()).unwrap_or(1);
            let stdout = std::io::stdout();
            let stderr = std::io::stderr();
            let mut out = stdout.lock();
            let mut err = stderr.lock();
            for _ in 0..count {
                let _ = out.write_all(text.as_bytes());
                let _ = out.write_all(b"\n");
                let _ = out.flush();
                let _ = err.write_all(text.as_bytes());
                let _ = err.write_all(b"\n");
                let _ = err.flush();
            }
        }
        "exit" => {
            let code: i32 = rest.first().and_then(|value| value.parse().ok()).unwrap_or(0);
            exit(code);
        }
        "sleep" => {
            let ms: u64 = rest.first().and_then(|value| value.parse().ok()).unwrap_or(100);
            sleep(Duration::from_millis(ms));
        }
        "stdin-cat" => {
            let mut buffer = String::new();
            let _ = std::io::stdin().read_to_string(&mut buffer);
            print!("{buffer}");
        }
        "trap-ignore-term" => {
            // Ignore SIGTERM and run forever: exercises the grace→SIGKILL
            // escalation (POSIX only).
            #[cfg(unix)]
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            loop {
                sleep(Duration::from_millis(100));
            }
        }
        "die-sigterm" => {
            // Die from SIGTERM's default disposition (POSIX only).
            #[cfg(unix)]
            unsafe {
                libc::raise(libc::SIGTERM);
            }
            #[cfg(not(unix))]
            exit(1);
        }
        "spawn-then-wait" => {
            // Spawn a sleeping grandchild and wait even longer: the grandchild
            // must survive the direct child's exit so tree-scoped teardown has
            // a survivor to kill.
            let ms: u64 = rest.first().and_then(|value| value.parse().ok()).unwrap_or(500);
            let helper = std::env::current_exe().expect("current_exe");
            let child = std::process::Command::new(helper)
                .args(["--dsh-child", "sleep", &(ms * 10).to_string()])
                .spawn();
            if let Ok(mut child) = child {
                let _ = child.wait();
            }
            sleep(Duration::from_millis(ms));
        }
        _ => {
            eprintln!("child: unknown mode {mode}");
            exit(2);
        }
    }
}
