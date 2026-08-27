import importlib.util
import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
MODULE_PATH = ROOT / "tools" / "memory_probe.py"


def load_probe():
    spec = importlib.util.spec_from_file_location("memory_probe", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("memory_probe module is not loadable")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ProcessSnapshotTests(unittest.TestCase):
    def test_parses_all_required_host_metrics_and_children(self):
        probe = load_probe()
        host = """
        HandleCount=198

        Name=dsh.exe

        PrivatePageCount=165576704

        ProcessId=12592

        ThreadCount=18

        WorkingSetSize=99389440
        """
        children = """
        Name=git.exe

        ParentProcessId=12592

        ProcessId=14000



        Name=pwsh.exe

        ParentProcessId=12592

        ProcessId=14001
        """

        snapshot = probe.parse_process_snapshot(host, children, expected_pid=12592)

        self.assertEqual(snapshot["working_set_bytes"], 99389440)
        self.assertEqual(snapshot["private_bytes"], 165576704)
        self.assertEqual(snapshot["threads"], 18)
        self.assertEqual(snapshot["handles"], 198)
        self.assertEqual(snapshot["children"], {"git.exe": 1, "pwsh.exe": 1})

    def test_rejects_incomplete_host_metrics(self):
        probe = load_probe()
        host = """
        HandleCount=198
        ProcessId=12592
        ThreadCount=18
        WorkingSetSize=99389440
        """

        with self.assertRaisesRegex(RuntimeError, "missing process metrics: private_bytes"):
            probe.parse_process_snapshot(host, "", expected_pid=12592)

    def test_rejects_metrics_for_the_wrong_pid(self):
        probe = load_probe()
        host = """
        HandleCount=198
        PrivatePageCount=165576704
        ProcessId=99999
        ThreadCount=18
        WorkingSetSize=99389440
        """

        with self.assertRaisesRegex(RuntimeError, "expected PID 12592"):
            probe.parse_process_snapshot(host, "", expected_pid=12592)


class RecordTests(unittest.TestCase):
    def test_record_binds_metrics_to_binary_and_home_without_exposing_home(self):
        probe = load_probe()
        snapshot = {
            "pid": 12592,
            "working_set_bytes": 10,
            "private_bytes": 20,
            "threads": 3,
            "handles": 4,
            "children": {},
        }

        record = probe.build_record(
            label="warm",
            snapshot=snapshot,
            binary_bytes=b"formal-release",
            home_path=r"C:\\Users\\Administrator\\AppData\\Local\\DeepSeek Harness",
            timestamp="2026-08-26T18:00:00+08:00",
        )

        self.assertEqual(record["label"], "warm")
        self.assertEqual(record["pid"], 12592)
        self.assertEqual(len(record["binary_sha256"]), 64)
        self.assertEqual(len(record["home_path_sha256"]), 64)
        self.assertNotIn("DeepSeek Harness", str(record))
        self.assertNotIn("command_line", record)

    def test_record_rejects_missing_required_metrics(self):
        probe = load_probe()

        with self.assertRaisesRegex(RuntimeError, "missing snapshot fields: handles"):
            probe.build_record(
                label="bad",
                snapshot={
                    "pid": 1,
                    "working_set_bytes": 2,
                    "private_bytes": 3,
                    "threads": 4,
                    "children": {},
                },
                binary_bytes=b"release",
                home_path="home",
                timestamp="now",
            )


class LiveSnapshotTests(unittest.TestCase):
    def test_collect_snapshot_queries_listener_and_exact_process_tree(self):
        probe = load_probe()
        commands = []

        def runner(argv):
            commands.append(argv)
            if argv[0] == "netstat":
                return "TCP  127.0.0.1:58080  0.0.0.0:0  LISTENING  12592"
            if "ProcessId=12592" in argv:
                return "HandleCount=9\nPrivatePageCount=20\nProcessId=12592\nThreadCount=3\nWorkingSetSize=10\n"
            if "ParentProcessId=12592" in argv:
                return "Name=git.exe\nParentProcessId=12592\nProcessId=13000\n"
            raise AssertionError(argv)

        snapshot = probe.collect_snapshot(port=58080, runner=runner)

        self.assertEqual(snapshot["pid"], 12592)
        self.assertEqual(snapshot["children"], {"git.exe": 1})
        flattened = " ".join(part for command in commands for part in command)
        self.assertNotIn("CommandLine", flattened)
        self.assertEqual(commands[0], ["netstat", "-ano", "-p", "tcp"])


class ExecutableIdentityTests(unittest.TestCase):
    def test_parses_executable_path_for_exact_pid(self):
        probe = load_probe()
        output = """
        ExecutablePath=D:\\deepwork\\deepseek-harness-rs\\target\\release\\dsh.exe

        ProcessId=12592
        """

        self.assertEqual(
            probe.parse_executable_path(output, expected_pid=12592),
            r"D:\deepwork\deepseek-harness-rs\target\release\dsh.exe",
        )

    def test_rejects_wrong_or_missing_executable_identity(self):
        probe = load_probe()

        with self.assertRaisesRegex(RuntimeError, "expected PID 12592"):
            probe.parse_executable_path(
                "ExecutablePath=D:\\other\\dsh.exe\nProcessId=99999\n",
                expected_pid=12592,
            )

    def test_asserts_running_binary_matches_explicit_binary(self):
        probe = load_probe()

        with self.assertRaisesRegex(RuntimeError, "running listener binary does not match"):
            probe.assert_running_binary(
                r"D:\running\dsh.exe",
                r"D:\expected\dsh.exe",
            )


class ListenerPidTests(unittest.TestCase):
    def test_selects_exact_loopback_listener_instead_of_wrapper_pid(self):
        probe = load_probe()
        netstat = """
          TCP    127.0.0.1:58080      0.0.0.0:0       LISTENING       12592
          TCP    127.0.0.1:58080      127.0.0.1:51001 ESTABLISHED     12592
          TCP    127.0.0.1:51001      127.0.0.1:58080 ESTABLISHED     1760
        """

        self.assertEqual(probe.parse_listener_pid(netstat, 58080), 12592)

    def test_rejects_ambiguous_listener_pids(self):
        probe = load_probe()
        netstat = """
          TCP    127.0.0.1:58080      0.0.0.0:0       LISTENING       12592
          TCP    127.0.0.1:58080      0.0.0.0:0       LISTENING       13000
        """

        with self.assertRaisesRegex(RuntimeError, "multiple listener PIDs"):
            probe.parse_listener_pid(netstat, 58080)

    def test_rejects_non_loopback_or_non_listening_rows(self):
        probe = load_probe()
        netstat = """
          TCP    0.0.0.0:58080        0.0.0.0:0       LISTENING       12592
          TCP    127.0.0.1:58080      127.0.0.1:51001 ESTABLISHED     12592
        """

        with self.assertRaisesRegex(RuntimeError, "no exact loopback listener"):
            probe.parse_listener_pid(netstat, 58080)


if __name__ == "__main__":
    unittest.main()
