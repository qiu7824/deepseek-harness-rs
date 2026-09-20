//! Advisory application diagnostics. Only process and sandbox adapters can
//! produce confirmed infrastructure failures; program text is never authority.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionDiagnostic {
    pub category: &'static str,
    pub confidence: &'static str,
    pub source: &'static str,
}

impl ExecutionDiagnostic {
    pub fn recovery(&self) -> &'static str {
        match self.category {
            "path_probe_access_denied" => {
                "Test-Path failed with access denied; existence is unknown, not false. Do not infer a missing tool or reinstall it. Use environment_probe for host discovery, then environment_validate in the selected context. For optional path checks use -LiteralPath and per-path try/catch, recording denied separately from missing; stop dependent work and request scoped access when required."
            }
            "powershell_script_authorization" => {
                "PowerShell rejected script authorization before the script body ran. In the same selected execution context inspect Get-ExecutionPolicy -List, $ExecutionContext.SessionState.LanguageMode, the script ACL, signature and Zone.Identifier. A host-side probe is not equivalent. Do not infer a syntax error, blindly retry, change global policy or execute the file contents to bypass authorization. For a read-only listing, inspect the script and its data manifest directly when permitted; otherwise request the required access or a trusted signed script."
            }
            "rustup_settings_write_denied" => {
                "Rustup attempted to write user-level settings. Check the selected toolchain and execution permissions with environment_validate; use an explicitly configured installed compiler when appropriate. If a settings change is required, request approval for that operation; do not grant broad home-directory access or repeat the same failing probe."
            }
            "git_config_access_denied" => {
                "Git cannot read its user configuration. Check the selected execution environment and approved readable paths. Request access to the required configuration if needed; do not silently ignore global configuration or modify home-directory ACLs."
            }
            "git_repository_ownership" => {
                "Git rejected repository ownership. Verify the exact repository path and owner with the user before trusting it. Do not set safe.directory=* or disable ownership checks globally."
            }
            "native_shell_interop" => {
                "Use execute_native with literal argv to distinguish the native exit status from PowerShell stderr redirection. This does not fix filesystem permissions. Use execute_steps for dependent commands; inspect prior effects before retrying."
            }
            "access_denied" => {
                "Check whether the actual target is inside the selected workspace and approved paths. A readable host path may still be inaccessible in the selected execution environment. Request the needed permission through the approval flow; do not automatically escalate or retry unchanged."
            }
            _ => {
                "Inspect the application error and validate the relevant capability before retrying."
            }
        }
    }
}

pub fn application_diagnostics(exit_code: Option<i32>, stderr: &str) -> Vec<ExecutionDiagnostic> {
    if exit_code == Some(0) {
        return Vec::new();
    }
    let text = stderr.to_ascii_lowercase();
    let mut result = Vec::new();
    let mut add = |category| {
        result.push(ExecutionDiagnostic {
            category,
            confidence: "suspected",
            source: "application_stderr",
        })
    };
    let console = text.contains("readconsoleoutput") || text.contains("hostexception");
    if text.contains("authorizationmanager")
        || text.contains("pssecurityexception")
        || text.contains("running scripts is disabled")
        || text.contains("禁止运行脚本")
    {
        add("powershell_script_authorization");
    }
    if console {
        add("console_io");
    }
    if crate::ShellSandboxInfo::com_access_denied(stderr) {
        add("com_activation");
    }
    if text.contains("dll load failed")
        || text.contains("dllnotfound")
        || text.contains("cannot open shared object")
    {
        add("dependency_load");
    }
    if text.contains("commandnotfound")
        || text.contains("command not found")
        || text.contains("not recognized")
    {
        add("command_discovery");
    }
    if text.contains("filenotfound")
        || text.contains("no such file")
        || text.contains("cannot find path")
    {
        add("file_missing");
    }
    if text.contains("access is denied")
        || text.contains("permission denied")
        || text.contains("unauthorizedaccess")
        || text.contains("拒绝访问")
        || text.contains("os error 5")
    {
        // Preserve the clue alongside other errors, without calling it a file
        // sandbox denial or discarding all denials because one COM error exists.
        add("access_denied");
    }
    let access_denied = text.contains("permission denied")
        || text.contains("access is denied")
        || text.contains("拒绝访问")
        || text.contains("os error 5");
    if access_denied && text.contains("settings.toml") && text.contains("rustup") {
        add("rustup_settings_write_denied");
    }
    if access_denied && text.contains(".gitconfig") {
        add("git_config_access_denied");
    }
    if access_denied
        && (text.contains("test-path") || text.contains("itemexistsunauthorizedaccesserror"))
    {
        add("path_probe_access_denied");
    }
    if text.contains("detected dubious ownership") {
        add("git_repository_ownership");
    }
    if text.contains("!_src.empty()") || text.contains("imdecode") {
        add("image_decode");
    }
    if text.contains("could not be broadcast") {
        add("array_shape");
    }
    if text.contains("nativecommanderror") || text.contains("nativecommandexitexception") {
        add("native_shell_interop");
    }
    result
}

