import hashlib
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from promote_release import PLATFORMS, notes_path, version_tag, release_metadata, validate_payload, validate_release, validate_run
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
                names = expected_artifacts("deepseek-harness-rs-v0.1.3-test-" + platform, platform.split("-")[0], ["core"])
                rows = []
                for name in sorted(names):
                    data = name.encode()
                    (root / name).write_bytes(data)
                    sha = hashlib.sha256(data).hexdigest()
                    expected[name] = sha
                    rows.append(f"{sha}  {name}\n")
                (root / f"SHA256SUMS-{platform}.txt").write_text("".join(rows), encoding="utf-8")
            self.assertEqual(validate_payload(root, "v0.1.3-test"), expected)
            self.assertEqual(validate_payload(root, "v0.1.3-test-r4"), expected)
            self.assertEqual(len(expected), 13)
            checksums = (root / "SHA256SUMS.txt").read_bytes()
            self.assertNotIn(b"\r", checksums, "release checksums must have identical bytes on Windows and Unix")
            self.assertEqual(checksums.count(b"\n"), len(expected))
            first = root / next(iter(expected))
            first.write_bytes(b"wrong build")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                validate_payload(root, "v0.1.3-test")

    def test_tag_cannot_escape_notes_or_artifact_paths(self):
        with self.assertRaisesRegex(ValueError, "invalid release tag"):
            validate_payload(Path("."), "../../other")

    def test_revision_tags_use_versioned_packages_and_notes(self):
        self.assertEqual(version_tag("v0.1.3-alpha.20-r4"), "v0.1.3-alpha.20")
        self.assertEqual(notes_path("v0.1.3-alpha.20-r4").name, "v0.1.3-alpha.20.md")
        self.assertEqual(version_tag("v0.1.3-alpha.20"), "v0.1.3-alpha.20")
        with self.assertRaises(ValueError):
            notes_path("../notes")

    def test_complete_draft_is_checked_before_publication(self):
        expected = {"package.zip": "a" * 64, "SHA256SUMS.txt": "b" * 64}
        release = {"draft": True, "body": "Release notes", "assets": [
            {"name": name, "digest": "sha256:" + sha} for name, sha in expected.items()]}
        validate_release(release, expected, "Release notes\n", allow_draft=True)
        with self.assertRaises(ValueError):
            validate_release(release, expected, "Release notes")
        validate_release({**release, "draft": False}, expected, "Release notes")

    def test_draft_mode_never_allows_missing_or_mismatched_assets(self):
        expected = {"package.zip": "a" * 64}
        for assets in ([], [{"name": "package.zip", "digest": "sha256:" + "b" * 64}],
                       [{"name": "package.zip", "digest": None}]):
            with self.assertRaisesRegex(ValueError, "asset set or checksum"):
                validate_release({"draft": True, "body": "Notes", "assets": assets},
                                 expected, "Notes", allow_draft=True)

    def test_draft_notes_must_match_before_publication(self):
        with self.assertRaisesRegex(ValueError, "release notes differ"):
            validate_release({"draft": True, "body": "Old notes", "assets": []},
                             {}, "Current notes", allow_draft=True)

    def test_draft_lookup_uses_paginated_releases_instead_of_published_tag_endpoint(self):
        draft = {"tag_name": "v1-test", "draft": True}
        with patch("promote_release.api", side_effect=[
            [{"tag_name": f"v2-{index}", "draft": False} for index in range(100)], [draft]
        ]) as api:
            self.assertEqual(release_metadata("v1-test", allow_draft=True), draft)
            self.assertEqual([call.args[0] for call in api.call_args_list],
                             ["releases?per_page=100&page=1", "releases?per_page=100&page=2"])
        with patch("promote_release.api", return_value=[]) as api:
            with self.assertRaisesRegex(ValueError, "not found"):
                release_metadata("v1-test", allow_draft=True)
