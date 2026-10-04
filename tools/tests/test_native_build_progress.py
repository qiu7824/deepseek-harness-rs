import io
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from build_native_sandbox import COMPILER_LINE_CHARS, LOG_TAIL_BYTES, last_compiler_line


class NativeBuildProgressTests(unittest.TestCase):
    def test_large_compiler_log_is_read_only_through_a_bounded_tail(self):
        class RecordedLog(io.BytesIO):
            def read(self, size=-1):
                self.requests.append((self.tell(), size))
                return super().read(size)

        content = b"old compiler output\n" * 100000 + b"\nCompiling codex-protocol v0.0.0\r\n\r\n"
        log = RecordedLog(content)
        log.requests = []
        with patch.object(Path, "open", return_value=log):
            self.assertEqual(last_compiler_line(Path("native-build.log")), "Compiling codex-protocol v0.0.0")
        self.assertEqual(log.requests, [(len(content) - LOG_TAIL_BYTES, LOG_TAIL_BYTES)])

    def test_partial_unicode_and_long_final_line_remain_bounded(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "native-build.log"
            path.write_bytes(b"prefix\n" + "编译".encode() * LOG_TAIL_BYTES + b"\xff\n\n")
            line = last_compiler_line(path)
            self.assertEqual(len(line), COMPILER_LINE_CHARS)
            self.assertTrue(line.endswith("\ufffd"))
            self.assertIn("编译", line)

    def test_empty_or_unavailable_log_does_not_fail_progress_reporting(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "native-build.log"
            self.assertIsNone(last_compiler_line(path))
            path.write_bytes(b"\n \r\n\t")
            self.assertIsNone(last_compiler_line(path))


if __name__ == "__main__":
    unittest.main()
