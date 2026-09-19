import pathlib
import sys
import unittest
import io
import json

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
from e2e_image_generation import fixture_user_index
from e2e_productivity import project_tool_results
from e2e_approval_interactions import ApprovalFixture


class FixtureMessageBoundaryTests(unittest.TestCase):
    def test_runtime_context_after_a_result_does_not_start_another_image_request(self):
        messages = [
            {"role": "user", "content": "image-generation-fixture"},
            {"role": "tool", "tool_call_id": "image-one", "content": "{}"},
            {"role": "user", "content": "Current runtime context. Updated tool catalog."},
        ]
        self.assertEqual(fixture_user_index(messages), 0)

    def test_latest_explicit_structured_request_is_used(self):
        messages = [
            {"role": "user", "content": "image-generation-fixture"},
            {"role": "tool", "content": "{}"},
            {"role": "user", "content": [{"type": "text", "text": "image-edit-fixture: make it blue"}]},
            {"role": "user", "content": "Current runtime context mentions image-generation-fixture."},
        ]
        self.assertEqual(fixture_user_index(messages), 2)

    def test_missing_fixture_request_is_not_silently_accepted(self):
        with self.assertRaisesRegex(AssertionError, "missing explicit"):
            fixture_user_index([{"role": "user", "content": "Current runtime context."}])

    def test_only_the_fixture_task_result_completes_the_project_tool_round(self):
        result = {"role": "tool", "tool_call_id": "tasks-one", "content": "TASK_FIXTURE"}
        messages = [{"role": "tool", "tool_call_id": "other-one", "content": "unrelated"}, result,
                    {"role": "user", "content": "Current runtime context."}]
        self.assertEqual(project_tool_results(messages), [result])

    def test_approval_summary_requests_cannot_emit_unadvertised_tools(self):
        request = {"messages": [{"role": "user", "content": 'Summarize: approval-e2e:{"tool":"read","path":"project/.env","marker":"fixture"}'}]}
        raw = json.dumps(request).encode()
        handler = object.__new__(ApprovalFixture)
        handler.headers = {"Content-Length": str(len(raw))}
        handler.rfile = io.BytesIO(raw)
        replies = []
        handler.send = lambda value, content_type: replies.append(value)
        handler.do_POST()
        self.assertEqual(len(replies), 1)
        self.assertIn("Approval fixture complete.", replies[0])
        self.assertNotIn("tool_calls", replies[0])

    def test_compaction_keeps_its_no_tool_instruction_with_cached_schemas(self):
        request = {"tools": [{"type": "function"}], "messages": [
            {"role": "user", "content": 'approval-e2e:{"tool":"read","path":"project/.env","marker":"fixture"}'},
            {"role": "user", "content": "You are acting as a compaction engine. Output only concise Markdown. Do not call tools."},
        ]}
        raw = json.dumps(request).encode()
        handler = object.__new__(ApprovalFixture)
        handler.headers = {"Content-Length": str(len(raw))}
        handler.rfile = io.BytesIO(raw)
        replies = []
        handler.send = lambda value, content_type: replies.append(value)
        handler.do_POST()
        self.assertIn("Earlier fixture approval interactions", replies[0])
        self.assertNotIn("tool_calls", replies[0])
