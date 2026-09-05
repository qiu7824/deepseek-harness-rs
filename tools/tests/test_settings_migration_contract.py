from __future__ import annotations
import json
import pathlib
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import e2e_settings_model_preserves_data as migration

class SettingsMigrationContractTests(unittest.TestCase):
    def test_fresh_rust_package_accepts_zero_links_and_rejects_regenerated_legacy_slots(self):
        with tempfile.TemporaryDirectory() as directory:
            home = pathlib.Path(directory)
            migration.assert_no_installation_links(home, "fresh boot")
            (home / "profiles/node_modules/plain-package").mkdir(parents=True)
            migration.assert_no_installation_links(home, "fresh boot")
            with patch.object(migration, "managed_profile_links", return_value=["profiles/node_modules/legacy-installation"]):
                with self.assertRaisesRegex(AssertionError, "unnecessary installation module links"):
                    migration.assert_no_installation_links(home, "cold restart")

    def test_rust_inventory_is_not_accepted_as_a_node_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            package = pathlib.Path(directory)
            (package / "web/dist").mkdir(parents=True)
            (package / "config").mkdir()
            binary = package / "deepseek-harness-rs.exe"
            binary.write_bytes(b"host fixture")
            (package / "dsh-launcher.exe").write_bytes(b"launcher fixture")
            manifest = {"name":"deepseek-harness-rs-v1.0.0-windows-x86_64-core", "host":binary.name,
                        "entry":"dsh-launcher.exe", "variant":"core", "platform":"windows"}
            path = package / "PACKAGE.json"
            path.write_text(json.dumps(manifest), encoding="utf-8")
            self.assertEqual(migration.rust_package_manifest(binary), manifest)
            for changes in ({"dependencies":{}}, {"host":"different.exe"}, {"entry":"../external.exe"}):
                path.write_text(json.dumps({**manifest, **changes}), encoding="utf-8")
                with self.assertRaises(AssertionError):
                    migration.rust_package_manifest(binary)

if __name__ == "__main__":
    unittest.main()
