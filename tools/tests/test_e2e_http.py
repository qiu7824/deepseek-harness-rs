import http.client
from http.server import BaseHTTPRequestHandler
import pathlib
import sys
import threading
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from e2e_http import ThreadingHTTPServer


class FixtureServerTests(unittest.TestCase):
    def test_session_manifest_fingerprints_native_and_legacy_but_not_temporary_files(self):
        from e2e_settings_model_preserves_data import session_manifest
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            names = ["session.jsonl", "session.jsonl.zstd", "session.v4.jsonl", "session.v4.jsonl.zstd"]
            for index, name in enumerate(names):
                path = root / str(index) / name
                path.parent.mkdir()
                path.write_bytes(b"published")
            (root / "session.v4.jsonl.tmp").write_bytes(b"not committed")
            (root / "session.v4.jsonl.backup").write_bytes(b"backup")
            before = session_manifest(root)
            self.assertEqual(len(before), 4)
            (root / "2/session.v4.jsonl").write_bytes(b"changed")
            after = session_manifest(root)
            self.assertNotEqual(before["2/session.v4.jsonl"], after["2/session.v4.jsonl"])
            self.assertEqual(before.keys(), after.keys())

    def test_bind_does_not_resolve_loopback_hostname(self):
        with patch("socket.getfqdn", side_effect=AssertionError("reverse DNS must not run")):
            with ThreadingHTTPServer(("127.0.0.1", 0), BaseHTTPRequestHandler) as server:
                self.assertEqual(server.server_name, "127.0.0.1")
                self.assertEqual(server.server_port, server.socket.getsockname()[1])
                self.assertGreater(server.server_port, 0)

    def test_local_request_preserves_http_fixture_behavior(self):
        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                content = self.rfile.read(int(self.headers["Content-Length"]))
                self.send_response(200)
                self.send_header("Content-Length", str(len(content)))
                self.end_headers()
                self.wfile.write(content)

        with ThreadingHTTPServer(("127.0.0.1", 0), Handler) as server:
            worker = threading.Thread(target=server.serve_forever, daemon=True)
            worker.start()
            connection = http.client.HTTPConnection("127.0.0.1", server.server_port, timeout=2)
            try:
                payload = "fixture 工具往返".encode()
                connection.request("POST", "/fixture", payload)
                response = connection.getresponse()
                self.assertEqual(response.status, 200)
                self.assertEqual(response.read(), payload)
            finally:
                connection.close()
                server.shutdown()
                worker.join(timeout=2)
            self.assertFalse(worker.is_alive())


if __name__ == "__main__":
    unittest.main()
