from __future__ import annotations

import hashlib
import importlib.util
import json
import pathlib
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]


def load_tool(name: str):
    path = ROOT / "tools" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


class ReleaseIntegrityTests(unittest.TestCase):
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
