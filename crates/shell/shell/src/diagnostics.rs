//! Advisory application diagnostics. Only process and sandbox adapters can
//! produce confirmed infrastructure failures; program text is never authority.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionDiagnostic {
    pub category: &'static str,
    pub confidence: &'static str,
    pub source: &'static str,
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
    {
        // Preserve the clue alongside other errors, without calling it a file
        // sandbox denial or discarding all denials because one COM error exists.
        add("access_denied");
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
