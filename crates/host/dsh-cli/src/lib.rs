//! dsh — command-line entry. Rust port of `apps/cli/src/args.ts` (the
//! launcher parses only what it owns: which profile to boot, which extra
//! patch overlays to apply, and the config dumps; everything after its own
//! flags belongs to the booted tree verbatim).

pub mod acp_stdio;
pub mod native_plugin;
pub mod profile_boot;
pub mod run_profile;
pub mod sdk_stdio;

pub use run_profile::{
    ProfileInterrupt, ProfileInterruptLatch, ProfileSurface, RunProfileHandle, RunProfileRequest,
    resolve_profile_surface, run_profile, run_profile_with_interrupt,
};

/// Boot a named profile and hand it the invocation's inner arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileInvocation {
    pub profile: String,
    /// Extra patch-list overlays applied after the profile's own layer, in
    /// argv order.
    pub patches: Vec<String>,
    /// Everything after the launcher's own flags, verbatim, for injected
    /// app plugins.
    pub args: Vec<String>,
}

/// Print a composed profile tree and exit without booting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpConfigInvocation {
    pub profile: String,
    /// Omit the profile's user layer and --patch overlays; print bundle
    /// layers only.
    pub default_only: bool,
    pub patches: Vec<String>,
}

/// Manage a profile's plugins: forward `args` to pnpm inside the profile
/// directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInvocation {
    pub profile: String,
    /// Raw pnpm arguments, verbatim.
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: std::path::PathBuf,
}

pub fn plugin_command_spec(
    invocation: &PluginInvocation,
    home: &std::path::Path,
) -> Result<PluginCommandSpec, String> {
    let cwd = dsh_app_boot::resolve_profile_dir(&invocation.profile, home)?;
    if !cwd.join("package.json").exists() {
        return Err(format!(
            "dsh: profile {:?} is not initialized",
            invocation.profile
        ));
    }
    Ok(PluginCommandSpec {
        program: "dsh-native-plugin".to_string(),
        args: invocation.args.clone(),
        cwd,
    })
}

pub fn run_plugin_command(
    invocation: &PluginInvocation,
    home: &std::path::Path,
) -> Result<(), String> {
    let spec = plugin_command_spec(invocation, home)?;
    native_plugin::run(&spec.cwd, &spec.args)
}

/// Read-only legacy-history inspection. It never writes to the source or DSH_HOME.
pub fn inspect_legacy_history(source: &std::path::Path) -> Result<String, String> {
    if !source.exists() {
        return Err(format!(
            "history source does not exist: {}",
            source.display()
        ));
    }
    let mut jsonl = 0usize;
    let mut other = 0usize;
    fn visit(path: &std::path::Path, jsonl: &mut usize, other: &mut usize) -> std::io::Result<()> {
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                visit(&entry?.path(), jsonl, other)?;
            }
        } else if path.extension().and_then(|v| v.to_str()) == Some("jsonl") {
            *jsonl += 1;
        } else {
            *other += 1;
        }
        Ok(())
    }
    visit(source, &mut jsonl, &mut other)
        .map_err(|error| format!("history scan failed: {error}"))?;
    Ok(format!(
        "source={}\njsonl_files={}\nother_files={}\nimport=not_performed\n",
        source.display(),
        jsonl,
        other
    ))
}

