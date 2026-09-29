use anyhow::{Context, Result, bail, ensure};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Run,
    Setup,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Implementation { Elevated, Unelevated }
impl Implementation {
    pub fn as_str(self) -> &'static str { match self { Self::Elevated => "elevated", Self::Unelevated => "unelevated" } }
}

#[derive(Debug, Clone)]
pub struct Request {
    pub action: Action,
    pub implementation: Implementation,
    pub home: PathBuf,
    pub workspace: PathBuf,
    pub read_only: bool,
    pub network: bool,
    pub reads: Vec<PathBuf>,
    pub temps: Vec<PathBuf>,
    pub private_roots: Vec<PathBuf>,
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
            implementation: Implementation::Elevated,
            home: PathBuf::new(),
            workspace: PathBuf::new(),
            read_only: false,
            network: true,
            reads: Vec::new(),
            temps: Vec::new(),
            private_roots: Vec::new(),
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
                "--runtime-root" | "--read-root" | "--temp-root" | "--private-root"
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
                        "--implementation" => request.implementation = match value.as_str() {
                            "elevated" => Implementation::Elevated,
                            "unelevated" => Implementation::Unelevated,
                            _ => bail!("unsupported Windows sandbox implementation"),
                        },
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
                        "--private-root" => request.private_roots.push(PathBuf::from(value)),
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
            request.workspace = std::fs::canonicalize(&request.workspace)
                .with_context(|| format!("workspace is unavailable: {}", request.workspace.display()))?;
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
            request.reads.len() + request.temps.len() + request.private_roots.len() <= 4096,
            "too many execution roots"
        );
        // Name the offending root: callers cannot otherwise tell which of many
        // attachment, runtime or temporary grants blocked the whole launch.
        for path in &request.reads {
            ensure!(path.is_absolute() && (path.is_dir() || path.is_file()), "read root must be an existing absolute file or directory: {}", path.display());
        }
        for path in &request.temps {
            ensure!(
                path.is_absolute() && path.is_dir(),
                "execution root must be an existing absolute directory: {}",
                path.display()
            );
        }
        for path in &request.private_roots {
            ensure!(path.is_absolute() && path.parent().is_some() && !path.components().any(|part|matches!(part,std::path::Component::ParentDir)), "private root must be an absolute product directory: {}", path.display());
            if path.exists() { ensure!(path.is_dir(),"private root must be a directory: {}", path.display()); }
            ensure!(crate::toolchain::real_profile().map(|profile|codex_windows_sandbox::canonicalize_path(path)!=codex_windows_sandbox::canonicalize_path(&profile)).unwrap_or(false),"the whole user profile cannot be a private root: {}", path.display());
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
    #[test]
    fn exact_read_files_and_many_roots_do_not_expand_to_the_parent() {
        let root=std::env::temp_dir().join(format!("dsh-native-args-{}",std::process::id()));std::fs::create_dir_all(&root).unwrap();
        let file=root.join("attachment.txt");std::fs::write(&file,"fixture").unwrap();
        let base=vec!["--native-home".to_string(),root.join("native").display().to_string(),"--workspace".to_string(),root.display().to_string()];
        let mut args=base.clone();for _ in 0..40 {args.extend(["--read-root".into(),file.display().to_string()]);}
        args.extend(["--private-root".into(),root.join("offline-private").display().to_string(),"--".into(),"whoami.exe".into()]);
        let parsed=Request::parse(args.into_iter()).unwrap();assert_eq!(parsed.reads.len(),40);assert!(parsed.reads.iter().all(|path|path==&file));
        let mut invalid=base.clone();invalid.extend(["--temp-root".into(),file.display().to_string(),"--".into(),"whoami.exe".into()]);
        let error=Request::parse(invalid.into_iter()).unwrap_err().to_string();assert!(error.contains(&file.display().to_string()),"{error}");
        let missing=root.join("已清理的附件.docx");
        let mut invalid=base;invalid.extend(["--read-root".into(),missing.display().to_string(),"--".into(),"whoami.exe".into()]);
        let error=Request::parse(invalid.into_iter()).unwrap_err().to_string();assert!(error.contains(&missing.display().to_string()),"{error}");
        std::fs::remove_file(file).unwrap();std::fs::remove_dir(root).unwrap();
    }
}
