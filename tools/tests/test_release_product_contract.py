from __future__ import annotations

import json
import pathlib
import re
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
from tools.verify_release_version import workspace_version

VERSION = workspace_version()
VARIANTS = ("core", "skin", "free")
PLATFORMS = (
    ("windows", "x86_64", "zip", "setup.exe"),
    ("linux", "x86_64", "tar.gz", "deb"),
    ("macos", "x86_64", "tar.gz", "pkg"),
    ("macos", "aarch64", "tar.gz", "pkg"),
)


def workflow_step(workflow: str, name: str) -> str:
    marker = f"      - name: {name}\n"
    start = workflow.index(marker)
    end = workflow.find("\n      - ", start + len(marker))
    return workflow[start:] if end < 0 else workflow[start:end]


def tool_row(source: str, tool_id: str) -> str:
    marker = f"    - id: {tool_id}\n"
    start = source.index(marker)
    end = source.find("\n    - id: ", start + len(marker))
    return source[start:] if end < 0 else source[start:end]


class ReleaseProductContractTests(unittest.TestCase):
    def preset(self, preset_id: str, name: str = "agent.cordis.yml") -> str:
        return (ROOT / "config" / "agent-presets" / preset_id / name).read_text(
            encoding="utf-8"
        )

    def test_ptc_disables_workflow_but_retains_engine_ralph_and_run_code(self):
        ptc = self.preset("code")
        workflow = tool_row(ptc, "tool-workflow")
        self.assertIn("disabled: true", workflow)
        self.assertIn("workflowEngine: true", ptc)
        self.assertIn("- id: workflow-worker-thread", ptc)
        self.assertIn("- id: tool-ralph", ptc)
        self.assertIn("mode: code", ptc)

        description = self.preset("code", "preset.yml")
        self.assertIn("默认不提供 workflow 工具", description)
        self.assertNotIn("具备标准模式的全部能力", description)

    def test_standard_and_cordis_keep_workflow_enabled(self):
        for preset_id in ("standard", "cordis"):
            source = self.preset(preset_id)
            workflow = tool_row(source, "tool-workflow")
            self.assertNotIn("disabled: true", workflow, preset_id)
            self.assertIn("workflowEngine: true", source, preset_id)
            self.assertIn("- id: tool-ralph", source, preset_id)

    def test_nonminimal_presets_expose_the_native_web_fetch_tool(self):
        for preset_id in ("code", "standard", "cordis"):
            source = self.preset(preset_id)
            marker = "- id: tool-web\n"
            start = source.index(marker)
            end = source.find("\n- id: ", start + len(marker))
            web = source[start:] if end < 0 else source[start:end]
            self.assertIn("fetch: true", web, preset_id)

        minimal = self.preset("minimal")
        self.assertNotIn("- id: tool-web", minimal)


    def test_release_workflow_names_every_variant_artifact_uniquely(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("python tools/verify_release_version.py --print-version", workflow)
        self.assertIn('--version "${GITHUB_REF_NAME#v}"', workflow)
        self.assertIn("tools.tests.test_release_product_contract", workflow)

        for variant in VARIANTS:
            self.assertIn(f"--variant {variant}", workflow)
        for platform, arch, _portable_ext, _installer_ext in PLATFORMS:
            self.assertIn(f"platform: {platform}", workflow)
            self.assertIn(f"arch: {arch}", workflow)

        upload = workflow_step(workflow, "上传当前平台产物")
        prefix = "deepseek-harness-rs-v${VERSION}"
        for variant in VARIANTS:
            self.assertIn(
                f'dpkg-deb --root-owner-group --build "$PKG" "dist/{prefix}-linux-x86_64-{variant}.deb"',
                workflow.replace("${variant}", variant),
            )
            self.assertIn(
                f'"dist/{prefix}-macos-${{{{ matrix.arch }}}}-{variant}.pkg"',
                workflow.replace("${variant}", variant),
            )
        installer = (ROOT / "packaging" / "windows" / "deepseek-harness-rs.iss").read_text(
            encoding="utf-8"
        )
        self.assertIn(
            "OutputBaseFilename=deepseek-harness-rs-v{#MyAppVersion}-windows-x86_64-{#Variant}-setup",
            installer,
        )
        for upload_pattern in (
            "dist/${{ steps.release.outputs.prefix }}-*-portable.*",
            "dist/${{ steps.release.outputs.prefix }}-*.exe",
            "dist/${{ steps.release.outputs.prefix }}-*.deb",
            "dist/${{ steps.release.outputs.prefix }}-*.pkg",
        ):
            self.assertIn(upload_pattern, upload)

        hashes = workflow_step(workflow, "生成校验和")
        self.assertLess(workflow.index("生成校验和"), workflow.index("上传当前平台产物"))
        self.assertIn("SHA256SUMS-${{ matrix.platform }}-${{ matrix.arch }}.txt", hashes)
        self.assertIn("glob(f'{prefix}-*')", hashes)
        self.assertIn("actual != expected", hashes)
        self.assertIn("missing={sorted(expected-actual)}", hashes)
        self.assertIn("dist/SHA256SUMS-${{ matrix.platform }}-${{ matrix.arch }}.txt", upload)

        for step_name in (
            "核验 Windows 安装器内容",
            "核验 Linux DEB 内容",
            "核验 macOS PKG 内容",
        ):
            self.assertIn(step_name, workflow)
        self.assertIn("tools/verify_installer_package.py", workflow)
        self.assertIn("issrc/releases/download/is-6_1_2/innosetup-6.1.2.exe", workflow)
        self.assertIn("a3ce1c40ef9c71a92691aaff0f413f530c8c9e3c766be481bc63ca7cc74e35e7", workflow)
        self.assertIn('Get-FileHash -LiteralPath $compilerInstaller -Algorithm SHA256', workflow)
        self.assertIn('& $compiler', workflow)
        self.assertIn("choco install innounp --version 0.50", workflow)

        gate = workflow_step(workflow, "版本与产品门禁")
        self.assertIn("python tools/verify_free_model_catalog.py", gate)
        self.assertIn("mimo-v2.5-free", gate)

    def test_free_model_catalog_verifier_uses_the_live_official_endpoint(self):
        verifier = (ROOT / "tools" / "verify_free_model_catalog.py").read_text(
            encoding="utf-8"
        )
        self.assertIn("https://opencode.ai/zen/v1/models", verifier)
        self.assertIn("mimo-v2.5-free", verifier)
        self.assertIn("urllib.request.urlopen", verifier)

    def test_release_verifier_enforces_physical_variant_and_version_boundaries(self):
        verifier = (ROOT / "tools" / "verify_release_package.py").read_text(
            encoding="utf-8"
        )
        for marker in (
            "core archive unexpectedly carries package defaults",
            "skin payload presence does not match package variant",
            "free archive is missing its package defaults",
            "packaged host version mismatch",
            'manifest["variant"]',
        ):
            self.assertIn(marker, verifier)

    def test_package_script_keeps_launcher_runtime_icon_in_every_variant(self):
        package = (ROOT / "tools" / "package_release.py").read_text(encoding="utf-8")
        verifier = (ROOT / "tools" / "verify_release_package.py").read_text(
            encoding="utf-8"
        )
        self.assertIn('ROOT / "packaging" / "windows" / "deepseek-black.ico"', package)
        self.assertIn('stage / "deepseek-black.ico"', package)
        self.assertIn('prefix + "deepseek-black.ico"', verifier)

    def test_release_stage_requires_every_web_bundle(self):
        stage = (ROOT / "tools" / "stage_release_web.py").read_text(
            encoding="utf-8"
        )
        for required in (
            "web/dist/plugins/ui-settings-models.js",
            "web/dist/plugins/ui-conversation.js",
            "web/dist/plugins/ui-theme.js",
            "web/dist/plugins/ui-trajectory.js",
            "web/dist/plugins/ui-model-selection.js",
        ):
            self.assertIn(required, stage)

        manifest = json.loads(
            (ROOT / "web" / "dist" / "plugins" / "manifest.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertTrue(manifest["rev"])

    def test_release_documentation_describes_runtime_and_variants(self):
        english = (ROOT / "README.md").read_text(encoding="utf-8")
        chinese = (ROOT / "README.zh.md").read_text(encoding="utf-8")
        porting = (ROOT / "PORTING.md").read_text(encoding="utf-8")
        for source in (english, chinese):
            for marker in (
                VERSION,
                "send_message",
                "web_fetch",
                "core",
                "skin",
                "free",
                "mimo-v2.5-free",
                "https://opencode.ai/zen/v1/models",
            ):
                self.assertIn(marker, source)
        for marker in (
            VERSION,
            "SessionSeq",
            "SessionLogOffset",
            "seedLength",
            "agent-message",
            "web_fetch",
        ):
            self.assertIn(marker, porting)


    def test_model_editor_persists_image_input_capability(self):
        source = (ROOT / "web" / "dist" / "plugins" / "ui-settings-models.js").read_text(
            encoding="utf-8"
        )
        for marker in (
            'children: t("modelImageInput")',
            'checked: Array.isArray(model.input) && model.input.includes("image")',
            'patch(index, { input: event.target.checked ? ["text", "image"] : ["text"] });',
            'modelImageInput: "Accepts image input"',
            'modelImageInput: "支持图片输入"',
        ):
            self.assertIn(marker, source)

    def test_model_picker_search_filters_and_bulk_selects_visible_candidates(self):
        source = (ROOT / "web" / "dist" / "plugins" / "ui-settings-models.js").read_text(
            encoding="utf-8"
        )
        markers = {
            "query state": "const [candidateQuery, setCandidateQuery]",
            "query reset after fetch": 'setCandidateQuery("");\n\t\t\t\t\tsetCandidates(found);',
            "query reset on close": 'setPicked(/* @__PURE__ */ new Set());\n\t\t\t\tsetCandidateQuery("");',
            "id and name filter": "candidate.id.toLowerCase().includes(normalizedCandidateQuery) || candidate.name?.toLowerCase().includes(normalizedCandidateQuery)",
            "visible-only bulk selection": "for (const candidate of visibleCandidates)",
            "search input": 'type: "search"',
            "no-match status": 'role: "status"',
            "visible render": "children: visibleCandidates.map",
        }
        missing = [name for name, marker in markers.items() if marker not in source]
        self.assertEqual(missing, [], ", ".join(missing))
        order = (
            "query state",
            "query reset after fetch",
            "query reset on close",
            "id and name filter",
            "search input",
            "no-match status",
            "visible render",
        )
        positions = [source.index(markers[name]) for name in order]
        self.assertEqual(positions, sorted(positions))
        self.assertEqual(source.count(markers["visible render"]), 1)
        self.assertNotIn("children: (candidates ?? []).map", source)

    def test_web_theme_visual_tokens_are_present(self):
        theme = (ROOT / "web" / "dist" / "plugins" / "ui-theme.js").read_text(
            encoding="utf-8"
        )
        for marker in (
            "--dsw-elevation-stroke",
            "--dsw-elevation-panel",
            "--dsw-elevation-prominent",
            "--dsw-elevation-soft",
            'ctx.effect(() => {',
            'tag.remove();',
        ):
            self.assertIn(marker, theme)

    def test_web_chat_keeps_rust_history_and_adds_hot_path_guards(self):
        conversation = (
            ROOT / "web" / "dist" / "plugins" / "ui-conversation.js"
        ).read_text(encoding="utf-8")
        for marker in (
            "async loadThrough(seq)",
            "async loadAround(seq)",
            "async loadNewer()",
            'useProjection("turnOutline")',
            "hasMoreBefore",
            "hasMoreAfter",
        ):
            self.assertIn(marker, conversation)
        for marker in (
            "const SCROLL_SAMPLE_INTERVAL_MS = 500",
            "scrollSamplePendingRef",
            'addEventListener("scrollend"',
            "const ChatNodeList = (0, react.memo)",
            '(0, react_jsx_runtime.jsx)(ChatNodeList, {',
            "function turnNavigatorWindow",
            "items.slice(windowRange.start, windowRange.end)",
            "contain:size layout",
            "summaryText",
            "data-expanded",
            "--dsw-elevation-panel",
        ):
            self.assertIn(marker, conversation)

    def test_chat_turn_jump_uses_one_targeted_history_page(self):
        conversation = (
            ROOT / "web" / "dist" / "plugins" / "ui-conversation.js"
        ).read_text(encoding="utf-8")
        self.assertIn("Promise.resolve(loadAround(item.anchor.seq))", conversation)
        self.assertIn("loadAround: (seq) => scoped.loadAround(seq)", conversation)
        self.assertNotIn("loadThrough(item.anchor.seq)", conversation)

    def test_continuable_prompt_and_preflight_use_runtime_capabilities(self):
        source = (
            ROOT / "crates" / "subagent" / "subagent" / "src" / "continuation.rs"
        ).read_text(encoding="utf-8")
        source += (
            ROOT / "crates" / "subagent" / "subagent" / "src" / "index.rs"
        ).read_text(encoding="utf-8")
        for marker in (
            "impl Drop for SubagentFollowupAdmission",
            "has_adjacent_send_message_tool",
            "Arc::ptr_eq(&registered, &visible)",
            "rollback_on_drop",
            "self.locks.acquire(child_id).await",
            "dispose_serialized",
            "admission_gate",
            "let mut activation = activation.lock()",
            "drain/disposal",
        ):
            self.assertIn(marker, source)

    def test_trajectory_pages_resident_history_before_rendering(self):
        trajectory = (
            ROOT / "web" / "dist" / "plugins" / "ui-trajectory.js"
        ).read_text(encoding="utf-8")
        for marker in (
            "const HISTORY_PAGE_NODES = 50",
            "historyNodeLimit",
            "hasResidentOlderHistory",
            "setHistoryNodeLimit",
            "completeInspection.eventNodes.slice(historyStartIndex)",
            "historyTailSeq",
            "findLastIndex((node) => node.seq <= fixedTailSeq)",
            "sameRunningAssistantRequest",
            "patchStreamingAssistant",
            "--dsw-elevation-panel",
        ):
            self.assertIn(marker, trajectory)




if __name__ == "__main__":
    unittest.main()
