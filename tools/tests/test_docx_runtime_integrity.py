import importlib.util
import os
from pathlib import Path
import tempfile
import unittest


class DocxRuntimeIntegrity(unittest.TestCase):
    def test_missing_or_mixed_dialog_renderer_is_rejected(self):
        spec = importlib.util.spec_from_file_location("package_release_docx", Path(__file__).parents[1] / "package_release.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory(dir=os.environ.get("DSH_TEST_TEMP_DIR")) as directory:
            root = Path(directory)
            core = root / "web/dist/plugins/docx-preview-runtime.js"
            sidebar = root / "release/plugins/dsh-sidebar-workbench-suite/lib/docx.js"
            for path in (core, sidebar):
                path.parent.mkdir(parents=True, exist_ok=True)
            sidebar.write_bytes(b"pinned renderer")
            with self.assertRaisesRegex(ValueError, "DOCX renderers"):
                module.verify_docx_runtime(root)
            core.write_bytes(b"obsolete renderer")
            with self.assertRaisesRegex(ValueError, "DOCX renderers"):
                module.verify_docx_runtime(root)
            core.write_bytes(sidebar.read_bytes())
            module.verify_docx_runtime(root)


if __name__ == "__main__":
    unittest.main()
