import unittest

from tools.verify_release_version import verify_build_identity, verify_manifest_version, verify_release_tag


class ReleaseTagTests(unittest.TestCase):
    def test_exact_version_and_numeric_revisions_are_accepted(self):
        for tag in ("v0.1.3-alpha.20", "v0.1.3-alpha.20-r3", "v0.1.3-alpha.20-r123"):
            verify_release_tag(tag, "0.1.3-alpha.20")

    def test_wrong_versions_and_malformed_revisions_are_rejected(self):
        for tag in ("v0.1.3-alpha.21", "v0.1.3-alpha.20-r0", "v0.1.3-alpha.20-r3oops", "v0.1.3-alpha.20-r3-r4", "0.1.3-alpha.20"):
            with self.assertRaises(ValueError):
                verify_release_tag(tag, "0.1.3-alpha.20")


class ReleaseBuildIdentityTests(unittest.TestCase):
    def test_diagnostic_allocator_build_cannot_be_released(self):
        with self.assertRaisesRegex(ValueError, "diagnostic"):
            verify_build_identity({"version": "1.0", "revision": "new", "dirty": False, "allocationDiagnostics": True}, "1.0", "new")

    def test_same_version_from_another_commit_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "build identity"):
            verify_build_identity({"version": "1.0", "revision": "old", "dirty": False}, "1.0", "new")

    def test_dirty_or_missing_identity_is_rejected(self):
        for info in ({}, {"version": "1.0", "revision": "new", "dirty": True}):
            with self.assertRaises(ValueError):
                verify_build_identity(info, "1.0", "new")

    def test_exact_clean_build_is_accepted(self):
        verify_build_identity({"version": "1.0", "revision": "new", "dirty": False}, "1.0", "new")


class FrontendIdentityTests(unittest.TestCase):
    def test_stale_or_missing_frontend_release_identity_is_rejected(self):
        for manifest in ({}, {"rev": "rust-v0.1.3-alpha.6"}):
            with self.assertRaisesRegex(ValueError, "frontend manifest"):
                verify_manifest_version(manifest, "0.1.3-alpha.12")

    def test_matching_frontend_release_identity_is_accepted(self):
        verify_manifest_version({"rev": "rust-v0.1.3-alpha.12"}, "0.1.3-alpha.12")
