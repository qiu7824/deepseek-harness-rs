from __future__ import annotations

import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import verify_windows_installer_behavior as verifier


class InstallerCompileGateTests(unittest.TestCase):
    def test_warning_is_fatal_even_when_compiler_reports_success(self):
        with tempfile.TemporaryDirectory() as temporary:
            log = Path(temporary) / "compile.log"
            log.write_text('Warning: A message has not been defined for the "chinesesimp" language.\nSuccessful compile.\n', encoding="utf-8-sig")
            with self.assertRaisesRegex(SystemExit, "emitted warnings"):
                verifier.require_warning_free_compile(log)
            log.write_text("Successful compile.\n", encoding="utf-8")
            verifier.require_warning_free_compile(log)

    @unittest.skipUnless(os.name == "nt", "Windows installer control flow")
    def test_success_exit_with_warning_stops_before_any_installation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            scratch = root / "scratch"
            def compile_with_warning(_command, log):
                log.write_text('Warning: Custom message "ErrorFileHash2" has not been defined.\nSuccessful compile.\n', encoding="utf-8")
                return 0
            with patch.object(sys, "argv", ["verify", "--compiler", str(root / "compiler.exe"), "--stage", str(root / "stage"), "--scratch", str(scratch)]), patch.object(verifier, "prepare", return_value=(scratch / "isolated.iss", {"variant": "core"})), patch.object(verifier, "run", side_effect=compile_with_warning) as run:
                with self.assertRaisesRegex(SystemExit, "emitted warnings"):
                    verifier.main()
            self.assertEqual(run.call_count, 1)
            self.assertFalse((scratch / "installed").exists())


if __name__ == "__main__":
    unittest.main()
