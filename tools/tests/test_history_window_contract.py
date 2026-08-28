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
            "HISTORY_PAGE_MESSAGES",
            "HISTORY_WINDOW_PAGES",
            "HISTORY_WINDOW_EVENTS",
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

    def test_workspace_hover_uses_human_windows_path(self):
        source = (ROOT / "web" / "dist" / "plugins" / "ui-workspace.js").read_text(encoding="utf-8")
        self.assertIn("function humanWorkspacePath(value)", source)
        self.assertIn('replace(/^\\\\\\\\\\?\\\\UNC\\\\/i, "\\\\\\\\")', source)
        self.assertIn('replace(/^\\\\\\\\\\?\\\\/, "")', source)
        self.assertIn("cwd: humanWorkspacePath(g.cwd)", source)
        self.assertIn("copyText: row.cwd", source)

    def test_stop_control_is_non_submitting_and_reports_cancel_failures(self):
        conversation = CONVERSATION.read_text(encoding="utf-8")
        runtime = RUNTIME.read_text(encoding="utf-8")
        self.assertIn('type: "button"', conversation[conversation.index("interruptible &&"):conversation.index("primaryLabel", conversation.index("interruptible &&"))])
        self.assertIn("let stopPending = false", conversation)
        self.assertIn("await scopedConversation(sessions, sessionId).cancel()", conversation)
        self.assertIn("stopPending = false", conversation)
        self.assertNotIn("scopedConversation(sessions, sessionId).cancel().catch(() => {})", conversation)
        self.assertIn('op: "stop"', runtime)

    def test_approval_protocol_is_fail_closed_with_three_decisions(self):
        connection = CONNECTION.read_text(encoding="utf-8")
        conversation = CONVERSATION.read_text(encoding="utf-8")
        host = (ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs").read_text(encoding="utf-8")
        approval = (ROOT / "crates" / "interaction" / "user-approval" / "src" / "lib.rs").read_text(encoding="utf-8")
        invariant = (ROOT / "crates" / "interaction" / "user-approval" / "src" / "invariant.rs").read_text(encoding="utf-8")
        self.assertIn('literal("allowed-always")', connection)
        self.assertIn('answer("allowed-always")', conversation)
        self.assertIn('"approval.allowAlways": "始终允许"', conversation)
        self.assertIn("grantKey: string().optional()", connection)
        self.assertIn("rememberable: boolean()", connection)
        self.assertIn("answered || approval.data.rememberable === false", conversation)
        self.assertNotIn("unansweredApproval", host)
        self.assertNotIn("UnansweredApprovalPolicy", approval)
        self.assertIn("Ok(ApprovalOutcome::Unavailable) | Err(_) => unattended", approval)
        self.assertIn('"allowed-always"', invariant)

    def test_security_shield_exposes_complete_persisted_controls(self):
        host = (ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs").read_text(encoding="utf-8")
        ui = (ROOT / "web" / "dist" / "plugins" / "ui-settings-general.js").read_text(encoding="utf-8")
        for field in (
            "unattendedPolicy",
            "riskToolPolicy",
            "outsideWritePolicy",
            "sensitiveReadPolicy",
            "credentialShellPolicy",
        ):
            self.assertIn(f'"{field}"', host)
            self.assertIn(field, ui)
        self.assertIn('api.settings.mutate({ ns: "security"', ui)
        self.assertIn('expectedRevision: state.namespace.revision', ui)
        self.assertIn('["allow-safe-only", "仅允许安全操作"]', ui)
        self.assertIn('["ask-every-time", "每次询问"]', ui)
        self.assertIn("硬阻断始终生效", ui)

    def test_security_settings_does_not_fan_out_session_history(self):
        ui = (ROOT / "web" / "dist" / "plugins" / "ui-settings-general.js").read_text(encoding="utf-8")
        section = ui[ui.index("function SecuritySection"):ui.index("//#endregion", ui.index("function SecuritySection"))]
        self.assertNotIn("api.sessions.list", section)
        self.assertNotIn("api.sessions.history", section)

    def test_security_settings_is_grouped_and_excludes_unrelated_code_graph_status(self):
        ui = (ROOT / "web" / "dist" / "plugins" / "ui-settings-general.js").read_text(encoding="utf-8")
        section = ui[ui.index("function SecuritySection"):ui.index("//#endregion", ui.index("function SecuritySection"))]
        self.assertIn("dshSecurityGroup", ui)
        self.assertIn("审批行为", section)
        self.assertIn("工具与路径", section)
        self.assertNotIn("图谱权限", section)
        self.assertNotIn("__DSH_CODE_GRAPH_STATUS__", section)

    def test_approval_is_rendered_in_the_conversation_flow_not_only_as_composer_takeover(self):
        conversation = CONVERSATION.read_text(encoding="utf-8")
        self.assertIn('name: "conversation.input.dock"', conversation)
        self.assertIn('id: "approval"', conversation)
        self.assertIn("ApprovalDock", conversation)
        self.assertNotIn('name: "conversation.composer",\n\t\t\t\tselect: selectApproval', conversation)

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
        self.assertIn("const historyBrowsing = useSession((s) => s.historyBrowsing)", source)
        self.assertIn("const loadingNewer = useSession((s) => s.loadingNewer)", source)
        self.assertIn("readerForwardIntentRef", source)
        self.assertIn("!historyBrowsing || readerForwardIntentRef.current", source)
        self.assertIn("if (historyBrowsing || !hasMoreAfter", source)
        self.assertIn("!historyBrowsing && (appendedUser", source)
        self.assertIn("Promise.resolve(loadNewer())", source)
        self.assertIn("newerRequestRef.current", source)
        self.assertIn("[historyBrowsing, hasMoreAfter, loadingNewer, loadNewer]", source)
        self.assertIn("floor - el.scrollTop <= 25", source)
        self.assertIn("el.scrollTop <= 80", source)
        self.assertIn("Promise.resolve(loadOlder())", source)
        self.assertNotIn("loadOlderAnchored", source)
        self.assertNotIn('t("chat.loadOlder")', source)

    def test_runtime_exposes_history_browse_state_to_prevent_jump_scroll_feedback(self):
        source = RUNTIME.read_text(encoding="utf-8")
        self.assertIn("historyBrowsing: this.historyTargetSeq !== null", source)

    def test_loading_older_history_retains_the_newly_loaded_head(self):
        source = RUNTIME.read_text(encoding="utf-8")
        self.assertIn('this.trimHistoryWindow("tail")', source)
        self.assertIn("eventStartSeq(older[0].event)", source)

    def test_live_history_trims_on_page_limit_before_event_limit(self):
        source = RUNTIME.read_text(encoding="utf-8")
        self.assertIn(
            "if (trim && (this.historyPages.length > HISTORY_WINDOW_PAGES || this.events.length > HISTORY_WINDOW_EVENTS)) this.trimHistoryWindow(\"head\")",
            source,
        )
        self.assertIn("if (this.events.length > HISTORY_WINDOW_EVENTS && this.historyPages.length === 1)", source)
        self.assertIn("page.firstSeq = eventStartSeq(this.events[0])", source)
        self.assertIn("page.eventCount = this.events.length", source)

    def test_context_jump_uses_constant_time_targeted_history_lookup(self):
        source = (ROOT / "release" / "plugins" / "dsh-context-jump" / "lib" / "client.js").read_text(encoding="utf-8")
        manifest = (ROOT / "release" / "plugins" / "dsh-context-jump" / "package.json").read_text(encoding="utf-8")
        self.assertIn("ensureTarget", source)
        self.assertIn("await session.loadAround(seq, true)", source)
        self.assertIn("snapshot.chat.nodes.get(preferredKey)", source)
        self.assertNotIn("snapshot.chat.nodes.has", source)
        self.assertIn("frame < 12", source)
        self.assertIn("const currentSession = () => ctx.sessions.binding(sessionId)?.session", source)
        self.assertIn("const currentFace = () => currentSession()?.projections.faceOf", source)
        self.assertIn("const railFace =", source)
        self.assertIn("currentSession()?.subscribe", source)
        self.assertIn('ctx.slots.inject("conversation.input.overlay"', source)
        self.assertIn('id: "user-message-rail"', source)
        self.assertNotIn("Temporarily leave the overlay seat empty", source)
        self.assertIn("node?.anchorSeq === seq && node.kind === \"user\"", source)
        self.assertNotIn("13:input-message", source)
        self.assertIn("targetError", source)
        self.assertNotIn("attempt < 200", source)
        self.assertIn('"@deepseek-ai/dsh-client-ui-conversation"', manifest)

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

    def test_runtime_session_disposal_is_not_reported_as_durable_session_removal(self):
        host = (ROOT / "crates" / "host" / "apiproxy" / "src" / "proxy.rs").read_text(encoding="utf-8")
        disposed = host[host.index('// session/disposed'):host.index('// workspace/session-deleted')]
        self.assertIn("HostFrame::SessionStatus", disposed)
        self.assertIn("running: false", disposed)
        self.assertNotIn("HostFrame::SessionRemoved", disposed)

    def test_theme_settings_include_popular_palettes_and_bing_wallpaper(self):
        theme = (ROOT / "web" / "dist" / "plugins" / "ui-theme.js").read_text(encoding="utf-8")
        host = (ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs").read_text(encoding="utf-8")
        for theme_id in ("catppuccin", "dracula", "nord", "tokyo-night", "linear", "notion"):
            self.assertIn(f'"{theme_id}"', theme)
            self.assertIn(f'"{theme_id}"', host)
        self.assertIn("/__dsh-bing-wallpaper", theme)
        self.assertIn("/__dsh-bing-wallpaper", host)
        self.assertIn('"ui-wallpaper"', host)
        self.assertNotIn('"ui-history"', host)
        self.assertNotIn("HistoryMemorySettings", theme)
        self.assertNotIn('id: "history-memory"', theme)
        self.assertIn("NO_SKIN", theme)
        self.assertIn("BasicAppearanceSettings", theme)
        self.assertIn('NO_SKIN ? "外观" : "皮肤与壁纸"', theme)
        for required in ("SKIN_CATALOG", "全部皮肤", "浅色皮肤", "深色皮肤", "随机皮肤", "搜索皮肤", "主题详情"):
            self.assertIn(required, theme)

    def test_history_loading_uses_the_bounded_automatic_window(self):
        runtime = RUNTIME.read_text(encoding="utf-8")
        self.assertIn("HISTORY_PAGE_MESSAGES = 12", runtime)
        self.assertIn("HISTORY_WINDOW_PAGES = 5", runtime)
        self.assertIn("HISTORY_WINDOW_EVENTS = 4096", runtime)
        self.assertNotIn("FULL_HISTORY_ON_OPEN", runtime)
        self.assertNotIn("DEFAULT_HISTORY_POLICY", runtime)
        self.assertNotIn("normalizeHistoryPolicy", runtime)
        self.assertNotIn("loadHistoryPolicy", runtime)
        self.assertNotIn('historyPolicy.mode === "full"', runtime)

    def test_code_graph_and_sidebar_editor_contract(self):
        graph = (ROOT / "web" / "dist" / "plugins" / "ui-code-graph.js").read_text(encoding="utf-8")
        manifest = (ROOT / "web" / "dist" / "plugins" / "manifest.json").read_text(encoding="utf-8")
        sidebar = (ROOT / "release" / "plugins" / "dsh-better-sidebar" / "lib" / "client.js").read_text(encoding="utf-8")
        for required in ("代码图谱", "符号列表", "查找引用", "调用者", "被调用者", "调用链", "文件依赖", "影响面"):
            self.assertIn(required, graph)
        for required in ("/__dsh-preview/code-graph", "references", "calls", "deps"):
            self.assertIn(required, graph)
        self.assertNotIn("extractSymbols", graph)
        self.assertNotIn("wordLines", graph)
        host_graph = (ROOT / "crates" / "code-graph" / "code-graph" / "src" / "lib.rs").read_text(encoding="utf-8")
        self.assertIn('engine: "rust-tree-sitter"', host_graph)
        self.assertIn("GraphSnapshot::from_graph", host_graph)
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
