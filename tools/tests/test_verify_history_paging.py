import importlib.util
import pathlib
import unittest
from unittest import mock


MODULE_PATH = pathlib.Path(__file__).resolve().parents[1] / "verify_history_paging.py"
SPEC = importlib.util.spec_from_file_location("verify_history_paging", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


class VerifyHistoryPagingTests(unittest.TestCase):
    def test_compacted_event_end_seq_is_the_page_cursor(self):
        page = {
            "events": [
                {
                    "event": {
                        "seq": 25,
                        "data": {"__historyEndSeq": 4120},
                    }
                }
            ],
            "firstSeq": 25,
            "lastSeq": 4120,
            "hasMoreBefore": True,
            "hasMoreAfter": False,
        }
        with mock.patch.object(MODULE, "rpc", return_value=page):
            result = MODULE.verify("http://unused", "session", 25, 1)
        self.assertEqual(result["windows"][0]["lastSeq"], 4120)
        self.assertEqual(result["uniqueEvents"], 1)


if __name__ == "__main__":
    unittest.main()
