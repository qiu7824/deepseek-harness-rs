import unittest

from tools.verify_release_version import verify_build_identity


class ReleaseBuildIdentityTests(unittest.TestCase):
    def test_same_version_from_another_commit_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "build identity"):
            verify_build_identity({"version": "1.0", "revision": "old", "dirty": False}, "1.0", "new")

    def test_dirty_or_missing_identity_is_rejected(self):
        for info in ({}, {"version": "1.0", "revision": "new", "dirty": True}):
            with self.assertRaises(ValueError):
                verify_build_identity(info, "1.0", "new")

    def test_exact_clean_build_is_accepted(self):
        verify_build_identity({"version": "1.0", "revision": "new", "dirty": False}, "1.0", "new")
