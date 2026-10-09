//! Host-owned event notifications; never exposed through the payload's env.
use std::sync::OnceLock;
use windows_sys::Win32::{
    Foundation::CloseHandle,
    System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent},
};

static EVENT: OnceLock<Option<String>> = OnceLock::new();

pub fn configure(name: Option<&str>) {
    let _ = EVENT.set(
        name.filter(|name| name.starts_with("Local\\DSH-Sandbox-Ready-"))
            .map(str::to_owned),
    );
}

pub(crate) fn event_name() -> Option<String> {
    EVENT.get().and_then(Clone::clone)
}

fn notify(suffix: &str) {
    let Some(name) = EVENT.get().and_then(Option::as_ref) else {
        return;
    };
    let wide: Vec<_> = format!("{name}-{suffix}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    unsafe {
        // Older Hosts do not create these optional events. Their absence is
        // not a reason to bypass preparation or fail an otherwise valid run.
        let handle = OpenEventW(EVENT_MODIFY_STATE, 0, wide.as_ptr());
        if handle != 0 {
            SetEvent(handle);
            CloseHandle(handle);
        }
    }
}

pub fn stage(phase: &str) {
    notify(&format!("stage-{phase}"));
    progress();
}

pub fn progress() {
    notify("progress");
}
