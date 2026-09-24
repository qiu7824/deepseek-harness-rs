import copy
import unittest
from tools.e2e_tool_results import tool_result_blocks


class ToolResultTests(unittest.TestCase):
    def event(self, failed=False):
        return {"type": "tool/result", "data": {"message": {
            "role": "tool", "toolCallId": "call", "source": {"kind": "tool", "callId": "call"},
            "isError": failed, "content": [{"type": "text", "text": "payload"}]}}}

    def test_native_and_legacy_preserve_call_content_and_error(self):
        for failed in [False, True]:
            native = self.event(failed)
            expected = {"type": "tool-result", "toolCallId": "call", "isError": failed,
                        "content": [{"type": "text", "text": "payload"}]}
            legacy = {"type": "tool/result", "data": {"message": {"role": "user", "content": [expected]}}}
            for event in [native, legacy]:
                original = copy.deepcopy(event)
                self.assertEqual(tool_result_blocks([{"type": "turn/start"}, event]), [expected])
                self.assertEqual(event, original)

    def test_invalid_ownership_or_error_payload_cannot_look_successful(self):
        for field, value in [("source", {"kind": "user"}), ("toolCallId", "other"),
                             ("isError", "false"), ("content", "not blocks")]:
            event = self.event()
            event["data"]["message"][field] = value
            with self.subTest(field=field), self.assertRaises(ValueError):
                tool_result_blocks([event])

    def test_empty_legacy_result_is_not_silently_ignored(self):
        with self.assertRaises(ValueError):
            tool_result_blocks([{"type": "tool/result", "data": {"message": {"role": "user", "content": []}}}])


if __name__ == "__main__":
    unittest.main()
