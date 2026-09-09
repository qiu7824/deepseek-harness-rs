//! The common Windows folder dialog runs in a cancellable, hidden helper.
use dsh_host_directory_picker::AbortSignal;
use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

static HOST: OnceLock<PathBuf> = OnceLock::new();

pub fn register_embedded_windows_picker() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    if let Some(previous) = HOST.get() {
        return if previous == &executable {
            Ok(())
        } else {
            Err("directory picker host already registered".into())
        };
    }
    HOST.set(executable)
        .map_err(|_| "directory picker registration raced".into())
}

pub(super) async fn pick(signal: &AbortSignal) -> Option<String> {
    let executable = HOST.get()?;
    let signal = signal.clone();
    let abort: dsh_native_command::NativeCommandAbort = Arc::new(move || signal.aborted());
    let result = dsh_native_command::run_native_command(
        &executable.to_string_lossy(),
        &["__dsh-directory-picker".into()],
        Some(abort),
    )
    .await
    .ok()?;
    let path = result.stdout.trim_end_matches(['\r', '\n']);
    (!path.is_empty()).then(|| path.to_string())
}

/// Called before creating an async runtime, on the helper's main STA thread.
pub fn run_windows_picker() -> Result<Option<String>, String> {
    use ::windows::{
        Win32::{
            Foundation::ERROR_CANCELLED,
            System::Com::{
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
            },
            UI::Shell::{
                FOS_DONTADDTORECENT, FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PICKFOLDERS,
                FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH,
            },
        },
        core::{HRESULT, w},
    };
    struct Apartment;
    impl Drop for Apartment {
        fn drop(&mut self) {
            unsafe {
                CoUninitialize();
            }
        }
    }
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
            .ok()
            .map_err(|error| error.to_string())?;
        let _apartment = Apartment;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| error.to_string())?;
        let options = dialog.GetOptions().map_err(|error| error.to_string())?;
        dialog
            .SetOptions(
                options
                    | FOS_PICKFOLDERS
                    | FOS_FORCEFILESYSTEM
                    | FOS_NOCHANGEDIR
                    | FOS_DONTADDTORECENT,
            )
            .map_err(|error| error.to_string())?;
        dialog
            .SetTitle(w!("DeepSeek Harness"))
            .map_err(|error| error.to_string())?;
        if let Err(error) = dialog.Show(None) {
            return if error.code() == HRESULT::from_win32(ERROR_CANCELLED.0) {
                Ok(None)
            } else {
                Err(error.to_string())
            };
        }
        let item = dialog.GetResult().map_err(|error| error.to_string())?;
        let path = item
            .GetDisplayName(SIGDN_FILESYSPATH)
            .map_err(|error| error.to_string())?;
        let selected = path.to_string().map_err(|error| error.to_string());
        CoTaskMemFree(Some(path.0.cast()));
        Ok(Some(selected?))
    }
}
