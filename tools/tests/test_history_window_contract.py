import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
RUNTIME = ROOT / "web" / "dist" / "plugins" / "client-runtime.js"
CONVERSATION = ROOT / "web" / "dist" / "plugins" / "ui-conversation.js"
CONNECTION = ROOT / "web" / "dist" / "plugins" / "connection.js"


class HistoryWindowContractTests(unittest.TestCase):
    def test_runtime_exposes_bidirectional_bounded_history_window(self):
        source = RUNTIME.read_text(encoding="utf-8")
        for required in (
            "hasMoreBefore",
            "hasMoreAfter",
            "loadingNewer",
            "async loadNewer()",
            "DEFAULT_HISTORY_POLICY",
            "normalizeHistoryPolicy",
            "Never cut a raw event range",
            "function eventEndSeq(event)",
        ):
            self.assertTrue(required in source, required)

    def test_connection_preserves_directional_history_fields(self):
        source = CONNECTION.read_text(encoding="utf-8")
        for required in (
            "hasMoreBefore: boolean()",
            "hasMoreAfter: boolean()",
            "firstSeq: number().int().nullable()",
            "lastSeq: number().int().nullable()",
        ):
            self.assertTrue(required in source, required)

    def test_glm_flash_model_limit_is_declared_in_model_settings(self):
        source = (ROOT / "web" / "dist" / "plugins" / "ui-settings-models.js").read_text(encoding="utf-8")
        self.assertIn('"glm-5.3-flash": 131072', source)
        self.assertIn("knownModelMaxTokens", source)

    def test_archived_sessions_do_not_offer_restore_and_open_view_action(self):
        source = (ROOT / "web" / "dist" / "plugins" / "ui-workspace.js").read_text(encoding="utf-8")
        self.assertNotIn('t("archive.view")', source)
        self.assertNotIn('restore(sessionId, true)', source)
        self.assertNotIn('"archive.view":', source)
        self.assertNotIn("查看、恢复", source)

    def test_settings_describe_is_singleflight_and_write_invalidated(self):
        source = CONNECTION.read_text(encoding="utf-8")
        for required in (
            "describeSettings(payload, signal)",
            "settingsDescriptionInFlight",
            "settingsDescriptionEpoch",
            "SETTINGS_DESCRIPTION_TTL_MS",
            "invalidateSettingsDescription()",
        ):
            self.assertTrue(required in source, required)

    def test_conversation_scroll_loads_forward_page_near_bottom(self):
        source = CONVERSATION.read_text(encoding="utf-8")
        self.assertIn("const hasMoreAfter = useSession((s) => s.hasMoreAfter)", source)
        self.assertIn("const loadingNewer = useSession((s) => s.loadingNewer)", source)
        self.assertIn("isAtBottom && hasMoreAfter && !loadingNewer", source)
        self.assertIn("Promise.resolve(loadNewer())", source)
        self.assertIn("newerRequestRef.current", source)
        self.assertIn("[hasMoreAfter, loadingNewer, loadNewer]", source)
        self.assertIn("floor - el.scrollTop <= 25", source)
        self.assertIn("el.scrollTop <= 80", source)
        self.assertIn("Promise.resolve(loadOlder())", source)

    def test_loading_older_history_retains_the_newly_loaded_head(self):
        source = RUNTIME.read_text(encoding="utf-8")
        self.assertIn('this.trimHistoryWindow("tail")', source)

    def test_context_jump_expands_the_contiguous_window_without_replacing_it(self):
        source = (ROOT / "release" / "plugins" / "dsh-context-jump" / "lib" / "client.js").read_text(encoding="utf-8")
        self.assertNotIn("loadAround(entry.seq)", source)
        self.assertIn("ensureTarget", source)
        self.assertIn("await session.loadOlder()", source)
        self.assertIn("await session.loadNewer()", source)
        self.assertIn("if (before === after) return false", source)

    def test_goal_actions_always_release_pending_after_transport_failure(self):
        source = (ROOT / "web" / "dist" / "plugins" / "ui-goal.js").read_text(encoding="utf-8")
        self.assertIn("try {", source)
        self.assertIn("finally {", source)
        self.assertIn("pendingRef.current = false", source)
        self.assertIn("setPending(false)", source)
        self.assertIn("actionError", source)
        host = (ROOT / "crates" / "host" / "apiproxy" / "src" / "proxy.rs").read_text(encoding="utf-8")
        self.assertIn("GoalVerb::Pause", host)
        self.assertIn("keep_inbox: true", host)
        manifest = (ROOT / "web" / "dist" / "plugins" / "manifest.json").read_text(encoding="utf-8")
        self.assertIn("@deepseek-ai/dsh-client-ui-goal", manifest)
        self.assertIn('goal.phase === "blocked"', source)

    def test_theme_settings_include_popular_palettes_and_bing_wallpaper(self):
        theme = (ROOT / "web" / "dist" / "plugins" / "ui-theme.js").read_text(encoding="utf-8")
        host = (ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs").read_text(encoding="utf-8")
        for theme_id in ("catppuccin", "dracula", "nord", "tokyo-night", "linear", "notion"):
            self.assertIn(f'"{theme_id}"', theme)
            self.assertIn(f'"{theme_id}"', host)
        self.assertIn("/__dsh-bing-wallpaper", theme)
        self.assertIn("/__dsh-bing-wallpaper", host)
        self.assertIn('"ui-wallpaper"', host)
        self.assertIn('"ui-history"', host)
        self.assertIn("HistoryMemorySettings", theme)
        self.assertIn("NO_SKIN", theme)
        self.assertIn("BasicAppearanceSettings", theme)
        self.assertIn('NO_SKIN ? "外观" : "皮肤与壁纸"', theme)
        for required in ("SKIN_CATALOG", "全部皮肤", "浅色皮肤", "深色皮肤", "随机皮肤", "搜索皮肤", "主题详情"):
            self.assertIn(required, theme)

    def test_code_graph_and_sidebar_editor_contract(self):
        graph = (ROOT / "web" / "dist" / "plugins" / "ui-code-graph.js").read_text(encoding="utf-8")
        manifest = (ROOT / "web" / "dist" / "plugins" / "manifest.json").read_text(encoding="utf-8")
        sidebar = (ROOT / "release" / "plugins" / "dsh-better-sidebar" / "lib" / "client.js").read_text(encoding="utf-8")
        for required in ("代码图谱", "符号列表", "查找引用", "调用者", "被调用者", "调用链", "文件依赖", "影响面"):
            self.assertIn(required, graph)
        for required in ("buildGraph", "dependencyNames", "wordLines", "references", "calls", "deps"):
            self.assertIn(required, graph)
        self.assertIn("await buildGraph(files)", graph)
        self.assertIn("fileIndex%8===7", graph)
        self.assertIn("defs.length===1", graph)
        self.assertIn("MAX_GRAPH_FILE_CHARS", graph)
        self.assertIn("MAX_GRAPH_TOTAL_CHARS", graph)
        self.assertIn("text.charCodeAt(index)===10", graph)
        self.assertNotIn('defs.find(def=>def.path===file.path)', graph)
        self.assertIn("ui-code-graph.js", manifest)
        self.assertIn("overflow:auto", sidebar)
        self.assertIn("numberedCode", sidebar)
        self.assertIn("dbs-code-line", sidebar)

    def test_environment_settings_expose_runtime_storage_and_workspaces(self):
        source = (ROOT / "web" / "dist" / "plugins" / "ui-workbench.js").read_text(encoding="utf-8")
        for required in ("运行概览", "存储目录", "工作区", "host.describe", "workspace.list", "正式数据根"):
            self.assertIn(required, source)

    def test_manual_windows_release_uses_the_same_fallback_version_as_packaging(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
        self.assertIn("$refName.StartsWith('v')", workflow)
        self.assertIn("else { '0.1.0-rc.8' }", workflow)
        self.assertNotIn("TrimStart('v')", workflow)


if __name__ == "__main__":
    unittest.main()
