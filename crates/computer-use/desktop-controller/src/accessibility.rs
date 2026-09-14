//! UI Automation lives on the controller's MTA thread, outside the Host.
//! Provider calls and tree traversal are bounded; hung providers can be
//! terminated with the owning controller process by the Host transport.
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::HWND,
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
        UI::Accessibility::*,
    },
    core::{BSTR, Interface},
};

struct Entry {
    element: IUIAutomationElement,
    name: String,
    kind: i32,
    bounds: (i32, i32, i32, i32),
}
pub struct Accessibility {
    automation: IUIAutomation,
    entries: Vec<Entry>,
    snapshot: String,
}
fn failure(error: windows::core::Error) -> String {
    format!("COMPUTER_USE_ACCESSIBILITY: {error}")
}
impl Accessibility {
    pub fn new() -> Result<Self, String> {
        let automation: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
                .map_err(failure)?;
        if let Ok(bounded) = automation.cast::<IUIAutomation2>() {
            unsafe {
                bounded.SetConnectionTimeout(1000).map_err(failure)?;
                bounded.SetTransactionTimeout(1000).map_err(failure)?;
            }
        }
        Ok(Self {
            automation,
            entries: Vec::new(),
            snapshot: String::new(),
        })
    }
    pub fn invalidate(&mut self) {
        self.entries.clear();
        self.snapshot.clear();
    }
    pub fn observe(
        &mut self,
        window: usize,
        snapshot: String,
        cancelled: impl Fn() -> bool,
    ) -> Result<Value, String> {
        self.invalidate();
        let root =
            unsafe { self.automation.ElementFromHandle(HWND(window as _)) }.map_err(failure)?;
        let walker = unsafe { self.automation.ControlViewWalker() }.map_err(failure)?;
        let started = Instant::now();
        let mut queue = VecDeque::from([(root, 0usize, None)]);
        let mut rows = Vec::new();
        while let Some((element, depth, parent)) = queue.pop_front() {
            if cancelled() {
                self.invalidate();
                return Err("COMPUTER_USE_ABORTED".into());
            }
            if started.elapsed() > Duration::from_secs(3) || self.entries.len() >= 512 {
                break;
            }
            let password = unsafe { element.CurrentIsPassword() }
                .map(|b| b.as_bool())
                .unwrap_or(true);
            let name = if password {
                "Protected field".to_string()
            } else {
                unsafe { element.CurrentName() }
                    .map(|v| v.to_string().chars().take(512).collect())
                    .unwrap_or_default()
            };
            let kind = unsafe { element.CurrentControlType() }.map_err(failure)?.0;
            let r = unsafe { element.CurrentBoundingRectangle() }.map_err(failure)?;
            let enabled = unsafe { element.CurrentIsEnabled() }
                .map(|v| v.as_bool())
                .unwrap_or(false);
            let index = self.entries.len();
            let mut actions = Vec::new();
            if unsafe {
                element.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
            }
            .is_ok()
            {
                actions.push("invoke");
            }
            if !password
                && unsafe {
                    element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                }
                .is_ok()
            {
                actions.push("set_value");
            }
            if unsafe {
                element.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                    UIA_SelectionItemPatternId,
                )
            }
            .is_ok()
            {
                actions.push("select");
            }
            if unsafe {
                element.GetCurrentPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId)
            }
            .is_ok()
            {
                actions.push("scroll_element");
            }
            rows.push(json!({"elementId":index,"parent":parent,"name":name,"controlType":kind,"enabled":enabled,"protected":password,"bounds":{"left":r.left,"top":r.top,"width":r.right-r.left,"height":r.bottom-r.top},"actions":actions}));
            if depth < 32 {
                let mut child = unsafe { walker.GetFirstChildElement(&element) };
                while let Ok(value) = child {
                    if queue.len() + self.entries.len() >= 512
                        || started.elapsed() > Duration::from_secs(3)
                        || cancelled()
                    {
                        break;
                    }
                    child = unsafe { walker.GetNextSiblingElement(&value) };
                    queue.push_back((value, depth + 1, Some(index)));
                }
            }
            self.entries.push(Entry {
                element,
                name,
                kind,
                bounds: (r.left, r.top, r.right, r.bottom),
            });
        }
        self.snapshot = snapshot;
        Ok(
            json!({"snapshotId":self.snapshot,"elements":rows,"truncated":!queue.is_empty(),"maxNodes":512,"maxDepth":32}),
        )
    }
    pub fn act(&mut self, args: &Value) -> Result<(), String> {
        if self.snapshot.is_empty() || args["snapshotId"].as_str() != Some(self.snapshot.as_str()) {
            return Err("COMPUTER_USE_STALE_SNAPSHOT".into());
        }
        let index = args["elementId"]
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .ok_or("COMPUTER_USE_ELEMENT_REQUIRED")?;
        let entry = self
            .entries
            .get(index)
            .ok_or("COMPUTER_USE_STALE_ELEMENT")?;
        unsafe {
            if entry
                .element
                .CurrentIsPassword()
                .map_err(failure)?
                .as_bool()
            {
                return Err("COMPUTER_USE_PROTECTED_ELEMENT".into());
            }
            let r = entry.element.CurrentBoundingRectangle().map_err(failure)?;
            if (r.left, r.top, r.right, r.bottom) != entry.bounds
                || entry.element.CurrentControlType().map_err(failure)?.0 != entry.kind
                || entry
                    .element
                    .CurrentName()
                    .map_err(failure)?
                    .to_string()
                    .chars()
                    .take(512)
                    .collect::<String>()
                    != entry.name
            {
                return Err("COMPUTER_USE_STALE_ELEMENT".into());
            }
            if !entry.element.CurrentIsEnabled().map_err(failure)?.as_bool() {
                return Err("COMPUTER_USE_ELEMENT_DISABLED".into());
            }
            match args["action"].as_str().unwrap_or("") {
                "invoke" => entry
                    .element
                    .GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId)
                    .map_err(failure)?
                    .Invoke()
                    .map_err(failure)?,
                "set_value" => {
                    let text = args["text"]
                        .as_str()
                        .filter(|v| v.len() <= 65536)
                        .ok_or("COMPUTER_USE_INVALID_TEXT")?;
                    entry
                        .element
                        .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                        .map_err(failure)?
                        .SetValue(&BSTR::from(text))
                        .map_err(failure)?;
                }
                "select" => entry
                    .element
                    .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    )
                    .map_err(failure)?
                    .Select()
                    .map_err(failure)?,
                "scroll_element" => {
                    let amount = match args["direction"].as_str() {
                        Some("up") => ScrollAmount_SmallDecrement,
                        Some("down") => ScrollAmount_SmallIncrement,
                        _ => return Err("COMPUTER_USE_SCROLL_DIRECTION".into()),
                    };
                    entry
                        .element
                        .GetCurrentPatternAs::<IUIAutomationScrollPattern>(UIA_ScrollPatternId)
                        .map_err(failure)?
                        .Scroll(ScrollAmount_NoAmount, amount)
                        .map_err(failure)?;
                }
                _ => return Err("COMPUTER_USE_UNSUPPORTED_ACTION".into()),
            }
        }
        self.invalidate();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_edit_control_and_snapshot_invalidation() {
        use windows_sys::Win32::{
            Foundation::*, System::Threading::GetCurrentThreadId, UI::WindowsAndMessaging::*,
        };
        let (send, receive) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || unsafe {
            let wide = |value: &str| value.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            let window = CreateWindowExW(
                0,
                wide("STATIC").as_ptr(),
                wide("Harness accessibility regression").as_ptr(),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                20,
                20,
                360,
                160,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!window.is_null());
            let edit = CreateWindowExW(
                0,
                wide("EDIT").as_ptr(),
                wide("before").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP,
                10,
                10,
                260,
                30,
                window,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            assert!(!edit.is_null());
            send.send((window as usize, GetCurrentThreadId())).unwrap();
            let mut message = MSG::default();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            DestroyWindow(window);
        });
        let (window, thread_id) = receive.recv_timeout(Duration::from_secs(5)).unwrap();
        struct Stop(u32);
        impl Drop for Stop {
            fn drop(&mut self) {
                unsafe {
                    PostThreadMessageW(self.0, WM_QUIT, 0, 0);
                }
            }
        }
        let stop = Stop(thread_id);
        unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            )
            .ok()
            .unwrap();
        }
        {
            let mut automation = Accessibility::new().unwrap();
            let state = automation
                .observe(window, "test:1".into(), || false)
                .unwrap();
            let id = state["elements"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["controlType"] == UIA_EditControlTypeId.0)
                .unwrap()["elementId"]
                .as_u64()
                .unwrap();
            automation.act(&json!({"action":"set_value","snapshotId":"test:1","elementId":id,"text":"测试字段"})).unwrap();
            assert_eq!(automation.act(&json!({"action":"set_value","snapshotId":"test:1","elementId":id,"text":"stale"})).unwrap_err(),"COMPUTER_USE_STALE_SNAPSHOT");
            automation
                .observe(window, "test:2".into(), || false)
                .unwrap();
            let edit = automation
                .entries
                .iter()
                .find(|entry| entry.kind == UIA_EditControlTypeId.0)
                .unwrap();
            let value = unsafe {
                edit.element
                    .GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
                    .unwrap()
                    .CurrentValue()
                    .unwrap()
                    .to_string()
            };
            assert_eq!(value, "测试字段");
        }
        unsafe {
            windows::Win32::System::Com::CoUninitialize();
        }
        drop(stop);
        thread.join().unwrap();
    }
}
