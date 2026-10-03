import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import run_flutter_test_diagnostics as diagnostics


class FlutterTestDiagnosticsTests(unittest.TestCase):
    def test_flutter_assertion_prints_are_attached_only_to_their_failed_test(self):
        failures = diagnostics.Failures()
        failures.consume(json.dumps({"type": "testStart", "test": {"id": 7, "name": "semantic icon matrix light at 1.0"}}))
        failures.consume(json.dumps({"type": "print", "testID": 7, "message": "x" * 10000}))
        failures.consume(json.dumps({"type": "print", "testID": 7, "message": "══╡ EXCEPTION CAUGHT BY FLUTTER TEST FRAMEWORK ╞══\nGolden failed: 0.01% of pixels differ\nActual image: icons_light_1.0x.png"}))
        failures.consume(json.dumps({"type": "error", "testID": 7, "error": "Test failed. See exception logs above."}))
        failures.consume(json.dumps({"type": "testDone", "testID": 7, "result": "failure"}))
        for test_id in range(8, 1008):
            failures.consume(json.dumps({"type": "print", "testID": test_id, "message": "unrelated later passing output"}))
            failures.consume(json.dumps({"type": "testDone", "testID": test_id, "result": "success"}))
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            failures.annotate(1)
        self.assertIn("semantic icon matrix light at 1.0%0AEXCEPTION CAUGHT BY", output.getvalue())
        self.assertIn("Golden failed: 0.01%25 of pixels differ", output.getvalue())
        self.assertIn("Actual image: icons_light_1.0x.png", output.getvalue())
        self.assertIn("Test failed. See exception logs above.", output.getvalue())
        self.assertNotIn("unrelated later passing output", output.getvalue())
        self.assertNotIn("x" * 100, output.getvalue())
        self.assertLessEqual(len(failures.prints[7]), diagnostics.PRINT_CONTEXT_CHARS)

    def test_early_errors_survive_many_later_passing_events_and_escape_annotations(self):
        failures = diagnostics.Failures()
        failures.consume(json.dumps({"type": "testStart", "test": {"id": 4, "name": "图片 100%\r\n失败"}}))
        failures.consume(json.dumps({"type": "error", "testID": 4, "error": "Expected 100%\nActual 0%", "stackTrace": "test/example_test.dart:42"}))
        failures.consume(json.dumps({"type": "testDone", "testID": 4, "result": "failure"}))
        for test_id in range(5, 2005):
            failures.consume(json.dumps({"type": "testStart", "test": {"id": test_id, "name": f"passing {test_id}"}}))
            failures.consume(json.dumps({"type": "testDone", "testID": test_id, "result": "success"}))
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            failures.annotate(1)
        self.assertEqual(output.getvalue().count("::error"), 1)
        self.assertIn("图片 100%25%0D%0A失败%0AExpected 100%25%0AActual 0%25", output.getvalue())
        self.assertIn("test/example_test.dart:42", output.getvalue())
        self.assertNotIn("passing 2004", output.getvalue())

    def test_bounded_annotations_and_missing_error_event(self):
        failures = diagnostics.Failures()
        for test_id in range(diagnostics.MAX_FAILURE_ANNOTATIONS + 3):
            failures.consume(json.dumps({"type": "testStart", "test": {"id": test_id, "name": f"failure {test_id}"}}))
            failures.consume(json.dumps({"type": "testDone", "testID": test_id, "result": "error"}))
        failures.consume(json.dumps({"type": "error", "testID": 0, "error": "x" * 10000}))
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            failures.annotate(101)
        annotations = output.getvalue().splitlines()
        self.assertEqual(len(annotations), diagnostics.MAX_FAILURE_ANNOTATIONS + 1)
        self.assertIn("3 additional failing tests", annotations[-1])
        self.assertLessEqual(len(annotations[0].split("::", 2)[-1]), diagnostics.ANNOTATION_CHARS + 3)
        self.assertIn("machine reporter marked this test as failed", annotations[1])

    def test_child_exit_stream_log_and_unterminated_machine_event_are_preserved(self):
        tools = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            fixture = root / "machine_child.py"
            fixture.write_text(
                "import json, sys\n"
                "print('ordinary stderr', file=sys.stderr, flush=True)\n"
                "print(json.dumps({'type':'testStart','test':{'id':8,'name':'早期失败'}}), flush=True)\n"
                "print(json.dumps({'type':'error','testID':8,'error':'exact assertion'}), flush=True)\n"
                "for i in range(300): print(json.dumps({'type':'testDone','testID':100+i,'result':'success'}))\n"
                "sys.stdout.write(json.dumps({'type':'testDone','testID':8,'result':'failure'}))\n"
                "sys.stdout.flush()\n"
                "raise SystemExit(23)\n",
                encoding="utf-8",
            )
            log = root / "machine.log"
            result = subprocess.run(
                [sys.executable, str(tools / "run_release_regression.py"), "--log", str(log), "--", sys.executable, str(tools / "run_flutter_test_diagnostics.py"), "--", sys.executable, str(fixture)],
                capture_output=True,
                text=True,
                encoding="utf-8",
            )
            self.assertEqual(result.returncode, 23, result.stderr)
            self.assertIn("ordinary stderr", result.stdout)
            self.assertIn('"testID": 399', result.stdout)
            self.assertIn("::error title=Flutter test failure::早期失败%0Aexact assertion", result.stdout)
            self.assertIn("::error title=Flutter test failure::早期失败%0Aexact assertion", log.read_text(encoding="utf-8"))

    def test_success_does_not_annotate_non_json_output_or_retried_errors(self):
        failures = diagnostics.Failures()
        failures.consume("not json")
        failures.consume("[]")
        failures.consume(json.dumps({"type": "error", "testID": None, "error": "retried"}))
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            failures.annotate(0)
        self.assertEqual(output.getvalue(), "")


if __name__ == "__main__":
    unittest.main()
