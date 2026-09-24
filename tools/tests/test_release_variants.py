from __future__ import annotations
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import release_variants as variants


class ReleaseVariantsTests(unittest.TestCase):
    def test_cli_selects_only_core_and_binds_the_actual_host(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binary = root / "dsh"
            binary.write_bytes(b"fixture binary")
            output, summary, selection = root / "output.txt", root / "summary.md", root / "selection.json"
            completed = subprocess.run([sys.executable, str(Path(variants.__file__)), "select",
                "--binary", str(binary), "--selection-report", str(selection),
                "--github-output", str(output), "--summary", str(summary)],
                env=dict(os.environ, PYTHONIOENCODING="cp1252"), capture_output=True)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            selected = json.loads(completed.stdout)
            self.assertEqual(selected["variants"], ["core"])
            self.assertEqual(selected["binarySha256"], hashlib.sha256(binary.read_bytes()).hexdigest())
            self.assertEqual(json.loads(selection.read_text(encoding="utf-8")), selected)
            self.assertEqual(output.read_text(), 'variants=core\nvariants_json=["core"]\n')

    def test_only_complete_core_platform_payloads_can_be_published(self):
        for platform, arch in [("windows", "x86_64"), ("linux", "x86_64"), ("macos", "x86_64"), ("macos", "aarch64")]:
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory); prefix = f"deepseek-harness-rs-v0.1.3-test-{platform}-{arch}"
                expected = variants.expected_artifacts(prefix, platform, ["core"])
                self.assertEqual(len(expected), 2)
                for name in expected: (root / name).write_bytes(b"artifact")
                checksums = root / "SHA256SUMS.txt"
                variants.write_checksums(root, prefix, platform, ["core"], checksums)
                self.assertEqual(len(checksums.read_text().splitlines()), 2)
                retired = root / f"{prefix}-skin-portable.zip"; retired.write_bytes(b"old artifact")
                with self.assertRaisesRegex(ValueError, "extra="):
                    variants.write_checksums(root, prefix, platform, ["core"], checksums)
                retired.unlink(); (root / next(iter(expected))).unlink()
                with self.assertRaisesRegex(ValueError, "missing="):
                    variants.write_checksums(root, prefix, platform, ["core"], checksums)

    def test_retired_and_unknown_distributions_are_rejected(self):
        for selected in [[], ["skin"], ["free"], ["core", "skin"], ["core", "free"], ["core", "core"]]:
            with self.assertRaises(ValueError):
                variants.expected_artifacts("valid-prefix", "windows", selected)

    def test_workflow_has_no_model_specific_or_skin_publication_path(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/release.yml").read_text(encoding="utf-8")
        for retired in ("verify_free_model_catalog.py", "--variant skin", "--variant free", "--bin dsh-skin-installer", "steps.free_probe"):
            self.assertNotIn(retired, workflow)
        self.assertIn("tools/release_variants.py checksums", workflow)
        self.assertIn("--variant core", workflow)
        self.assertIn("tools/build_native_sandbox.py", workflow)


if __name__ == "__main__":
    unittest.main()
