//! dsh — command-line entry. Rust port of `apps/cli/src/args.ts` (the
//! launcher parses only what it owns: which profile to boot, which extra
//! patch overlays to apply, and the config dumps; everything after its own
//! flags belongs to the booted tree verbatim).

pub mod acp_stdio;
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
        program: std::env::var("DSH_PNPM_BIN").unwrap_or_else(|_| "pnpm".to_string()),
        args: invocation.args.clone(),
        cwd,
    })
}

pub fn run_plugin_command(
    invocation: &PluginInvocation,
    home: &std::path::Path,
) -> Result<std::process::ExitStatus, String> {
    let spec = plugin_command_spec(invocation, home)?;
    std::process::Command::new(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .status()
        .map_err(|error| format!("dsh: failed to run {}: {error}", spec.program))
}

/// The resolved `dsh` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshInvocation {
    Profile(ProfileInvocation),
    DumpConfig(DumpConfigInvocation),
    Plugin(PluginInvocation),
}

/// The launcher's own help text.
pub const HELP_TEXT: &str = "\
dsh: boot a DeepSeek Harness profile — an ordered stack of plugin-bundle patch layers under your own overrides.

Examples:
  dsh --profile web                          boot the web profile (same as: dsh web)
  dsh --profile headless \"run the tests\"     answer one task, print the result, and exit
  dsh --profile tui --patch ./extra.yml      boot a custom profile with one extra overlay
  dsh --profile tui --resume <session>       arguments after the launcher flags reach the app
  dsh --profile web --help                   the web app's own flags and help
  dsh plugin --profile tui add <package>     install a plugin into the tui profile
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

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn boot_resolution_stops_at_the_first_inner_argument() {
        let parsed = parse_dsh_args(&argv(&["--profile", "tui", "--resume", "abc"]), "0.0.1")
            .expect("parsed");
        assert_eq!(
            parsed,
            DshInvocation::Profile(ProfileInvocation {
                profile: "tui".to_string(),
                patches: vec![],
                args: vec!["--resume".to_string(), "abc".to_string()],
            })
        );
    }

    #[test]
    fn web_alias_boots_the_web_profile() {
        let parsed = parse_dsh_args(&argv(&["web", "-h"]), "0.0.1").expect("parsed");
        assert_eq!(
            parsed,
            DshInvocation::Profile(ProfileInvocation {
                profile: "web".to_string(),
                patches: vec![],
                args: vec!["-h".to_string()],
            })
        );
    }

    #[test]
    fn patches_collect_in_argv_order() {
        let parsed = parse_dsh_args(
            &argv(&["--profile", "tui", "--patch", "a.yml", "--patch", "b.yml"]),
            "0.0.1",
        )
        .expect("parsed");
        match parsed {
            DshInvocation::Profile(invocation) => {
                assert_eq!(
                    invocation.patches,
                    vec!["a.yml".to_string(), "b.yml".to_string()]
                );
            }
            other => panic!("expected profile, got {other:?}"),
        }
    }

    #[test]
    fn config_dumps_reject_app_arguments_and_mutual_flags() {
        let error = parse_dsh_args(
            &argv(&["--profile", "web", "--dump-config", "extra"]),
            "0.0.1",
        )
        .expect_err("rejected");
        assert!(error.message.contains("take no app arguments"));

        let error = parse_dsh_args(
            &argv(&["--profile", "web", "--dump-config", "--dump-default-config"]),
            "0.0.1",
        )
        .expect_err("rejected");
        assert!(error.message.contains("mutually exclusive"));
    }

    #[test]
    fn plugin_forwards_pnpm_arguments() {
        let parsed = parse_dsh_args(
            &argv(&["plugin", "--profile", "tui", "add", "some-pkg"]),
            "0.0.1",
        )
        .expect("parsed");
        assert_eq!(
            parsed,
            DshInvocation::Plugin(PluginInvocation {
                profile: "tui".to_string(),
                args: vec!["add".to_string(), "some-pkg".to_string()],
            })
        );
    }

    #[test]
    fn plugin_command_uses_direct_argv_and_the_profile_directory() {
        let home =
            std::env::temp_dir().join(format!("dsh-plugin-command-spec-{}", std::process::id()));
        let profile_dir = home.join("profiles").join("web");
        std::fs::create_dir_all(&profile_dir).expect("profile dir");
        std::fs::write(profile_dir.join("package.json"), "{}\n").expect("profile manifest");
        let invocation = PluginInvocation {
            profile: "web".to_string(),
            args: vec!["add".to_string(), "pkg;not-shell".to_string()],
        };
        let spec = plugin_command_spec(&invocation, &home).expect("command spec");
        assert_eq!(spec.program, "pnpm");
        assert_eq!(spec.args, invocation.args);
        assert_eq!(spec.cwd, profile_dir);
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn a_missing_profile_errors_and_bare_help_exits_zero() {
        let error = parse_dsh_args(&argv(&[]), "0.0.1").expect_err("rejected");
        assert!(error.message.contains("--profile <name> is required"));
        assert_eq!(error.exit_code, 1);

        let help = parse_dsh_args(&argv(&["-h"]), "0.0.1").expect_err("help");
        assert_eq!(help.exit_code, 0);
        assert!(help.message.contains("dsh --profile web"));

        let version = parse_dsh_args(&argv(&["--version"]), "0.0.1").expect_err("version");
        assert_eq!(version.exit_code, 0);
        assert_eq!(version.message, "0.0.1");
    }
}
