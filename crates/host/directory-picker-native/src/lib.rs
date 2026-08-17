//! Native backend of the directory-picker seam: registers
//! `ctx.directoryPicker` with the `native` capability — each pick opens one
//! OS directory chooser on the host's display. Rust port of
//! `packages/host/directory-picker-native` (macOS `osascript`, Linux
//! `zenity` with `kdialog` fallback; the Windows `IFileOpenDialog` COM
//! dialog arrives with the win32-dialog milestone).
//!
//! # Deviations
//!
//! - Windows picks answer `None` until the COM dialog milestone (a
//!   picker-unavailable posture would be wrong — the capability advertises
//!   native, the dialog itself is the deferred half).
//! - The TS injectable runner seam collapses to the
//!   [`dsh_native_command::run_native_command`] boundary.

use std::sync::Arc;

use async_trait::async_trait;
use cordis::{ArcValue, Context, InjectSpec, Plugin, PluginError, arc};
use dsh_host_directory_picker::{
    AbortSignal, DirectoryPicker, DirectoryPickerCapability,
    DirectoryPickerNativeCapability, register,
};
use futures::future::BoxFuture;

/// Cordis plugin name.
pub const NAME: &str = "host-directory-picker-native";

/// Open the macOS chooser through `osascript` (the TS native-picker path).
async fn pick_macos(signal: &AbortSignal) -> Option<String> {
    let abort = signal.clone();
    let signal_flag: dsh_native_command::NativeCommandAbort =
        Arc::new(move || abort.aborted());
    let script = "POSIX path of (choose folder with prompt \"Select workspace directory\")";
    match dsh_native_command::run_native_command(
        "osascript",
        &["-e".to_string(), script.to_string()],
        Some(signal_flag),
    )
    .await
    {
        Ok(output) => {
            let path = output.stdout.trim();
            if path.is_empty() {
                None
            } else {
                Some(path.to_string())
            }
        }
        // Operator cancel exits non-zero with no output: None (the TS
        // picker maps the same rejection to cancellation).
        Err(_) => None,
    }
}

/// Open the Linux chooser through `zenity` (kdialog fallback; the TS
/// native-picker path).
async fn pick_linux(signal: &AbortSignal) -> Option<String> {
    for binary in ["zenity", "kdialog"] {
        let abort = signal.clone();
        let signal_flag: dsh_native_command::NativeCommandAbort =
            Arc::new(move || abort.aborted());
        let args: Vec<String> = match binary {
            "zenity" => vec![
                "--file-selection".to_string(),
                "--directory".to_string(),
                "--title=Select workspace directory".to_string(),
            ],
            _ => vec![
                "--getexistingdirectory".to_string(),
                std::env::var("HOME").unwrap_or_default(),
            ],
        };
        match dsh_native_command::run_native_command(binary, &args, Some(signal_flag)).await {
            Ok(output) => {
                let path = output.stdout.trim();
                if !path.is_empty() {
                    return Some(path.to_string());
                }
                // zenity answered with no path: fall through to kdialog.
            }
            // Missing binary (ENOENT) or operator cancel: try the fallback,
            // then give up.
            Err(_) => {}
        }
    }
    None
}

/// The `ctx.directoryPicker` native implementation (stable capability
/// object per service life).
pub struct NativeDirectoryPicker {
    capability: DirectoryPickerCapability,
}

impl DirectoryPicker for NativeDirectoryPicker {
    fn capability(&self) -> DirectoryPickerCapability {
        self.capability.clone()
    }
}

impl NativeDirectoryPicker {
    /// Construct an unregistered backend; `install` registers it as
    /// `ctx.directoryPicker`.
    pub fn new() -> Arc<Self> {
        let pick: Arc<
            dyn Fn(AbortSignal) -> BoxFuture<'static, Option<String>> + Send + Sync,
        > = Arc::new(move |signal: AbortSignal| {
            Box::pin(async move {
                if cfg!(target_os = "macos") {
                    pick_macos(&signal).await
                } else if cfg!(target_os = "linux") {
                    pick_linux(&signal).await
                } else {
                    // Windows: the IFileOpenDialog COM dialog arrives with
                    // the win32-dialog milestone; picking is unavailable
                    // until then.
                    None
                }
            })
        });
        Arc::new(Self {
            capability: DirectoryPickerCapability::Native(DirectoryPickerNativeCapability::new(pick)),
        })
    }

    /// Construct and register as `ctx.directoryPicker` (TS constructor +
    /// `super(ctx, 'directoryPicker')`).
    pub fn install(ctx: &Context) -> Arc<Self> {
        let backend = Self::new();
        register(ctx, backend.clone());
        backend
    }
}

/// The Cordis plugin form.
pub struct NativeDirectoryPickerPlugin;

#[async_trait]
impl Plugin for NativeDirectoryPickerPlugin {
    fn name(&self) -> Option<&'static str> {
        Some(NAME)
    }

    fn inject(&self) -> InjectSpec {
        InjectSpec::new([])
    }

    async fn apply(&self, ctx: &Context, _config: ArcValue) -> Result<(), PluginError> {
        NativeDirectoryPicker::install(ctx);
        Ok(())
    }
}

#[allow(unused)]
fn _vocab() {
    let _ = arc(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_the_native_capability() {
        let backend = NativeDirectoryPicker::new();
        assert_eq!(backend.capability().kind(), "native");
    }
}
