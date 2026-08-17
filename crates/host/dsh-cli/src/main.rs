//! dsh — command-line entry. The launcher parses its own flags, then
//! dispatches per mode. Rust port of `apps/cli/src/bin.ts` (the profile
//! boot itself arrives with the profile-boot milestone; the adapter prints
//! and exits for help/version/parse errors).

use dsh_host_cli::{DshArgsError, DshInvocation, parse_dsh_args};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let version = env!("CARGO_PKG_VERSION");
    let invocation = match parse_dsh_args(&args, version) {
        Ok(invocation) => invocation,
        Err(DshArgsError { message, exit_code }) => {
            if exit_code == 0 {
                print!("{message}");
            } else {
                eprintln!("{message}");
            }
            std::process::exit(exit_code);
        }
    };
    match invocation {
        DshInvocation::Profile(invocation) => {
            eprintln!(
                "dsh: profile boot is not implemented in the Rust composition yet (profile \"{}\", {} patch file(s), {} inner argument(s))",
                invocation.profile,
                invocation.patches.len(),
                invocation.args.len()
            );
            std::process::exit(1);
        }
        DshInvocation::DumpConfig(invocation) => {
            let home = dsh_home_paths::resolve_dsh_home(None, &|name| std::env::var(name).ok());
            match dsh_host_cli::profile_boot::run_dump_config(
                &invocation.profile,
                invocation.default_only,
                &invocation.patches,
                &home,
            ) {
                Ok((dump, warnings)) => {
                    for warning in warnings {
                        eprintln!("{warning}");
                    }
                    print!("{dump}");
                }
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        }
        DshInvocation::Plugin(invocation) => {
            eprintln!(
                "dsh: plugin forwarding is not implemented in the Rust composition yet (profile \"{}\", pnpm args {:?})",
                invocation.profile,
                invocation.args
            );
            std::process::exit(1);
        }
    }
}