/// Native applications receive their argv directly. Batch files have a
/// different parsing contract and require an explicit shell adapter.
pub fn validate_native_argv(argv: &[String]) -> Result<(), String> {
    let Some(program) = argv.first().filter(|value| !value.trim().is_empty()) else {
        return Err("[TOOL_INPUT_INVALID] native program must be non-empty".into());
    };
    if argv.iter().any(|value| value.contains('\0')) {
        return Err("[TOOL_INPUT_INVALID] native argv must not contain NUL".into());
    }
    if cfg!(windows)
        && std::path::Path::new(program)
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
            })
    {
        return Err("[TOOL_INPUT_INVALID] .cmd/.bat require an explicit shell adapter; execute the underlying .exe for literal argv".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn user_configuration_failures_have_scoped_recovery() {
        for (stderr, category) in [
            (
                "Test-Path : Access is denied; ItemExistsUnauthorizedAccessError",
                "path_probe_access_denied",
            ),
            (
                "ai-context.ps1 : AuthorizationManager 检查失败。 FullyQualifiedErrorId : UnauthorizedAccess",
                "powershell_script_authorization",
            ),
            (
                "could not write settings file: C:\\Users\\xs\\.rustup\\settings.toml: 拒绝访问。 (os error 5)",
                "rustup_settings_write_denied",
            ),
            (
                "git : warning: unable to access 'C:/Users/xs/.gitconfig': Permission denied",
                "git_config_access_denied",
            ),
            (
                "fatal: detected dubious ownership in repository",
                "git_repository_ownership",
            ),
        ] {
            let entries = application_diagnostics(Some(1), stderr);
            let entry = entries
                .iter()
                .find(|entry| entry.category == category)
                .unwrap();
            assert_eq!(entry.confidence, "suspected");
            assert!(!entry.recovery().is_empty());
            assert!(application_diagnostics(Some(0), stderr).is_empty());
        }
    }
    #[test]
    fn successful_com_and_runner_examples_are_not_failures() {
        assert!(
            application_diagnostics(
                Some(0),
                "CLSID 80070005 COMObject access is denied\nbwrap: error"
            )
            .is_empty()
        );
    }
    #[test]
    fn multiple_application_errors_are_advisory_and_independent() {
        let diagnostics = application_diagnostics(
            Some(1),
            "ReadConsoleOutput HostException access is denied\nCLSID 80070005 COMObject\nFileNotFoundError",
        );
        for category in [
            "console_io",
            "com_activation",
            "access_denied",
            "file_missing",
        ] {
            assert!(diagnostics.iter().any(|entry| entry.category == category));
        }
        assert!(
            diagnostics
                .iter()
                .all(|entry| entry.confidence == "suspected")
        );
    }
    #[test]
    fn direct_argv_preserves_empty_quoted_and_unicode_arguments() {
        assert!(
            validate_native_argv(&["python".into(), "".into(), "双引号\"和$变量".into()]).is_ok()
        );
        assert!(validate_native_argv(&[]).is_err());
        assert!(validate_native_argv(&["python".into(), "a\0b".into()]).is_err());
    }
}
