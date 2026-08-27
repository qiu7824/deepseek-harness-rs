import importlib.util
import json
import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "tools" / "memory_scenarios.py"


def load_scenarios():
    spec = importlib.util.spec_from_file_location("memory_scenarios", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("memory_scenarios module is not loadable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class MatrixTests(unittest.TestCase):
    def test_preflights_list_and_history_before_any_repetition_batch(self):
        scenarios = load_scenarios()
        calls = []

        def transport(_url, body):
            calls.append(body["method"])
            if body["method"] == "session.history":
                return json.dumps({"type":"server-response","rpcId":body["rpcId"],"result":{"ok":False,"error":{"message":"session missing"}}}).encode(), 0.001
            return json.dumps({"type":"server-response","rpcId":body["rpcId"],"result":{"ok":True,"value":{}}}).encode(), 0.001

        with self.assertRaisesRegex(RuntimeError, "session.history failed"):
            scenarios.run_default_matrix(
                base_url="http://127.0.0.1:58080",
                history_session_id="missing",
                snapshotter=lambda label: {"label": label},
                transport=transport,
            )

        self.assertEqual(calls, ["session.list", "session.history"])

    def test_rejects_history_target_without_more_pages(self):
        scenarios = load_scenarios()
        calls = []

        def transport(_url, body):
            calls.append(body["method"])
            value = {"hasMore": False} if body["method"] == "session.history" else {}
            response = {"type":"server-response","rpcId":body["rpcId"],"result":{"ok":True,"value":value}}
            return json.dumps(response).encode(), 0.001

        with self.assertRaisesRegex(RuntimeError, "history preflight requires hasMore=true"):
            scenarios.run_default_matrix(
                base_url="http://127.0.0.1:58080",
                history_session_id="short",
                snapshotter=lambda label: {"label": label},
                transport=transport,
            )

        self.assertEqual(calls, ["session.list", "session.history"])

    def test_runs_second_batch_slope_matrix_and_samples_each_boundary(self):
        scenarios = load_scenarios()
        labels = []
        calls = []

        def snapshotter(label):
            labels.append(label)
            return {"label": label, "working_set_bytes": len(labels)}

        def transport(_url, body):
            calls.append(body["method"])
            value = {"hasMore": True} if body["method"] == "session.history" else {}
            return json.dumps({"type":"server-response","rpcId":body["rpcId"],"result":{"ok":True,"value":value}}).encode(), 0.001

        result = scenarios.run_default_matrix(
            base_url="http://127.0.0.1:58080",
            history_session_id="session-long",
            snapshotter=snapshotter,
            transport=transport,
        )

        self.assertEqual(labels, [
            "baseline",
            "list_20",
            "list_100",
            "list_second_100",
            "history_20",
            "history_100",
            "history_second_100",
        ])
        self.assertEqual(calls.count("session.list"), 201)
        self.assertEqual(calls.count("session.history"), 201)
        self.assertEqual(len(result["snapshots"]), 7)
        self.assertEqual(
            [workload["requests"] for workload in result["workloads"]],
            [20, 80, 100, 20, 80, 100],
        )
        self.assertEqual(
            [workload["cumulative_requests"] for workload in result["workloads"]],
            [20, 100, 200, 20, 100, 200],
        )


class ScenarioTests(unittest.TestCase):
    def test_repeats_read_only_rpc_and_accumulates_bytes_and_latency(self):
        scenarios = load_scenarios()
        calls = []
        responses = []

        def transport(url, body):
            calls.append((url, body))
            response = json.dumps({"type":"server-response","rpcId":body["rpcId"],"result":{"ok":True,"value":{}}}).encode()
            responses.append(response)
            return response, 0.012

        result = scenarios.run_rpc_scenario(
            base_url="http://127.0.0.1:58080",
            method="session.list",
            payload={},
            repetitions=3,
            transport=transport,
        )

        self.assertEqual(result["requests"], 3)
        self.assertEqual(result["response_bytes"], sum(map(len, responses)))
        self.assertAlmostEqual(result["elapsed_seconds"], 0.036)
        self.assertEqual([url for url, _ in calls], [
            "http://127.0.0.1:58080/api/session.list",
        ] * 3)
        self.assertTrue(all(body["method"] == "session.list" for _, body in calls))

    def test_rejects_mutating_or_unknown_rpc_methods(self):
        scenarios = load_scenarios()

        with self.assertRaisesRegex(ValueError, "read-only RPC allowlist"):
            scenarios.run_rpc_scenario(
                base_url="http://127.0.0.1:58080",
                method="session.updateTodos",
                payload={},
                repetitions=1,
                transport=lambda _url, _body: (b"{}", 0.0),
            )

    def test_rejects_non_positive_repetition_count(self):
        scenarios = load_scenarios()

        with self.assertRaisesRegex(ValueError, "repetitions must be positive"):
            scenarios.run_rpc_scenario(
                base_url="http://127.0.0.1:58080",
                method="session.list",
                payload={},
                repetitions=0,
                transport=lambda _url, _body: (b"{}", 0.0),
            )

    def test_stops_on_rpc_business_error(self):
        scenarios = load_scenarios()

        def transport(_url, body):
            response = {"type":"server-response","rpcId":body["rpcId"],"result":{"ok":False,"error":{"message":"session missing"}}}
            return json.dumps(response).encode(), 0.001

        with self.assertRaisesRegex(RuntimeError, "session.list failed: session missing"):
            scenarios.run_rpc_scenario(
                base_url="http://127.0.0.1:58080",
                method="session.list",
                payload={},
                repetitions=3,
                transport=transport,
            )

    def test_rejects_wrong_response_type_rpc_id_and_missing_value(self):
        scenarios = load_scenarios()
        invalid = [
            {"type":"other","rpcId":"memory-probe-0","result":{"ok":True,"value":{}}},
            {"type":"server-response","rpcId":"wrong","result":{"ok":True,"value":{}}},
            {"type":"server-response","rpcId":"memory-probe-0","result":{"ok":True}},
            {},
        ]
        for response in invalid:
            with self.subTest(response=response):
                with self.assertRaisesRegex(RuntimeError, "invalid RPC response"):
                    scenarios.run_rpc_scenario(
                        base_url="http://127.0.0.1:58080",
                        method="session.list",
                        payload={},
                        repetitions=1,
                        transport=lambda _url, _body, value=response: (json.dumps(value).encode(), 0.001),
                    )


class CliTests(unittest.TestCase):
    def test_requires_explicit_production_identity_arguments(self):
        scenarios = load_scenarios()
        args = scenarios.parse_args([
            "--binary", "target/release/dsh.exe",
            "--home", "formal-home",
            "--history-session", "session-long",
            "--output", "report.jsonl",
        ])

        self.assertEqual(args.port, 58080)
        self.assertEqual(args.history_session, "session-long")
        self.assertEqual(args.output, "report.jsonl")

    def test_rejects_non_production_port(self):
        scenarios = load_scenarios()
        with self.assertRaisesRegex(ValueError, "formal production port is fixed at 58080"):
            scenarios.parse_args([
                "--binary", "target/release/dsh.exe",
                "--home", "formal-home",
                "--history-session", "session-long",
                "--output", "report.jsonl",
                "--port", "58081",
            ])

    def test_completion_output_does_not_expose_absolute_path(self):
        scenarios = load_scenarios()
        rendered = scenarios.render_completion(
            pathlib.Path(r"C:\\Users\\Administrator\\secret\\report.jsonl"),
            report_bytes=b"report",
            snapshots=7,
        )
        self.assertNotIn("Administrator", rendered)
        self.assertNotIn("secret", rendered)
        self.assertIn('"report_sha256"', rendered)

    def test_rejects_implicit_production_identity(self):
        scenarios = load_scenarios()

        with self.assertRaises(SystemExit):
            scenarios.parse_args([])


class IdentityGateTests(unittest.TestCase):
    def test_snapshotter_collects_a_fresh_sample_for_each_label(self):
        scenarios = load_scenarios()
        calls = []

        def collect(port):
            calls.append(port)
            return {"pid": 10, "working_set_bytes": len(calls), "private_bytes": 2, "threads": 3, "handles": 4, "children": {}}

        snapshotter = scenarios.make_snapshotter(
            port=58080,
            binary_bytes=b"release",
            home_path="home",
            expected_pid=10,
            collect=collect,
            timestamp=lambda: "now",
        )

        first = snapshotter("first")
        second = snapshotter("second")

        self.assertEqual(calls, [58080, 58080])
        self.assertEqual(first["working_set_bytes"], 1)
        self.assertEqual(second["working_set_bytes"], 2)

    def test_snapshotter_rejects_pid_change_during_matrix(self):
        scenarios = load_scenarios()
        snapshots = iter([
            {"pid": 10, "working_set_bytes": 1, "private_bytes": 2, "threads": 3, "handles": 4, "children": {}},
            {"pid": 11, "working_set_bytes": 1, "private_bytes": 2, "threads": 3, "handles": 4, "children": {}},
        ])
        snapshotter = scenarios.make_snapshotter(
            port=58080,
            binary_bytes=b"release",
            home_path="home",
            expected_pid=10,
            collect=lambda _port: next(snapshots),
            timestamp=lambda: "now",
        )

        snapshotter("baseline")
        with self.assertRaisesRegex(RuntimeError, "listener PID changed"):
            snapshotter("next")


class ReportTests(unittest.TestCase):
    def test_writes_typed_jsonl_without_plain_home_path(self):
        scenarios = load_scenarios()
        lines = scenarios.render_jsonl_report(
            snapshots=[{"label": "baseline", "home_path_sha256": "a" * 64}],
            workloads=[{"label": "list_20", "requests": 20}],
        )

        self.assertEqual(len(lines.splitlines()), 2)
        self.assertIn('"type":"snapshot"', lines)
        self.assertIn('"type":"workload"', lines)
        self.assertEqual(lines.count('"schema_version":1'), 2)
        self.assertNotIn("DeepSeek Harness", lines)


if __name__ == "__main__":
    unittest.main()
