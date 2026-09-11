import hashlib
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from promote_release import PLATFORMS, validate_payload, validate_run
from release_variants import expected_artifacts


class ReleasePromotionTests(unittest.TestCase):
    def test_only_successful_same_commit_complete_build_is_accepted(self):
        run = {"status": "completed", "conclusion": "success", "path": ".github/workflows/release.yml", "head_sha": "abc"}
        artifacts = [{"name": "deepseek-harness-rs-" + p, "expired": False, "workflow_run": {"head_sha": "abc"}} for p in PLATFORMS]
        validate_run(run, "abc", artifacts)
        for invalid_run, sha, files in [({**run, "conclusion": "failure"}, "abc", artifacts), (run, "old", artifacts), (run, "abc", artifacts[:-1]), (run, "abc", [{**a, "expired": True} for a in artifacts])]:
            with self.assertRaises(ValueError):
                validate_run(invalid_run, sha, files)

    def test_complete_payload_checks_every_file_and_rejects_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            expected = {}
            for platform in PLATFORMS:
                names = expected_artifacts("deepseek-harness-rs-v0.1.3-test-" + platform, platform.split("-")[0], ["core", "skin", "free"])
                rows = []
                for name in sorted(names):
                    data = name.encode()
                    (root / name).write_bytes(data)
                    sha = hashlib.sha256(data).hexdigest()
                    expected[name] = sha
                    rows.append(f"{sha}  {name}\n")
                (root / f"SHA256SUMS-{platform}.txt").write_text("".join(rows), encoding="utf-8")
            self.assertEqual(validate_payload(root, "v0.1.3-test"), expected)
            self.assertEqual(len(expected), 24)
            first = root / next(iter(expected))
            first.write_bytes(b"wrong build")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                validate_payload(root, "v0.1.3-test")

    def test_tag_cannot_escape_notes_or_artifact_paths(self):
        with self.assertRaisesRegex(ValueError, "invalid release tag"):
            validate_payload(Path("."), "../../other")
