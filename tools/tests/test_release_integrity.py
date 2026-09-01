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
    def test_release_inputs_are_not_gitignored(self):
        import subprocess

        required = [
            "tools/stage_release_web.py",
            "tools/tests/connection_controller_harness.js",
            "tools/tests/test_v012_alpha2_sync_contract.py",
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


if __name__ == "__main__":
    unittest.main()