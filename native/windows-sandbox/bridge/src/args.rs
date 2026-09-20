use anyhow::{Result, bail, ensure};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Run,
    Setup,
    Status,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub action: Action,
    pub home: PathBuf,
    pub workspace: PathBuf,
    pub read_only: bool,
    pub network: bool,
    pub reads: Vec<PathBuf>,
    pub temps: Vec<PathBuf>,
    pub ready_event: Option<String>,
    pub timeout_ms: Option<u64>,
    pub tty: bool,
    pub session: Option<String>,
    pub command: Vec<String>,
}

impl Request {
    pub fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut args = args.peekable();
        let mut request = Self {
            action: Action::Run,
            home: PathBuf::new(),
            workspace: PathBuf::new(),
            read_only: false,
            network: true,
            reads: Vec::new(),
            temps: Vec::new(),
            ready_event: None,
            timeout_ms: None,
            tty: false,
            session: None,
            command: Vec::new(),
        };
        let mut seen = std::collections::HashSet::new();
        while let Some(flag) = args.next() {
            if flag == "--" {
                request.command = args.collect();
                break;
            }
            if !matches!(
                flag.as_str(),
                "--runtime-root" | "--read-root" | "--temp-root"
            ) {
                ensure!(seen.insert(flag.clone()), "duplicate flag {flag}");
            }
            match flag.as_str() {
                "--setup" => {
                    ensure!(request.action == Action::Run, "conflicting action");
                    request.action = Action::Setup;
                }
                "--status" => {
                    ensure!(request.action == Action::Run, "conflicting action");
                    request.action = Action::Status;
                }
                "--tty" => request.tty = true,
                _ => {
                    let value = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("missing value for {flag}"))?;
                    ensure!(
                        !value.contains('\0') && value.len() <= 32768,
                        "invalid flag value"
                    );
                    match flag.as_str() {
                        "--native-home" => request.home = PathBuf::from(value),
                        "--workspace" => request.workspace = PathBuf::from(value),
                        "--mode" => {
                            request.read_only = match value.as_str() {
                                "read-only" => true,
                                "workspace-write" => false,
                                _ => bail!("unsupported native mode {value}"),
                            }
                        }
                        "--network" => {
                            request.network = match value.as_str() {
                                "enabled" => true,
                                "restricted" => false,
                                _ => bail!("unsupported network policy"),
                            }
                        }
                        "--runtime-root" | "--read-root" => {
                            request.reads.push(PathBuf::from(value))
                        }
                        "--temp-root" => request.temps.push(PathBuf::from(value)),
                        "--session-id" => request.session = Some(value),
                        "--ready-event" => {
                            ensure!(
                                value.starts_with("Local\\DSH-Sandbox-Ready-")
                                    && value.len() <= 160,
                                "invalid ready event"
                            );
                            request.ready_event = Some(value);
                        }
                        "--command-timeout-ms" => {
                            let ms = value.parse::<u64>()?;
                            ensure!((1..=2_147_483_647).contains(&ms), "invalid timeout");
                            request.timeout_ms = Some(ms);
                        }
                        _ => bail!("unsupported flag {flag}"),
                    }
                }
            }
        }
        ensure!(
            request.home.is_absolute(),
            "native state directory must be absolute"
        );
        if request.action != Action::Status {
            ensure!(
                request.workspace.is_absolute(),
                "workspace must be absolute"
            );
            request.workspace = std::fs::canonicalize(&request.workspace)?;
            ensure!(
                request.workspace.is_dir() && request.workspace.parent().is_some(),
                "workspace must be a specific project directory"
            );
            if let Some(profile) = crate::toolchain::real_profile()
                .ok()
                .and_then(|p| std::fs::canonicalize(p).ok())
            {
                ensure!(
                    request.workspace != profile,
                    "the whole user profile cannot be a workspace"
                );
            }
        }
        ensure!(
            request.reads.len() + request.temps.len() <= 32,
            "too many execution roots"
        );
        for path in request.reads.iter().chain(&request.temps) {
            ensure!(
                path.is_absolute() && path.is_dir(),
                "execution root must be an existing absolute directory"
            );
        }
        if request.action == Action::Run {
            ensure!(!request.command.is_empty(), "missing command");
        }
        Ok(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_widening_and_ambiguous_requests() {
        for flags in [
            vec!["--mode", "danger-full-access"],
            vec!["--network", "automatic"],
            vec!["--setup", "--status"],
            vec!["--mode", "read-only", "--mode", "workspace-write"],
        ] {
            assert!(Request::parse(flags.into_iter().map(str::to_owned)).is_err());
        }
    }
}
