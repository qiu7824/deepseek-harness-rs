from __future__ import annotations

import json
import pathlib
import re
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
LAUNCHER = ROOT / "crates" / "host" / "dsh-launcher" / "src" / "main.rs"
PACKAGE = ROOT / "tools" / "package_release.py"
VERIFIER = ROOT / "tools" / "verify_release_package.py"
WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
INSTALLER = ROOT / "packaging" / "windows" / "deepseek-harness-rs.iss"
SKIN_CENTER = ROOT / "release" / "plugins" / "dsh-skin-center" / "lib" / "client.js"
SKINS = ROOT / "web" / "dist" / "skins"


def workflow_step(workflow: str, name: str) -> str:
    marker = f"      - name: {name}\n"
    start = workflow.index(marker)
    end = workflow.find("\n      - ", start + len(marker))
    return workflow[start:] if end < 0 else workflow[start:end]


class LauncherReleaseContractTests(unittest.TestCase):
    def test_launcher_exposes_the_complete_desktop_contract(self):
        source = LAUNCHER.read_text(encoding="utf-8")
        for required in (
            "CreateMutexW",
            "ERROR_ALREADY_EXISTS",
            "libc::flock",
            "libc::LOCK_EX | libc::LOCK_NB",
            "unix_single_instance_lock_rejects_contention_and_reopens_after_drop",
            "SetForegroundWindow",
            "TRAY_AUTOSTART_COMMAND",
            "TRAY_CHECK_UPDATE_COMMAND",
            ".icon_path(icon_path)",
            "launcher_icon_path()",
            "dsh_home_paths::default_dsh_home",
            "LauncherCommand::SetAutostart",
            "LauncherCommand::CheckUpdate",
            "CARGO_PKG_VERSION",
            "https://api.github.com/repos/qiu7824/deepseek-harness-rs/releases",
        ):
            self.assertIn(required, source)
        self.assertIn("tray_menu_spec(", source)
        self.assertIn("TRAY_AUTOSTART_COMMAND", source)
        self.assertIn("copy.check_update", source)
        self.assertIn("TRAY_QUIT_COMMAND", source)
        self.assertIn("ZsuiCommand::ShowMainWindow", source)
        self.assertIn("ZsuiCommand::Quit", source)
        for section_title in (
            'service: "服务"',
            'preferences: "启动与更新"',
            'last_action: "最近操作"',
        ):
            self.assertIn(section_title, source)
        for forbidden in (
            "TRAY_OPEN_LOGS_COMMAND",
            "LauncherCommand::OpenLogs",
            "LauncherCommand::InstallSkins",
            "Message::OpenLogs",
            "Message::InstallSkins",
            "button(state.copy.open_logs)",
            "button(state.copy.install_skins)",
        ):
            self.assertNotIn(forbidden, source)
        self.assertNotIn("copy.open_logs", source)
        self.assertNotIn("copy.install_skins", source)
        windows = (
            ROOT / "crates" / "vendor" / "zsui" / "src" / "platform" / "windows" / "window.rs"
        ).read_text(encoding="utf-8")
        self.assertIn('wide_null("ZsuiMainWindow")', source)
        self.assertIn("FindWindowW(class_name.as_ptr(), title.as_ptr())", source)
        self.assertIn("WindowsWindowRole::Quick", windows)
        self.assertIn("ShowWindow(quick, SW_HIDE)", windows)
        application = (
            ROOT
            / "crates"
            / "vendor"
            / "zsui"
            / "src"
            / "platform"
            / "windows"
            / "application.rs"
        ).read_text(encoding="utf-8")
        tray = (
            ROOT
            / "crates"
            / "vendor"
            / "zsui"
            / "src"
            / "platform"
            / "windows"
            / "services"
            / "tray.rs"
        ).read_text(encoding="utf-8")
        self.assertIn("clear_windows_win32_status_item_routes", application)
        self.assertIn("dispatch_windows_win32_status_item_callback", tray)
        self.assertIn("restore_windows_win32_status_items", tray)

    def test_retained_skin_catalog_matches_the_user_selection(self):
        expected = {
            "blue-fantasy",
            "deepseek-official",
            "harbor",
            "miku",
            "minecraft",
            "trading",
            "xp",
        }
        actual = {path.name for path in SKINS.iterdir() if path.is_dir()}
        self.assertEqual(actual, expected)

        catalog = SKIN_CENTER.read_text(encoding="utf-8")
        for skin in expected:
            self.assertIn(f'id: "{skin}"', catalog)
        for removed in ("whale-song", "dragon-heir"):
            self.assertNotIn(f'id: "{removed}"', catalog)

    def test_official_skin_is_the_default_of_the_skin_variant(self):
        official = json.loads(
            (SKINS / "deepseek-official" / "skin.json").read_text(encoding="utf-8")
        )
        self.assertEqual(official["source"], "https://www.deepseek.com/harness/")
        package = PACKAGE.read_text(encoding="utf-8")
        verifier = VERIFIER.read_text(encoding="utf-8")
        self.assertIn('default_skin = "deepseek-official"', package)
        self.assertIn('manifest.get("default_skin")', verifier)

    def test_package_defaults_use_the_real_host_schema_and_preserve_user_settings(self):
        package = PACKAGE.read_text(encoding="utf-8")
        host = (ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs").read_text(
            encoding="utf-8"
        )
        verifier = VERIFIER.read_text(encoding="utf-8")
        self.assertIn("settings.defaults.json", package)
        self.assertIn('{"ui-theme": {"preference": default_skin}}', package)
        self.assertNotIn('{"ui-theme": {"skin": default_skin}}', package)
        self.assertIn('packaged_resource("settings.defaults.json")', host)
        self.assertIn("merge_package_defaults", host)
        self.assertIn("settings.defaults.json", verifier)

    def test_windows_variants_have_distinct_installer_and_shortcut_identity(self):
        installer = INSTALLER.read_text(encoding="utf-8")
        app_ids = re.findall(r'#define MyAppId "([^"]+)"', installer)
        self.assertEqual(len(app_ids), 3)
        self.assertEqual(len(set(app_ids)), 3)
        self.assertIn('#define MyAppName "DeepSeek Harness-rs (" + MyVariantDisplay + ")"', installer)
        self.assertIn("DefaultGroupName={#MyAppName}", installer)
        self.assertIn('Name: "{group}\\{#MyAppName}"', installer)
        self.assertIn('Name: "{autodesktop}\\{#MyAppName}"', installer)

    def test_release_pipeline_builds_core_skin_and_free_variants(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        installer = INSTALLER.read_text(encoding="utf-8")
        for variant in ("core", "skin", "free"):
            self.assertIn(f"--variant {variant}", workflow)
            self.assertIn(f'Variant == "{variant}"', installer)
            self.assertIn(f'linux-x86_64-${{variant}}', workflow)
            self.assertIn(f'macos-${{{{ matrix.arch }}}}-${{variant}}', workflow)
        self.assertIn("ling-3.0-flash-fin-free", PACKAGE.read_text(encoding="utf-8"))
        self.assertIn("ling-3.0-flash-fin-free", VERIFIER.read_text(encoding="utf-8"))
        self.assertIn('stage / "deepseek-black.ico"', PACKAGE.read_text(encoding="utf-8"))
        self.assertIn('prefix + "deepseek-black.ico"', VERIFIER.read_text(encoding="utf-8"))
        gate = workflow_step(workflow, "版本与产品门禁")
        self.assertIn("cargo test --locked -p dsh-launcher", gate)
        self.assertNotIn("if:", gate)

    def test_workflow_step_extracts_only_one_named_step(self):
        workflow = """jobs:
  build:
    steps:
      - name: 版本与产品门禁
        run: |
          echo gate
      - name: 旁路步骤
        run: cargo test --locked -p dsh-launcher
      - name: 构建正式二进制
        run: cargo build
"""
        gate = workflow_step(workflow, "版本与产品门禁")
        self.assertIn("echo gate", gate)
        self.assertNotIn("旁路步骤", gate)
        self.assertNotIn("cargo test --locked -p dsh-launcher", gate)

    def test_launcher_uses_only_supported_native_desktop_services(self):
        source = LAUNCHER.read_text(encoding="utf-8")
        linux = (
            ROOT
            / "crates"
            / "vendor"
            / "zsui"
            / "src"
            / "platform"
            / "desktop_runtime"
            / "linux_direct.rs"
        ).read_text(encoding="utf-8")
        self.assertIn("let builder = NativeWindowBuilder::new(copy.title)", source)
        self.assertIn(
            '#[cfg(not(target_os = "linux"))]\n    let builder = builder.tray(tray);',
            source,
        )
        self.assertIn(
            '#[cfg(target_os = "linux")]\n    let close_command = ZsuiCommand::Quit;',
            source,
        )
        self.assertIn(
            '#[cfg(not(target_os = "linux"))]\n    let close_command = ZsuiCommand::HideMainWindow;',
            source,
        )
        self.assertIn(".on_close_requested(close_command)", source)
        self.assertIn(
            '#[cfg(target_os = "linux")]\n    let initial_window_visible = true;',
            source,
        )
        self.assertIn(
            '#[cfg(not(target_os = "linux"))]\n    let initial_window_visible = !background;',
            source,
        )
        self.assertIn(".visible(initial_window_visible)", source)
        self.assertIn("if !request.trays.is_empty()", linux)

    def test_launcher_mutable_files_live_under_the_user_home(self):
        source = LAUNCHER.read_text(encoding="utf-8")
        self.assertIn("fn launcher_runtime_root(root: &Path) -> PathBuf", source)
        self.assertIn('home.join("launcher")', source)
        self.assertIn("launcher_log_dir(&self.root)", source)
        self.assertNotIn('self.root.join("logs")', source)


if __name__ == "__main__":
    unittest.main()
