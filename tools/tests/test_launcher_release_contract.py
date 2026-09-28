from __future__ import annotations

import json
import pathlib
import re
import shlex
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

    def test_skins_are_retired_from_packages_and_updates(self):
        self.assertFalse(SKINS.exists())
        self.assertFalse(SKIN_CENTER.exists())
        self.assertFalse((ROOT / "crates" / "host" / "dsh-skin-installer").exists())
        package = PACKAGE.read_text(encoding="utf-8")
        verifier = VERIFIER.read_text(encoding="utf-8")
        self.assertNotIn("default_skin", package)
        self.assertNotIn("default_skin", verifier)
        updater = (ROOT / "crates" / "host" / "dsh-launcher" / "src" / "updater.rs").read_text(encoding="utf-8")
        self.assertIn('"core" | "skin" => "core".to_string(),', updater)

    def test_package_defaults_use_the_real_host_schema_and_preserve_user_settings(self):
        package = PACKAGE.read_text(encoding="utf-8")
        host = (ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs").read_text(
            encoding="utf-8"
        )
        verifier = VERIFIER.read_text(encoding="utf-8")
        self.assertNotIn("settings.defaults.json", package)
        self.assertIn('packaged_resource("settings.defaults.json")', host)
        self.assertIn("merge_package_defaults", host)
        self.assertIn("settings.defaults.json", verifier)

    def test_windows_variants_have_distinct_installer_and_shortcut_identity(self):
        installer = INSTALLER.read_text(encoding="utf-8")
        app_ids = re.findall(r'#define MyAppId "([^"]+)"', installer)
        self.assertEqual(len(app_ids), 1)
        self.assertEqual(app_ids[0], "{{A6F42843-79DD-4FA1-91D2-0B71F8974B78}")
        self.assertIn('#define MyAppName "DeepSeek Harness-rs (" + MyVariantDisplay + ")"', installer)
        self.assertIn("DefaultGroupName={#MyAppName}", installer)
        self.assertIn('Name: "{group}\\{#MyAppName}"', installer)
        self.assertIn('Name: "{autodesktop}\\{#MyAppName}"', installer)

    def test_installers_share_the_one_screen_surface_and_default_to_drive_d(self):
        windows = ROOT / "packaging" / "windows"
        desktop_path = windows / "deepseek-harness-desktop-core.iss"
        ui_path = windows / "installer" / "installer-ui.iss"
        for path in (INSTALLER, desktop_path, ui_path):
            self.assertTrue(path.read_bytes().startswith(b"\xef\xbb\xbf"), f"{path.name} needs a UTF-8 BOM for Inno 6.1.2")
        web = INSTALLER.read_text(encoding="utf-8-sig")
        desktop = desktop_path.read_text(encoding="utf-8-sig")
        ui = ui_path.read_text(encoding="utf-8-sig")
        self.assertIn("DefaultDirName=D:\\Program Files (x86)\\DeepSeek Harness-rs\\{#Variant}", web)
        self.assertIn("DefaultDirName=D:\\Program Files (x86)\\DeepSeek Harness-rs\\desktop", desktop)
        for script in (web, desktop):
            include = script.index('#include ArtDir + "\\installer-ui.iss"')
            # The behaviour verifier strips [Icons] through [Code]; the include precedes it.
            self.assertLess(include, script.index("[Icons]"))
            for hook in ("DshIsLanding(CurPageID)", "DshApplyChoices;", "DshShowError(Failure);", "Result := CheckInstallDirectory;"):
                self.assertIn(hook, script)
            self.assertIn("WizardResizable=no", script)
            # Modern style otherwise enlarges the wizard to 120% after InitializeWizard,
            # leaving the stock finished page visible around the 520x440 surface.
            self.assertIn("WizardSizePercent=100", script)
        self.assertIn("DshSurface.Anchors := [akLeft, akTop, akRight, akBottom];", ui)
        # Upgrades stop only the Host that runs from this desktop installation.
        self.assertIn("if Result = '' then StopBundledHost;", desktop)
        self.assertIn("ExpandConstant('{app}\\host\\deepseek-harness-rs.exe')", desktop)
        self.assertIn("Where-Object { $_.Path -ieq $p }", desktop)
        for required in ("chinesesimp.DshInstallNow=立即安装", "chinesesimp.DshChooseLocation=选择安装位置", "chinesesimp.DshLaunchNow=立即体验",
                         "function ShouldSkipPage", "BrowseForFolder(", "procedure CurInstallProgressChanged", "WizardSelectTasks('desktopicon')"):
            self.assertIn(required, ui)
        for piece in ("logo", "wordmark", "button"):
            for scale in ("1x", "2x"):
                art = windows / "installer" / f"{piece}-{scale}.bmp"
                self.assertTrue(art.read_bytes().startswith(b"BM"), art.name)
                self.assertIn(f'Source: "{{#ArtDir}}\\{piece}-{scale}.bmp"; Flags: dontcopy', ui)
        verifier = (ROOT / "tools" / "verify_windows_installer_behavior.py").read_text(encoding="utf-8")
        self.assertEqual(verifier.count("f'/DArtDir={ROOT / \"packaging/windows/installer\"}'"), 2)

    def test_release_pipeline_builds_only_web_core(self):
        workflow = WORKFLOW.read_text(encoding="utf-8")
        installer = INSTALLER.read_text(encoding="utf-8")
        for variant in ("core",):
            self.assertIn(f"--variant {variant}", workflow)
            self.assertIn(f'Variant != "{variant}"', installer)
            self.assertIn(f'linux-x86_64-${{variant}}', workflow)
            self.assertIn(f'macos-${{{{ matrix.arch }}}}-${{variant}}', workflow)
        self.assertNotIn("--variant skin", workflow)
        self.assertNotIn("--variant free", workflow)
        self.assertNotIn("package_defaults", PACKAGE.read_text(encoding="utf-8"))
        self.assertIn('stage / "deepseek-black.ico"', PACKAGE.read_text(encoding="utf-8"))
        self.assertIn('prefix + "deepseek-black.ico"', VERIFIER.read_text(encoding="utf-8"))
        gate = workflow_step(workflow, "Host 与启动器回归")
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

    def test_release_pipeline_runs_task_knowledge_and_atomic_write_unit_regressions(self):
        gate = workflow_step(WORKFLOW.read_text(encoding="utf-8"), "Host 与启动器回归")
        commands = [shlex.split(line.strip()) for line in gate.splitlines()
                    if line.strip().startswith("cargo test ")]
        library_packages = set()
        for command in commands:
            if "--lib" not in command:
                continue
            self.assertIn("--locked", command)
            library_packages.update(command[index + 1] for index, item in enumerate(command)
                                    if item == "-p")
        self.assertTrue({"dsh-schedule-host", "dsh-knowledge-base", "dsh-atomic-write"}
                        .issubset(library_packages), library_packages)
        self.assertNotIn("if:", gate, "feature regressions run on every release platform")

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
            '#[cfg(not(target_os = "linux"))]\n    let initial_window_visible = control;',
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
