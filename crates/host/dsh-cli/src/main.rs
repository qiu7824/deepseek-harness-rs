//! dsh — command-line entry. The launcher parses its own flags, then
//! dispatches per mode. Rust port of `apps/cli/src/bin.ts` (the profile
//! boot itself arrives with the profile-boot milestone; the adapter prints
//! and exits for help/version/parse errors).

use dsh_host_cli::{
    DshArgsError, DshInvocation, ProfileInterruptLatch, RunProfileRequest, parse_dsh_args,
    run_profile_with_interrupt,
};

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "__dsh-sdk-jsonrpc") {
        if let Err(error) = dsh_host_cli::sdk_stdio::run().await {
            eprintln!("dsh-sdk-jsonrpc: {error}");
            std::process::exit(1);
        }
        return;
    }
    if args.first().is_some_and(|arg| arg == "__dsh-acp") {
        if let Err(error) = dsh_host_cli::acp_stdio::run().await {
            eprintln!("dsh-acp: {error}");
            std::process::exit(1);
        }
        return;
    }
    #[cfg(windows)]
    if args
        .first()
        .is_some_and(|arg| arg == "__dsh-sandbox-windows")
    {
        match dsh_sandbox_local::run_windows_sandbox(args.into_iter().skip(1)) {
            Ok(exit_code) => std::process::exit(exit_code),
            Err(error) => {
                eprintln!("dsh-sandbox-windows: {error}");
                std::process::exit(125);
            }
        }
    }
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
            let home = dsh_home_paths::resolve_dsh_home(None, &|name| std::env::var(name).ok());
            let interrupt = ProfileInterruptLatch::new(Box::pin(async {
                tokio::signal::ctrl_c()
                    .await
                    .map_err(|error| format!("failed to wait for Ctrl+C: {error}"))
            }));
            let handle = match run_profile_with_interrupt(
                RunProfileRequest {
                    profile: invocation.profile,
                    patches: invocation.patches,
                    args: invocation.args,
                    home,
                    telemetry_env: std::env::var("DSH_TELEMETRY_DISABLED").ok(),
                    install_anchor: std::env::var_os("DSH_INSTALL_ANCHOR")
                        .map(std::path::PathBuf::from)
                        .or_else(|| {
                            let executable = std::env::current_exe().ok()?;
                            executable
                                .ancestors()
                                .map(|dir| dir.join("package.json"))
                                .find(|candidate| candidate.is_file())
                        }),
                },
                Some(interrupt.waiter()),
            )
            .await
            {
                Ok(handle) => handle,
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            };
            if let Some(url) = handle.readiness_url() {
                println!("dsh web: {url}");
                if let Err(error) = interrupt.waiter().await {
                    eprintln!("dsh: {error}");
                    std::process::exit(1);
                }
            }
            if let Some(output) = handle.output() {
                println!("{output}");
            }
            if let Err(error) = handle.shutdown().await {
                eprintln!("dsh: shutdown failed: {error}");
                std::process::exit(1);
            }
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
            let home = dsh_home_paths::resolve_dsh_home(None, &|name| std::env::var(name).ok());
            match dsh_host_cli::run_plugin_command(&invocation, &home) {
                Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(1);
                }
            }
        }
    }
}
