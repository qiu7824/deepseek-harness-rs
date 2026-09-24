import json
import os
from pathlib import Path
import plistlib
import sys
import tarfile
import tempfile
import unittest
import zipfile

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import package_flutter_release as package


class FlutterReleaseTests(unittest.TestCase):
    def fixture(self, root, platform):
        client, core = root / "client", root / "core"
        if platform == "windows":
            files = ["dsh_desktop.exe", "flutter_windows.dll", "data/app.so", "data/icudtl.dat"]
        elif platform == "linux":
            files = ["dsh_desktop", "lib/libflutter_linux_gtk.so", "lib/libapp.so", "data/icudtl.dat"]
        else:
            files = ["Contents/MacOS/DeepSeek Harness", "Contents/Frameworks/FlutterMacOS.framework/FlutterMacOS", "Contents/Frameworks/App.framework/App"]
        for name in files:
            path = client / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(name.encode())
        if platform == "macos":
            (client / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleExecutable": "DeepSeek Harness"}))
        host = package.binary_name(platform, "deepseek-harness-rs")
        for name in [host, package.binary_name(platform, "dsh-remote-helper"), "web/dist/index.html", "runtime/node/" + package.binary_name(platform, "node")]:
            path = core / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(("core:" + name).encode())
        manifest = {"version": "1.2.3", "platform": platform, "arch": "x86_64", "variant": "core", "host": host}
        (core / "PACKAGE.json").write_text(json.dumps(manifest), encoding="utf-8")
        return client, core

    def test_platform_layout_and_archive_preserve_complete_client_and_host(self):
        for platform in ["windows", "linux", "macos"]:
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                client, core = self.fixture(root, platform)
                staged = root / "delivery"
                meta = package.stage(client, core, staged, platform, "x86_64", "1.2.3", "revision")
                host = staged / meta["hostRoot"]
                for path in core.rglob("*"):
                    if path.is_file():
                        self.assertEqual(path.read_bytes(), (host / path.relative_to(core)).read_bytes())
                if platform == "macos":
                    self.assertEqual(meta["hostRoot"], "DeepSeek Harness.app/Contents/Resources/host")
                else:
                    self.assertEqual(meta["hostRoot"], "host")
                package.write_manifest(staged, meta)
                expected = json.loads((staged / "DESKTOP.json").read_text())
                if platform == "windows":
                    archive = root / "delivery.zip"
                    with zipfile.ZipFile(archive, "w") as writer:
                        for path in staged.rglob("*"):
                            writer.write(path, path.relative_to(root).as_posix())
                else:
                    archive = root / "delivery.tar.gz"
                    with tarfile.open(archive, "w:gz") as writer:
                        writer.add(staged, arcname="delivery")
                package.verify_archive(archive, "delivery", expected)
                altered = json.loads(json.dumps(expected))
                altered["files"][meta["hostRoot"] + "/" + package.binary_name(platform, "deepseek-harness-rs")]["sha256"] = "0" * 64
                with self.assertRaisesRegex(ValueError, "inventory"):
                    package.verify_archive(archive, "delivery", altered)

    def test_wrong_core_identity_and_missing_runtime_fail_before_destination_creation(self):
        for failure in ["version", "arch", "variant", "missing-runtime"]:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                client, core = self.fixture(root, "windows")
                if failure == "missing-runtime":
                    (client / "flutter_windows.dll").unlink()
                else:
                    manifest = json.loads((core / "PACKAGE.json").read_text())
                    manifest[failure] = "wrong"
                    (core / "PACKAGE.json").write_text(json.dumps(manifest))
                with self.assertRaises(ValueError):
                    package.stage(client, core, root / "delivery", "windows", "x86_64", "1.2.3", "revision")
                self.assertFalse((root / "delivery").exists())

    def test_existing_destination_and_bundled_host_are_not_overwritten(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            client, core = self.fixture(root, "windows")
            target = root / "existing"
            target.mkdir()
            marker = target / "keep.txt"
            marker.write_text("keep")
            with self.assertRaisesRegex(ValueError, "already exists"):
                package.stage(client, core, target, "windows", "x86_64", "1.2.3", "revision")
            self.assertEqual(marker.read_text(), "keep")
            (client / "host").mkdir()
            (client / "host/keep.txt").write_text("unrelated")
            with self.assertRaisesRegex(ValueError, "unexpected bundled Host"):
                package.stage(client, core, root / "delivery", "windows", "x86_64", "1.2.3", "revision")
            self.assertEqual((client / "host/keep.txt").read_text(), "unrelated")

    def test_outside_symlinks_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            client, _ = self.fixture(root, "macos")
            outside = root / "private.txt"
            outside.write_text("private")
            try:
                (client / "escape").symlink_to(outside)
            except OSError:
                self.skipTest("symlink creation requires local permission")
            with self.assertRaisesRegex(ValueError, "escapes"):
                package.inventory(client)

    def test_workflow_uses_one_source_commit_and_all_native_platforms(self):
        workflow = (package.ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
        self.assertIn(package.FLUTTER_REVISION, workflow)
        for marker in ["flutter test", "flutter analyze", "flutter build ${{ matrix.platform }}", "tools/package_flutter_release.py", "tools/verify_flutter_installer.py", "--enforce-lockfile"]:
            self.assertIn(marker, workflow)


if __name__ == "__main__":
    unittest.main()
