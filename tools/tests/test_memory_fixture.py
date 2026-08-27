import hashlib
import importlib.util
import json
import pathlib
import tempfile
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "tools" / "memory_fixture.py"


def load_fixture():
    spec = importlib.util.spec_from_file_location("memory_fixture", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("memory_fixture module is not loadable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class CliTests(unittest.TestCase):
    def test_requires_explicit_output_and_accepts_shape(self):
        fixture = load_fixture()
        args = fixture.parse_args([
            "--output", "dense.jsonl",
            "--events", "68000",
            "--message-groups", "40",
        ])

        self.assertEqual(args.output, "dense.jsonl")
        self.assertEqual(args.events, 68_000)
        self.assertEqual(args.message_groups, 40)

    def test_rejects_implicit_output(self):
        fixture = load_fixture()
        with self.assertRaises(SystemExit):
            fixture.parse_args([])


class FixtureTests(unittest.TestCase):
    def test_generates_exact_dense_synthetic_event_stream(self):
        fixture = load_fixture()

        count = 0
        messages = 0
        deltas = 0
        last_seq = -1
        forbidden = ("Administrator", "DeepSeek Harness", "credential")
        for event in fixture.generate_events(total_events=68_000, message_groups=40):
            self.assertEqual(event["seq"], count)
            rendered = str(event)
            self.assertTrue(all(value.lower() not in rendered.lower() for value in forbidden))
            count += 1
            last_seq = event["seq"]
            messages += int(event["type"].endswith("/message"))
            deltas += int(event["type"] == "assistant/reasoning-delta")

        self.assertEqual(count, 68_000)
        self.assertEqual(last_seq, 67_999)
        self.assertEqual(messages, 80)
        self.assertGreater(deltas, 60_000)

    def test_rejects_impossible_or_unbounded_fixture_shape(self):
        fixture = load_fixture()

        with self.assertRaisesRegex(ValueError, "total_events"):
            list(fixture.generate_events(total_events=5, message_groups=40))
        with self.assertRaisesRegex(ValueError, "message_groups"):
            list(fixture.generate_events(total_events=100, message_groups=0))

    def test_streams_canonical_jsonl_fixture_to_explicit_path(self):
        fixture = load_fixture()
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "dense.jsonl"

            summary = fixture.write_jsonl(
                output,
                fixture.generate_events(total_events=100, message_groups=10),
            )

            lines = output.read_text(encoding="utf-8").splitlines()
            self.assertEqual(summary["events"], 100)
            self.assertEqual(summary["bytes"], output.stat().st_size)
            digest = hashlib.sha256()
            with output.open("rb") as stream:
                while chunk := stream.read(4096):
                    digest.update(chunk)
            self.assertEqual(summary["sha256"], digest.hexdigest())
            self.assertEqual(len(lines), 100)
            self.assertEqual(json.loads(lines[0])["seq"], 0)
            self.assertEqual(json.loads(lines[-1])["seq"], 99)
            self.assertNotIn("source_path", summary)


if __name__ == "__main__":
    unittest.main()