pub fn import_legacy_history(
    source: &std::path::Path,
    target_home: &std::path::Path,
) -> Result<usize, String> {
    let mut candidates = Vec::new();
    fn visit(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                visit(&entry?.path(), out)?;
            }
        } else if path.file_name().and_then(|v| v.to_str()) == Some("session.jsonl") {
            out.push(path.to_path_buf());
        }
        Ok(())
    }
    visit(source, &mut candidates).map_err(|error| format!("history scan failed: {error}"))?;
    let sessions_root = target_home.join("sessions");
    let mut imported = 0usize;
    for source_file in candidates {
        let content = std::fs::read_to_string(&source_file)
            .map_err(|error| format!("history read {}: {error}", source_file.display()))?;
        let first = content
            .lines()
            .next()
            .ok_or_else(|| format!("history artifact has no header: {}", source_file.display()))?;
        let header: dsh_session_persistence_jsonl::HeaderLine = serde_json::from_str(first)
            .map_err(|error| format!("history header {}: {error}", source_file.display()))?;
        if header.type_ != "session" {
            return Err(format!(
                "unsupported history artifact: {}",
                source_file.display()
            ));
        }
        let root = sessions_root.to_string_lossy();
        let target = dsh_session_persistence_jsonl::log_path(
            &root,
            header.cwd.as_deref(),
            &header.id,
            dsh_session_persistence_jsonl::JsonlCompression::None,
        );
        if target.exists() {
            return Err(format!(
                "history target already exists: {}",
                target.display()
            ));
        }
        std::fs::create_dir_all(target.parent().expect("session artifact parent"))
            .map_err(|error| format!("history target directory: {error}"))?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = options
            .open(&target)
            .map_err(|error| format!("history target {}: {error}", target.display()))?;
        std::io::Write::write_all(&mut file, content.as_bytes())
            .map_err(|error| format!("history target write {}: {error}", target.display()))?;
        file.sync_all()
            .map_err(|error| format!("history target sync: {error}"))?;
        imported += 1;
    }
    Ok(imported)
}

/// The resolved `dsh` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshInvocation {
    Profile(ProfileInvocation),
    DumpConfig(DumpConfigInvocation),
    Plugin(PluginInvocation),
    HistoryInspect(HistoryInspectInvocation),
    HistoryImport(HistoryImportInvocation),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryInspectInvocation {
    pub source: std::path::PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryImportInvocation {
    pub source: std::path::PathBuf,
    pub target_home: std::path::PathBuf,
}

/// The launcher's own help text.
pub const HELP_TEXT: &str = "\
dsh: boot a DeepSeek Harness profile — an ordered stack of plugin-bundle patch layers under your own overrides.

Examples:
  dsh --profile web                          boot the web profile (same as: dsh web)
  dsh web --host 0.0.0.0 --port 3080        expose web/API to a trusted network (unsafe without a firewall)
  dsh --profile headless \"run the tests\"     answer one task, print the result, and exit
  dsh --profile tui --patch ./extra.yml      boot a custom profile with one extra overlay
  dsh --profile tui --resume <session>       arguments after the launcher flags reach the app
  dsh --profile web --help                   the web app's own flags and help
  dsh plugin --profile tui add <package>     install a plugin into the tui profile
  dsh history inspect <directory>            inspect legacy history without importing it
  dsh history import <directory> --to <home> import compatible JSONL without overwriting
";

/// Parse failure: the CLI prints the message and exits with a code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshArgsError {
    pub message: String,
    pub exit_code: i32,
}

impl DshArgsError {
    fn error(message: impl Into<String>) -> Self {
        Self {
            message: format!("error: {}", message.into()),
            exit_code: 1,
        }
    }
}

impl std::fmt::Display for DshArgsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The launcher flags shared by the default command and the `web` alias.
#[derive(Default)]
struct BootOptions {
    patch: Vec<String>,
    dump_config: bool,
    dump_default_config: bool,
}

/// Resolve a boot or dump invocation from the launcher flags and the
/// leftover inner arguments.
fn resolve_boot(
    profile: String,
    options: BootOptions,
    args: Vec<String>,
) -> Result<DshInvocation, DshArgsError> {
    let patches = options.patch;
    if patches.iter().any(|patch| patch.is_empty()) {
        return Err(DshArgsError::error("--patch needs a path"));
    }
    if !options.dump_config && !options.dump_default_config {
        return Ok(DshInvocation::Profile(ProfileInvocation {
            profile,
            patches,
            args,
        }));
    }
    if options.dump_config && options.dump_default_config {
        return Err(DshArgsError::error(
            "--dump-config and --dump-default-config are mutually exclusive",
        ));
    }
    if !args.is_empty() {
        let rendered: Vec<String> = args
            .iter()
            .map(|argument| format!("{argument:?}"))
            .collect();
        return Err(DshArgsError::error(format!(
            "config dumps take no app arguments, got {}",
            rendered.join(" ")
        )));
    }
    let default_only = options.dump_default_config;
    if default_only && !patches.is_empty() {
        return Err(DshArgsError::error(
            "--dump-default-config prints the bundle layers and takes no --patch",
        ));
    }
    Ok(DshInvocation::DumpConfig(DumpConfigInvocation {
        profile,
        default_only,
        patches,
    }))
}

