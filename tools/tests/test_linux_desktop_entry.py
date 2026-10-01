import importlib.util
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

SOURCE = Path(__file__).resolve().parents[2] / "apps/desktop_flutter/linux"
spec = importlib.util.spec_from_file_location("linux_desktop_entry", SOURCE / "install-desktop-entry.py")
desktop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(desktop)


class LinuxDesktopEntryTests(unittest.TestCase):
    def bundle(self, root):
        root.mkdir()
        (root / "dsh_desktop").write_text("runtime", encoding="utf-8")
        template = root / "share/applications" / (desktop.APPLICATION_ID + ".desktop")
        template.parent.mkdir(parents=True)
        template.write_text(
            (SOURCE / (desktop.APPLICATION_ID + ".desktop.in")).read_text(encoding="utf-8")
            .replace("@BINARY_NAME@", "dsh_desktop")
            .replace("@APPLICATION_ID@", desktop.APPLICATION_ID), encoding="utf-8")
        icon = root / "share/icons/hicolor/scalable/apps" / (desktop.APPLICATION_ID + ".svg")
        icon.parent.mkdir(parents=True)
        icon.write_text('<svg xmlns="http://www.w3.org/2000/svg"/>', encoding="utf-8")
        return root

    def test_install_and_uninstall_only_own_launcher_and_icon(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.bundle(root / "中文 workspace")
            data = root / "isolated-data"
            self.assertFalse(data.exists())
            self.assertTrue(desktop.integrate(bundle, data))
            entry = data / "applications" / (desktop.APPLICATION_ID + ".desktop")
            value = entry.read_text(encoding="utf-8")
            self.assertIn("Exec=" + desktop.exec_argument(str(bundle / "dsh_desktop")), value)
            self.assertIn("Icon=" + desktop.APPLICATION_ID, value)
            self.assertIn("StartupWMClass=" + desktop.APPLICATION_ID, value)
            self.assertIn(desktop.owner(bundle), value)
            self.assertTrue(desktop.integrate(bundle, data))
            sentinel = data / "applications/keep.desktop"
            sentinel.write_text("keep", encoding="utf-8")
            self.assertTrue(desktop.integrate(bundle, data, remove=True))
            self.assertFalse(entry.exists())
            self.assertEqual(list((data / "icons/hicolor/scalable/apps").iterdir()), [])
            self.assertEqual(sentinel.read_text(), "keep")
            self.assertTrue((bundle / "dsh_desktop").exists())
            self.assertFalse(desktop.integrate(bundle, data, remove=True))

    def test_old_bundle_cannot_remove_newer_registration(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            first, second = self.bundle(root / "first"), self.bundle(root / "second")
            data = root / "data"
            desktop.integrate(first, data)
            desktop.integrate(second, data)
            self.assertFalse(desktop.integrate(first, data, remove=True))
            entry = data / "applications" / (desktop.APPLICATION_ID + ".desktop")
            self.assertIn(desktop.owner(second), entry.read_text(encoding="utf-8"))

    def test_unmanaged_launcher_and_modified_icon_are_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle, data = self.bundle(root / "bundle"), root / "data"
            entry = data / "applications" / (desktop.APPLICATION_ID + ".desktop")
            entry.parent.mkdir(parents=True)
            entry.write_text("[Desktop Entry]\nName=Keep", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unmanaged"):
                desktop.integrate(bundle, data)
            self.assertEqual(entry.read_text(), "[Desktop Entry]\nName=Keep")
            entry.unlink()
            desktop.integrate(bundle, data)
            icon = data / "icons/hicolor/scalable/apps" / (desktop.APPLICATION_ID + ".svg")
            icon.write_text("custom icon", encoding="utf-8")
            desktop.integrate(bundle, data, remove=True)
            self.assertEqual(icon.read_text(), "custom icon")

    def test_exec_path_uses_desktop_quoting_without_a_shell(self):
        self.assertEqual(desktop.exec_argument('/opt/DeepSeek Harness/dsh_desktop'),
                         '"/opt/DeepSeek Harness/dsh_desktop"')
        self.assertEqual(desktop.exec_argument('/tmp/%done/$HOME/`cmd`/"file"/\\app'),
                         '"/tmp/%%done/\\\\$HOME/\\\\`cmd\\\\`/\\\\"file\\\\"/\\\\\\\\app"')
        with self.assertRaises(ValueError):
            desktop.exec_argument("/tmp/line\nbreak")
        with self.assertRaises(ValueError):
            desktop.exec_argument("/tmp/name=value/dsh_desktop")

    def test_failed_launcher_install_does_not_leave_an_unowned_icon(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle, data = self.bundle(root / "bundle"), root / "data"
            original = desktop.atomic_write

            def fail_launcher(path, content):
                if path.suffix == ".desktop":
                    raise PermissionError("fixture write refused")
                original(path, content)

            with mock.patch.object(desktop, "atomic_write", side_effect=fail_launcher):
                with self.assertRaises(PermissionError):
                    desktop.integrate(bundle, data)
            self.assertFalse((data / "icons/hicolor/scalable/apps" / (desktop.APPLICATION_ID + ".svg")).exists())

    @unittest.skipUnless(shutil.which("desktop-file-validate"), "freedesktop validator requires a Linux environment")
    def test_installed_entry_passes_freedesktop_validator(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            desktop.integrate(self.bundle(root / "workspace with spaces"), root / "data")
            entry = root / "data/applications" / (desktop.APPLICATION_ID + ".desktop")
            subprocess.run([shutil.which("desktop-file-validate"), str(entry)], check=True)


if __name__ == "__main__":
    unittest.main()
