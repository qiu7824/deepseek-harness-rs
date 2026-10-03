import base64
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import publish_flutter_golden_evidence as evidence


PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==")


class FlutterGoldenEvidenceTests(unittest.TestCase):
    def fixture(self, temporary):
        root = Path(temporary)
        repo, remote = root / "repo", root / "remote.git"
        repo.mkdir()
        subprocess.run(["git", "init", "-b", "main", str(repo)], check=True, capture_output=True)
        subprocess.run(["git", "init", "--bare", str(remote)], check=True, capture_output=True)
        evidence.git(repo, ["config", "user.name", "Fixture"])
        evidence.git(repo, ["config", "user.email", "fixture@example.invalid"])
        evidence.git(repo, ["remote", "add", "origin", str(remote)])
        golden_dir = repo / "apps/desktop_flutter/test/goldens"
        golden_dir.mkdir(parents=True)
        for name in evidence.GOLDENS:
            (golden_dir / name).write_bytes(PNG)
        (repo / "source.txt").write_text("keep main source", encoding="utf-8")
        evidence.git(repo, ["add", "."])
        evidence.git(repo, ["commit", "-m", "main fixture"])
        head = evidence.git(repo, ["rev-parse", "HEAD"])
        (repo / "staged.txt").write_text("keep original index", encoding="utf-8")
        evidence.git(repo, ["add", "staged.txt"])
        failures = repo / "apps/desktop_flutter/test/failures"
        failures.mkdir()
        return repo, remote, head, failures

    def create(self, repo, head, run_id="1234"):
        return evidence.create_commit(repo, platform="macos", arch="aarch64", run_id=run_id, run_attempt="1", source=head)

    def test_parentless_tree_only_contains_expected_pngs_and_manifest_without_changing_main_or_index(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo, remote, head, failures = self.fixture(temporary)
            expected = "icons_light_1.0x_testImage.png"
            (failures / expected).write_bytes(PNG)
            (failures / "private.png").write_bytes(b"do not publish unrelated user data")
            (failures / ".env").write_text("do not publish an environment", encoding="utf-8")
            index = Path(evidence.git(repo, ["rev-parse", "--git-path", "index"]))
            if not index.is_absolute():
                index = repo / index
            original_index = index.read_bytes()
            original_status = evidence.git(repo, ["status", "--porcelain=v1"])
            branch, commit = self.create(repo, head)
            self.assertEqual(branch, "diagnostics/flutter-goldens-1234-macos-aarch64")
            self.assertIsNotNone(commit)
            self.assertEqual(evidence.git(repo, ["rev-list", "--parents", "-n", "1", commit]), commit)
            files = evidence.git(repo, ["ls-tree", "-r", "--name-only", commit]).splitlines()
            self.assertEqual(set(files), {"manifest.json", f"failures/{expected}", *(f"baselines/{name}" for name in evidence.GOLDENS)})
            manifest = json.loads(evidence.git(repo, ["show", f"{commit}:manifest.json"]))
            self.assertEqual(manifest["sourceRevision"], head)
            self.assertEqual(manifest["files"][f"failures/{expected}"]["sha256"], hashlib.sha256(PNG).hexdigest())
            evidence.publish(repo, branch, commit)  # A temporary local bare repository, never a network push.
            self.assertEqual(subprocess.check_output(["git", "rev-parse", f"refs/heads/{branch}"], cwd=remote, text=True).strip(), commit)
            self.assertEqual(evidence.git(repo, ["rev-parse", "HEAD"]), head)
            self.assertEqual(evidence.git(repo, ["rev-parse", "refs/heads/main"]), head)
            self.assertEqual(evidence.git(repo, ["symbolic-ref", "HEAD"]), "refs/heads/main")
            self.assertEqual(index.read_bytes(), original_index)
            self.assertEqual(evidence.git(repo, ["status", "--porcelain=v1"]), original_status)
            with self.assertRaisesRegex(ValueError, "refusing to overwrite"):
                evidence.publish(repo, branch, commit)

    def test_no_failure_pngs_create_no_commit_or_branch(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo, remote, head, failures = self.fixture(temporary)
            (failures / "unrelated.png").write_bytes(PNG)
            self.assertEqual(self.create(repo, head), ("diagnostics/flutter-goldens-1234-macos-aarch64", None))
            self.assertEqual(subprocess.check_output(["git", "for-each-ref", "--format=%(refname)"], cwd=remote, text=True), "")

    def test_invalid_identity_invalid_png_and_outside_symlink_are_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            repo, remote, head, failures = self.fixture(temporary)
            source = failures / "icons_light_1.0x_testImage.png"
            source.write_bytes(b"not a PNG")
            with self.assertRaisesRegex(ValueError, "PNG header"):
                self.create(repo, head)
            with self.assertRaisesRegex(ValueError, "run identity"):
                self.create(repo, head, run_id="../main")
            with self.assertRaisesRegex(ValueError, "checked-out commit"):
                self.create(repo, "0" * 40)
            source.unlink()
            outside = Path(temporary) / "outside.png"
            outside.write_bytes(PNG)
            try:
                source.symlink_to(outside)
            except OSError:
                self.skipTest("creating local symlinks is unavailable")
            with self.assertRaisesRegex(ValueError, "symbolic links"):
                self.create(repo, head)


if __name__ == "__main__":
    unittest.main()