/// Parse one full argument collection.
fn parse_options(
    args: &[String],
    default_profile: Option<&str>,
) -> Result<(String, BootOptions, Vec<String>), DshArgsError> {
    let mut options = BootOptions::default();
    let mut profile: Option<String> = None;
    let mut inner: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--profile" => {
                let Some(value) = args.get(i + 1) else {
                    return Err(DshArgsError::error(
                        "option '--profile <name>' argument missing",
                    ));
                };
                profile = Some(value.clone());
                i += 2;
            }
            "--patch" => {
                let Some(value) = args.get(i + 1) else {
                    return Err(DshArgsError::error(
                        "option '--patch <path>' argument missing",
                    ));
                };
                options.patch.push(value.clone());
                i += 2;
            }
            "--dump-config" => {
                options.dump_config = true;
                i += 1;
            }
            "--dump-default-config" => {
                options.dump_default_config = true;
                i += 1;
            }
            "-h" | "--help" if profile.is_none() && default_profile.is_none() => {
                return Err(DshArgsError {
                    message: HELP_TEXT.to_string(),
                    exit_code: 0,
                });
            }
            _ => {
                inner.extend_from_slice(&args[i..]);
                break;
            }
        }
    }
    let profile = match profile.or_else(|| default_profile.map(str::to_string)) {
        Some(profile) => profile,
        None => {
            if inner
                .iter()
                .any(|argument| argument == "-h" || argument == "--help")
            {
                return Err(DshArgsError {
                    message: HELP_TEXT.to_string(),
                    exit_code: 0,
                });
            }
            return Err(DshArgsError::error("--profile <name> is required"));
        }
    };
    if profile.is_empty() {
        return Err(DshArgsError::error("--profile needs a name"));
    }
    Ok((profile, options, inner))
}

/// Resolve argv into one invocation, or an error for help (exit 0),
/// version (exit 0), or a parse failure (exit 1).
pub fn parse_dsh_args(argv: &[String], version: &str) -> Result<DshInvocation, DshArgsError> {
    if argv.len() == 1 && (argv[0] == "-V" || argv[0] == "--version") {
        return Err(DshArgsError {
            message: version.to_string(),
            exit_code: 0,
        });
    }
    // The `web` alias: a subcommand with the launcher flags.
    if argv.first().map(String::as_str) == Some("web") {
        let (profile, options, inner) = parse_options(&argv[1..], Some("web"))?;
        return resolve_boot(profile, options, inner);
    }
    // The read-only legacy-history inspector is intentionally separate from Host boot.
    if argv.first().map(String::as_str) == Some("history") {
        return match argv.get(1).map(String::as_str) {
            Some("inspect") if argv.len() == 3 => {
                Ok(DshInvocation::HistoryInspect(HistoryInspectInvocation {
                    source: argv[2].clone().into(),
                }))
            }
            Some("inspect") => Err(DshArgsError::error(
                "history inspect needs a source directory",
            )),
            Some("import") if argv.len() == 5 && argv[3] == "--to" => {
                Ok(DshInvocation::HistoryImport(HistoryImportInvocation {
                    source: argv[2].clone().into(),
                    target_home: argv[4].clone().into(),
                }))
            }
            Some("import") => Err(DshArgsError::error(
                "history import needs <source> --to <DSH_HOME>",
            )),
            _ => Err(DshArgsError::error("history supports inspect and import")),
        };
    }
    // The `plugin` subcommand.
    if argv.first().map(String::as_str) == Some("plugin") {
        let (profile, _options, inner) = parse_options(&argv[1..], None)?;
        if inner.is_empty() {
            return Err(DshArgsError::error(
                "plugin needs pnpm arguments to forward (e.g. add <package>)",
            ));
        }
        return Ok(DshInvocation::Plugin(PluginInvocation {
            profile,
            args: inner,
        }));
    }
    let (profile, options, inner) = parse_options(argv, None)?;
    resolve_boot(profile, options, inner)
}
