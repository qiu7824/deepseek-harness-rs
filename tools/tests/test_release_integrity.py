from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import pathlib
import shutil
import tempfile
import unittest
from unittest import mock


ROOT = pathlib.Path(__file__).resolve().parents[2]


def load_tool(name: str):
    path = ROOT / "tools" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class ReleaseIntegrityTests(unittest.TestCase):
    def web_fixture(self, root):
        source = root / "web" / "dist"
        (source / "plugins").mkdir(parents=True)
        (source / "empty-directory").mkdir()
        bundle = b"globalThis.releaseUi = 'current';"
        (source / "plugins" / "ui-settings-general.js").write_bytes(bundle)
        (source / "index.html").write_bytes(b"<html>current</html>")
        (source / "plugins" / "manifest.json").write_text(json.dumps({"entries": [{
            "url": "/plugins/ui-settings-general.js", "rev": hashlib.sha256(bundle).hexdigest()[:16],
        }]}), encoding="utf-8")
        staged = root / "target" / "release" / "web" / "dist"
        shutil.copytree(source, staged)
        return source, staged

    def assert_rejected_without_packaging_side_effects(self, package, root, message):
        suffix = "deepseek-harness-rs-v0.1.3-test-linux-x86_64-core"
        previous = root / "dist" / suffix
        previous.mkdir(parents=True, exist_ok=True)
        marker = previous / "previous-package.txt"
        marker.write_bytes(b"keep previous package")
        archive = root / "dist" / f"{suffix}-portable.tar.gz"
        archive.write_bytes(b"keep previous archive")
        with mock.patch.object(package, "ROOT", root), mock.patch.object(package, "verify_release_version") as version_check, mock.patch.object(package.sys, "argv", [
            "package_release.py", "--platform", "linux", "--arch", "x86_64", "--version", "0.1.3-test",
        ]):
            with self.assertRaisesRegex(ValueError, message) as failure:
                package.main()
            self.assertIn("stage_release_web.py", str(failure.exception))
            version_check.assert_not_called()
        self.assertEqual(marker.read_bytes(), b"keep previous package")
        self.assertEqual(archive.read_bytes(), b"keep previous archive")

    def test_packaging_accepts_identical_staged_files_directories_and_manifest(self):
        package = load_tool("package_release")
        with tempfile.TemporaryDirectory(dir=os.environ.get("DSH_TEST_TEMP_DIR")) as directory:
            source, staged = self.web_fixture(pathlib.Path(directory))
            package.verify_staged_web(source, staged)
            self.assertEqual((source / "plugins/manifest.json").read_bytes(), (staged / "plugins/manifest.json").read_bytes())

    def test_stale_web_is_rejected_before_replacing_existing_package_or_archive(self):
        package = load_tool("package_release")
        for change in ("missing", "extra", "changed", "manifest", "empty-directory", "missing-stage"):
            with self.subTest(change=change), tempfile.TemporaryDirectory(dir=os.environ.get("DSH_TEST_TEMP_DIR")) as directory:
                root = pathlib.Path(directory)
                _, staged = self.web_fixture(root)
                if change == "missing":
                    (staged / "index.html").unlink()
                elif change == "extra":
                    (staged / "old-plugin.js").write_bytes(b"obsolete")
                elif change == "changed":
                    (staged / "plugins/ui-settings-general.js").write_bytes(b"globalThis.releaseUi = 'old-ui!';")
                elif change == "manifest":
                    (staged / "plugins/manifest.json").write_bytes(b"{}")
                elif change == "empty-directory":
                    (staged / "empty-directory").rmdir()
                else:
                    shutil.rmtree(staged)
                self.assert_rejected_without_packaging_side_effects(package, root, "staged web distribution")

    def test_equal_web_trees_still_require_valid_manifest_revisions(self):
        package = load_tool("package_release")
        for change in ("stale-revision", "missing-manifest", "malformed-manifest"):
            with self.subTest(change=change), tempfile.TemporaryDirectory(dir=os.environ.get("DSH_TEST_TEMP_DIR")) as directory:
                root = pathlib.Path(directory)
                source, staged = self.web_fixture(root)
                for distribution in (source, staged):
                    manifest = distribution / "plugins/manifest.json"
                    if change == "missing-manifest":
                        manifest.unlink()
                    elif change == "malformed-manifest":
                        manifest.write_text("{broken", encoding="utf-8")
                    else:
                        value = json.loads(manifest.read_text(encoding="utf-8"))
                        value["entries"][0]["rev"] = "0" * 16
                        manifest.write_text(json.dumps(value), encoding="utf-8")
                self.assert_rejected_without_packaging_side_effects(package, root, "invalid web manifest")

    def test_free_package_requires_recent_complete_inference_evidence(self):
        from datetime import datetime, timezone, timedelta
        package = load_tool("package_release")
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "verification.json"
            report = {"url": "https://opencode.ai/zen/v1/models", "model": "ling-3.0-flash-fin-free", "pricingSource": "https://opencode.ai/docs/zen/", "binarySha256": "a" * 64, "verifiedAt": datetime.now(timezone.utc).isoformat(), **{key: True for key in ("available", "freePricingVerified", "harnessVerified", "inference", "streaming", "toolCall", "toolResult", "anonymous")}}
            path.write_text(json.dumps(report), encoding="utf-8")
            self.assertEqual(package.verified_free_model(path), report)
            for invalid in ({**report, "toolResult": False}, {**report, "model": "different-free"}, {**report, "verifiedAt": (datetime.now(timezone.utc) - timedelta(days=2)).isoformat()}):
                path.write_text(json.dumps(invalid), encoding="utf-8")
                with self.assertRaises(ValueError):
                    package.verified_free_model(path)

    def test_release_inputs_are_not_gitignored(self):
        import subprocess

        required = [
            "tools/stage_release_web.py",
            "tools/package_release.py",
            "tools/verify_release_package.py",
            "tools/verify_free_model_catalog.py",
            "tools/verify_installer_package.py",
            "tools/tests/connection_controller_harness.js",
            "tools/tests/test_rust_runtime_contract.py",
            "tools/tests/test_release_product_contract.py",
            "crates/web/web-fetch-http/Cargo.toml",
            "crates/web/web-fetch-http/src/lib.rs",
            "web/dist/plugins/ui-conversation.js",
            "web/dist/plugins/ui-theme.js",
            "web/dist/plugins/ui-trajectory.js",
            "web/dist/plugins/ui-model-selection.js",
            "web/dist/plugins/ui-settings-models.js",
            "web/dist/plugins/ui-schedule.js",
            "web/dist/skins/deepseek-official/skin.json",
        ]
        result = subprocess.run(
            ["git", "check-ignore", *required],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 1, result.stdout)

        tracked = subprocess.run(
            ["git", "ls-files", "--error-unmatch", *required],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        self.assertEqual(tracked.returncode, 0, tracked.stderr)

    def test_manifest_revisions_cover_every_declared_bundle(self):
        manifest_path = ROOT / "web" / "dist" / "plugins" / "manifest.json"
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        for entry in manifest["entries"]:
            bundle = ROOT / "web" / "dist" / entry["url"].lstrip("/")
            self.assertTrue(bundle.is_file(), entry["url"])
            self.assertEqual(
                entry["rev"],
                hashlib.sha256(bundle.read_bytes()).hexdigest()[:16],
                entry["url"],
            )

    def test_staging_rejects_temporary_files(self):
        stage_release_web = load_tool("stage_release_web")
        with tempfile.TemporaryDirectory() as temporary:
            source = pathlib.Path(temporary) / "source"
            target = pathlib.Path(temporary) / "target"
            (source / "plugins").mkdir(parents=True)
            (source / "plugins" / ".hermes-tmp.bad").write_text("", encoding="utf-8")
            original = stage_release_web.SOURCE
            stage_release_web.SOURCE = source
            try:
                with self.assertRaisesRegex(ValueError, "temporary"):
                    stage_release_web.stage_release_web(target)
            finally:
                stage_release_web.SOURCE = original

    def test_packaging_requires_staged_web(self):
        source = (ROOT / "tools" / "package_release.py").read_text(encoding="utf-8")
        self.assertNotIn("else ROOT / \"web\" / \"dist\"", source)
        self.assertIn("missing staged web distribution", source)

    def test_installer_verifier_rejects_missing_packages_before_extraction(self):
        source = (ROOT / "tools" / "verify_installer_package.py").read_text(
            encoding="utf-8"
        )
        package_guard = 'if not package.is_file():\n        raise SystemExit(f"missing installer package: {package}")'
        self.assertIn(package_guard, source)
        self.assertLess(source.index(package_guard), source.index("verify_windows(package, stage)"))

    def test_portable_verifier_executes_the_host_extracted_from_the_archive(self):
        source = (ROOT / "tools" / "verify_release_package.py").read_text(
            encoding="utf-8"
        )
        for marker in (
            "archived_host = read_archive_file(archive, host)",
            "archive host does not match target/release host",
            "with tempfile.TemporaryDirectory() as temporary:",
            "extracted_root",
            "extracted_host",
            "assert_same_tree(extracted_root, staged_root)",
            "subprocess.check_output(\n                [str(extracted_host), \"--version\"]",
        ):
            self.assertIn(marker, source)
        self.assertNotIn("subprocess.check_output([str(staged_host), \"--version\"]", source)

    def test_release_path_components_reject_traversal(self):
        package = load_tool("package_release")
        verifier = load_tool("verify_release_package")
        for module in (package, verifier):
            for field, value in (
                ("arch", "../outside"),
                ("arch", "x86_64/../../outside"),
                ("version", "0.1.2/../../outside"),
            ):
                with self.assertRaisesRegex(ValueError, field):
                    module.validated_release_component(field, value)
        self.assertEqual(package.validated_release_component("arch", "x86_64"), "x86_64")

    def test_release_workflow_classifies_any_semver_suffix_as_prerelease(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
        self.assertIn("contains(github.ref_name, '-')", workflow)
        self.assertNotIn("contains(github.ref_name, '-rc')", workflow)
        self.assertIn("python tools/verify_release_version.py", workflow)

    def test_release_workflow_runs_web_fetch_and_client_performance_regressions(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
        for marker in (
            "-p dsh-web ",
            "-p dsh-web-fetch-http",
            "-p dsh-tool-web",
            "tools.tests.test_client_performance",
        ):
            self.assertIn(marker, workflow)

    def test_linux_deb_records_root_owned_payload(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
        self.assertIn("dpkg-deb --root-owner-group --build", workflow)


if __name__ == "__main__":
    unittest.main()
