from __future__ import annotations
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import release_variants as variants
from tools.tests.test_free_model_evidence import attested, report


class ReleaseVariantsTests(unittest.TestCase):
    def test_core_skin_are_independent_of_missing_failed_or_stale_free_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory); binary=root/"dsh";binary.write_bytes(b"fixture binary")
            path=root/"free.json"
            self.assertEqual(variants.select_variants(path,binary,"failure")["variants"],["core","skin"])
            payload=report([{**attested(),"status":"unavailable","available":False,"reason":"供应商仅限OpenCode，Rust匿名不可用"}])
            path.write_text(json.dumps(payload),encoding="utf-8")
            selected=variants.select_variants(path,binary,"failure")
            self.assertEqual(selected["variants"],["core","skin"])
            self.assertIn("供应商仅限OpenCode",selected["free"]["reason"])
            self.assertEqual(variants.select_variants(path,binary,"success")["variants"],["core","skin"])

    def test_free_requires_successful_probe_and_current_binary_attestation(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory);binary=root/"dsh";binary.write_bytes(b"fixture binary");path=root/"free.json"
            digest=hashlib.sha256(binary.read_bytes()).hexdigest()
            payload=report([{**attested(),"binarySha256":digest}]);payload["binarySha256"]=digest
            path.write_text(json.dumps(payload),encoding="utf-8")
            self.assertEqual(variants.select_variants(path,binary,"success")["variants"],["core","skin","free"])
            self.assertEqual(variants.select_variants(path,binary,"failure")["variants"],["core","skin"])
            binary.write_bytes(b"other candidate")
            self.assertEqual(variants.select_variants(path,binary,"success")["variants"],["core","skin"])

    def test_each_platform_checksum_set_matches_only_selected_variants(self):
        for platform,arch in [("windows","x86_64"),("linux","x86_64"),("macos","x86_64"),("macos","aarch64")]:
            for selected in [["core","skin"],["core","skin","free"]]:
                with tempfile.TemporaryDirectory() as directory:
                    root=Path(directory);prefix=f"deepseek-harness-rs-v0.1.3-test-{platform}-{arch}"
                    expected=variants.expected_artifacts(prefix,platform,selected)
                    for name in expected:(root/name).write_bytes(b"artifact")
                    checksums=root/"SHA256SUMS.txt"
                    variants.write_checksums(root,prefix,platform,selected,checksums)
                    self.assertEqual(len(checksums.read_text().splitlines()),2*len(selected))
                    (root/f"{prefix}-unexpected.zip").write_bytes(b"unverified")
                    with self.assertRaisesRegex(ValueError,"extra="):
                        variants.write_checksums(root,prefix,platform,selected,checksums)

    def test_unknown_variant_cannot_enter_artifact_set(self):
        for selected in [[],["free"],["core","skin","unverified"]]:
            with self.assertRaises(ValueError):variants.expected_artifacts("valid-prefix","windows",selected)

    def test_workflow_keeps_live_free_failure_optional_but_retains_evidence(self):
        workflow=(Path(__file__).resolve().parents[2]/".github/workflows/release.yml").read_text(encoding="utf-8")
        from tools.tests.test_release_product_contract import workflow_step
        probe=workflow_step(workflow,"验证免费模型完整运行链路")
        self.assertIn("continue-on-error: true",probe)
        self.assertIn("--binary target/release/",probe)
        self.assertNotIn("verify_free_model_catalog.py",workflow_step(workflow,"版本与产品门禁"))
        self.assertIn("if: always()",workflow_step(workflow,"保留免费模型验收证据"))
        self.assertIn('pattern: "deepseek-harness-rs-*"',workflow)
        self.assertIn("steps.variants.outputs.variants_json",workflow_step(workflow,"生成校验和"))
        self.assertIn("-p dsh-desktop-controller -p dsh-uu-controller",workflow)


if __name__=="__main__":unittest.main()
