import pathlib
import tempfile
import unittest
import os
import json
from tools.build_sidebar_assets import validate_inputs, browser_entry


class SidebarAssetResolutionTests(unittest.TestCase):
    def test_import_condition_wins_over_legacy_umd_main(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("DSH_TEST_TEMP_DIR")) as folder:
            root = pathlib.Path(folder)
            (root / "module.mjs").write_text("export const value=1", encoding="utf8")
            (root / "legacy.js").write_text("globalThis.value=1", encoding="utf8")
            (root / "package.json").write_text(json.dumps({"exports": {".": {"types": "./types.d.ts", "import": "./module.mjs", "default": "./legacy.js"}}, "main": "legacy.js"}), encoding="utf8")
            self.assertEqual(browser_entry(root), (root / "module.mjs").resolve())
            (root / "package.json").write_text(json.dumps({"main": "./server", "browser": {"./server": "./legacy.js"}}), encoding="utf8")
            self.assertEqual(browser_entry(root), (root / "legacy.js").resolve())

    def test_ancestor_dependency_cannot_shadow_the_pinned_tree(self):
        with tempfile.TemporaryDirectory(dir=os.environ.get("DSH_TEST_TEMP_DIR")) as folder:
            root = pathlib.Path(folder)
            modules = root / "pinned/node_modules"
            sources = root / "source/vendor-src"
            generated = root / "build"
            for path in [modules, sources, generated]:
                path.mkdir(parents=True)
            validate_inputs({"inputs": {str(modules / "pdfjs-dist/build/pdf.mjs"): {}, str(sources / "pdf.js"): {}, str(generated / "pdf-assets.js"): {}}}, modules, sources, generated, root)
            for foreign in [root / "node_modules/pdfjs-dist/build/pdf.mjs", root / "pinned/node_modules-other/pdf.mjs"]:
                with self.assertRaisesRegex(ValueError, "unpinned input"):
                    validate_inputs({"inputs": {str(foreign): {}}}, modules, sources, generated, root)


if __name__ == "__main__":
    unittest.main()
