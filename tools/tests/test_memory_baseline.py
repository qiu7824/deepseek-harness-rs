import importlib.util
import json
import pathlib
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "tools" / "validate_memory_baseline.py"


def load_validator():
    spec = importlib.util.spec_from_file_location("validate_memory_baseline", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("validator module is not loadable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def sample_records():
    snapshot = {
        "schema_version": 1,
        "type": "snapshot",
        "label": "baseline",
        "timestamp": "now",
        "pid": 10,
        "working_set_bytes": 1048576,
        "private_bytes": 2097152,
        "threads": 3,
        "handles": 4,
        "children": {},
        "binary_sha256": "a" * 64,
        "home_path_sha256": "b" * 64,
    }
    workload = {
        "schema_version": 1,
        "type": "workload",
        "label": "list_20",
        "cumulative_requests": 20,
        "method": "session.list",
        "requests": 20,
        "response_bytes": 1048576,
        "elapsed_seconds": 1.25,
    }
    return [snapshot, workload]


class BaselineValidatorTests(unittest.TestCase):
    def test_accepts_markdown_derived_from_jsonl(self):
        validator = load_validator()
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            report = root / "report.jsonl"
            report.write_text("\n".join(json.dumps(x) for x in sample_records()) + "\n", encoding="utf-8")
            markdown = root / "report.md"
            markdown.write_text(validator.render_evidence_markdown(report), encoding="utf-8")

            result = validator.validate(report, markdown)

            self.assertEqual(result["records"], 2)
            self.assertEqual(result["snapshots"], 1)
            self.assertEqual(result["workloads"], 1)

    def test_rejects_markdown_identity_or_table_drift(self):
        validator = load_validator()
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            report = root / "report.jsonl"
            report.write_text("\n".join(json.dumps(x) for x in sample_records()) + "\n", encoding="utf-8")
            markdown = root / "report.md"
            expected = validator.render_evidence_markdown(report)
            markdown.write_text(expected.replace("PID：`10`", "PID：`11`"), encoding="utf-8")

            with self.assertRaisesRegex(RuntimeError, "markdown evidence drift"):
                validator.validate(report, markdown)

    def test_rejects_wrong_schema_record_count_and_plain_home(self):
        validator = load_validator()
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            report = root / "report.jsonl"
            bad = sample_records()
            bad[0]["schema_version"] = 2
            report.write_text("\n".join(json.dumps(x) for x in bad) + "\nDeepSeek Harness\n", encoding="utf-8")
            markdown = root / "report.md"
            markdown.write_text("", encoding="utf-8")

            with self.assertRaises(RuntimeError):
                validator.validate(report, markdown)


if __name__ == "__main__":
    unittest.main()
