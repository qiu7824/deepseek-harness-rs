import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
SUBAGENT_TOOL = ROOT / "crates" / "subagent" / "tool-subagent" / "src" / "lib.rs"


class NewSessionContractTests(unittest.TestCase):
    def test_default_preset_tools_use_the_supported_json_schema_subset(self):
        source = SUBAGENT_TOOL.read_text(encoding="utf-8")
        schema = source[
            source.index('fn subagent_parameters('):
            source.index('fn resolve_requested_provider(', source.index('fn subagent_parameters('))
        ]
        self.assertNotIn('"minimum"', schema)
        self.assertIn('"max_tokens"', schema)
        self.assertIn("max_tokens must be at least 1", source)


if __name__ == "__main__":
    unittest.main()
