from __future__ import annotations

import json
import pathlib
import shutil
import subprocess
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
NODE = shutil.which("node")


@unittest.skipUnless(NODE, "Node.js is required for bundled client performance contracts")
class ClientPerformanceContractTests(unittest.TestCase):
    def run_contract(self, script: str) -> dict[str, object]:
        completed = subprocess.run(
            [NODE, str(ROOT / "tools" / "tests" / script)],
            cwd=ROOT,
            check=True,
            capture_output=True,
            text=True,
            timeout=30,
        )
        lines = [line for line in completed.stdout.splitlines() if line.strip()]
        self.assertTrue(lines, completed.stderr)
        return json.loads(lines[-1])

    def test_partial_trajectory_updates_do_not_rescan_settled_history(self) -> None:
        result = self.run_contract("test_trajectory_builder_perf.mjs")
        self.assertEqual(result["settledNodes"], 2000)
        self.assertEqual(result["partialUpdates"], 500)
        self.assertEqual(result["partialFullScans"], 0)
        self.assertGreater(result["settlementFullScans"], 0)


    def test_oversized_live_stream_compacts_without_cutting_its_prefix(self) -> None:
        result = self.run_contract("test_history_compaction.mjs")
        self.assertEqual(result, {"rawEvents": 5000, "compactedEvents": 1})


if __name__ == "__main__":
    unittest.main()
