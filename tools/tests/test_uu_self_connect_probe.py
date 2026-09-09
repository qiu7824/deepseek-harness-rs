import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("uu_self_probe", Path(__file__).parents[1] / "probe_uu_self_connect.py")
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class SelfConnectProbeTests(unittest.TestCase):
    def test_empty_success_receipt_is_not_connection_proof(self):
        calls = []
        def call(args):
            calls.append(args)
            return {"connected_devices": ["remote"]} if args[1] == "status" else {"devices": []}
        result = probe.probe_connection("local", call, sleep=lambda _: None)
        self.assertEqual(result["status"], "not-established")
        self.assertFalse(result["receiptNamesTarget"])
        self.assertEqual(result["cleanup"], "confirmed")
        self.assertTrue(result["existingConnectionsPreserved"])
        self.assertEqual([args for args in calls if args[1] == "disconnect"], [["device", "disconnect", "local"]])

    def test_existing_self_connection_is_never_disconnected(self):
        calls = []
        def call(args):
            calls.append(args)
            return {"connected_devices": ["local", "remote"]}
        result = probe.probe_connection("local", call)
        self.assertEqual(result["status"], "already-connected-unverified")
        self.assertEqual(calls, [["device", "status"]])

    def test_receipt_without_status_is_still_unverified(self):
        def call(args):
            return {"connected_devices": []} if args[1] == "status" else {"devices": [{"targetId": "local"}]}
        result = probe.probe_connection("local", call, sleep=lambda _: None)
        self.assertTrue(result["receiptNamesTarget"])
        self.assertEqual(result["status"], "not-established")

    def test_confirmed_connection_does_not_claim_video_or_input(self):
        statuses = iter([[], ["local"], []])
        def call(args):
            return {"connected_devices": next(statuses)} if args[1] == "status" else {"devices": [{"targetId": "local"}]}
        result = probe.probe_connection("local", call, sleep=lambda _: None)
        self.assertEqual(result["status"], "connected-media-unverified")
        self.assertEqual(result["mediaVerification"], "not-run")
        self.assertFalse(result["desktopInputSent"])

    def test_failed_connect_still_cleans_pending_request(self):
        calls = []
        def call(args):
            calls.append(args)
            if args[1] == "connect":
                raise probe.ProbeError("CLI_TIMEOUT")
            return {"connected_devices": []} if args[1] == "status" else {"devices": []}
        result = probe.probe_connection("local", call)
        self.assertEqual(result["error"], "CLI_TIMEOUT")
        self.assertIn(["device", "disconnect", "local"], calls)

    def test_unknown_status_does_not_become_empty_success(self):
        with self.assertRaises(probe.ProbeError):
            probe.connection_ids({"devices": []})
        with self.assertRaises(probe.ProbeError):
            probe.connection_ids({"connected_devices": [{"token": "private-value"}]})


if __name__ == "__main__":
    unittest.main()
