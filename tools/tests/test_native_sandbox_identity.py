import copy
import hashlib
import pathlib
import tempfile
import unittest

from tools.native_sandbox_identity import HELPERS, source_identity, verify_record


class NativeIdentityTests(unittest.TestCase):
    def setUp(self):
        self.hashes = {name: hashlib.sha256(name.encode()).hexdigest() for name in HELPERS}
        self.record = {"schemaVersion": 1, "sourceSha256": "a" * 64, "revision": "revision", "productVersion": "1.0.0",
                       "dirty": False, "bridgeBuildInfo": {"sourceSha256": "a" * 64, "revision": "revision", "dirty": False},
                       "helpers": self.hashes.copy()}

    def verify(self, record, **options):
        verify_record(record, "a" * 64, "revision", "1.0.0", self.hashes.__getitem__, **options)

    def test_whole_source_and_all_helpers_must_match(self):
        self.verify(self.record)
        for key, value in [("sourceSha256", "b" * 64), ("revision", "old"), ("productVersion", "0.9.0")]:
            stale = copy.deepcopy(self.record)
            stale[key] = value
            with self.assertRaisesRegex(ValueError, "stale"):
                self.verify(stale)
        for helper in HELPERS:
            stale = copy.deepcopy(self.record)
            stale["helpers"][helper] = "c" * 64
            with self.assertRaisesRegex(ValueError, "checksum"):
                self.verify(stale)

    def test_copied_old_bridge_cannot_be_blessed_by_a_new_manifest(self):
        stale = copy.deepcopy(self.record)
        stale["bridgeBuildInfo"].pop("sourceSha256")
        with self.assertRaisesRegex(ValueError, "stale"):
            self.verify(stale)
        stale = copy.deepcopy(self.record)
        stale["helpers"].pop(HELPERS[-1])
        with self.assertRaisesRegex(ValueError, "three helpers"):
            self.verify(stale)

    def test_development_identity_is_never_accepted_as_a_clean_release(self):
        dirty = copy.deepcopy(self.record)
        dirty["dirty"] = dirty["bridgeBuildInfo"]["dirty"] = True
        with self.assertRaisesRegex(ValueError, "clean"):
            self.verify(dirty)
        self.verify(dirty, require_clean=False)

    def test_source_digest_tracks_native_engine_bridge_lock_and_shared_build_code(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            names = ["native/windows-sandbox/bridge/src/main.rs", "native/windows-sandbox/engine/src/lib.rs",
                     "native/windows-sandbox/Cargo.lock", "Cargo.toml", "crates/host/dsh-cli/build_identity.rs"]
            for name in names:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("original", encoding="utf-8")
            original = source_identity(root)["sha256"]
            for name in names:
                path = root / name
                path.write_text("changed", encoding="utf-8")
                self.assertNotEqual(source_identity(root)["sha256"], original, name)
                path.write_text("original", encoding="utf-8")
            output = root / "native/windows-sandbox/target/release/helper.exe"
            output.parent.mkdir(parents=True)
            output.write_bytes(b"compiler output")
            self.assertEqual(source_identity(root)["sha256"], original)


if __name__ == "__main__":
    unittest.main()
