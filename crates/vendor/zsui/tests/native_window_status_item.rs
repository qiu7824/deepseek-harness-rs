use zsui::{native_window, Command, TraySpec};

#[test]
fn native_window_builder_attaches_a_status_item_to_the_runtime_app() {
    let tray = TraySpec::new()
        .tooltip("Lifecycle test")
        .item("Open", Command::ShowMainWindow)
        .item("Quit", Command::Quit);

    let app = native_window("Lifecycle test")
        .tray(tray.clone())
        .build()
        .expect("native window with status item should build");

    assert_eq!(app.tray, Some(tray));
}
