import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
RUNTIME = ROOT / "web" / "dist" / "plugins" / "client-runtime.js"
CONVERSATION = ROOT / "web" / "dist" / "plugins" / "ui-conversation.js"
CONNECTION = ROOT / "web" / "dist" / "plugins" / "connection.js"


class ProductSurfaceContractTests(unittest.TestCase):
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

    def test_prompt_acceptance_cannot_be_overwritten_by_a_late_idle_frame(self):
        runtime = RUNTIME.read_text(encoding="utf-8")
        prompt = runtime[runtime.index("async prompt(content, mode)"):runtime.index("async readAttachment", runtime.index("async prompt(content, mode)"))]
        self.assertIn("const runningRevisionAtStart = this.runningRevision", prompt)
        self.assertIn("if (result.value.accepted && this.runningRevision === runningRevisionAtStart) this.handleRunning(true)", prompt)
        self.assertNotIn("this.running = true", prompt)

    def test_prompt_marks_send_attempt_before_returning_from_history_browse(self):
        runtime = RUNTIME.read_text(encoding="utf-8")
        prompt = runtime[runtime.index("async prompt(content, mode)"):runtime.index("async readAttachment", runtime.index("async prompt(content, mode)"))]
        attempted = prompt.index("this.promptAttempted = true")
        return_latest = prompt.index("await this.returnLatest()")
        self.assertLess(attempted, return_latest)

    def test_prompt_rebases_running_revision_after_returning_from_history_browse(self):
        runtime = RUNTIME.read_text(encoding="utf-8")
        prompt = runtime[runtime.index("async prompt(content, mode)"):runtime.index("async readAttachment", runtime.index("async prompt(content, mode)"))]
        return_latest = prompt.index("await this.returnLatest()")
        revision = prompt.index("const runningRevisionAtStart = this.runningRevision")
        self.assertLess(return_latest, revision)

    def test_ordinary_send_commits_only_after_host_acceptance(self):
        conversation = CONVERSATION.read_text(encoding="utf-8")
        start = conversation.index("\n\t\t\tasync sink(session, text, imageIds, mode) {")
        end = conversation.index("\n\t\t\tasync steerQueue(session, shell)", start)
        sink = conversation[start:end]
        self.assertIn("await this.conversation().sendSession(session, text, imageIds, mode)", sink)
        accepted = sink.index("await this.conversation().sendSession(session, text, imageIds, mode)")
        committed = sink.index("shell?.commitAcceptedSend(text, imageIds)")
        self.assertLess(accepted, committed)
        self.assertNotIn(".catch(() =>", sink)
        self.assertIn('shell?.notify("error", message)', sink)

    def test_late_send_acceptance_does_not_clear_newer_local_input(self):
        conversation = CONVERSATION.read_text(encoding="utf-8")
        start = conversation.index("commitAcceptedSend(draft, imageIds)")
        end = conversation.index("commitSend(imageIds)", start)
        commit = conversation[start:end]
        self.assertIn("if (this.snapshot.draft !== draft) return false", commit)
        self.assertIn("this.imageIds.some((id, index) => id !== imageIds[index])", commit)

    def test_failed_ordinary_send_keeps_the_original_draft_without_late_restore(self):
        conversation = CONVERSATION.read_text(encoding="utf-8")
        start = conversation.index("\n\t\t\tasync sink(session, text, imageIds, mode) {")
        end = conversation.index("\n\t\t\tasync steerQueue(session, shell)", start)
        sink = conversation[start:end]
        self.assertNotIn("restoreImages(imageIds)", sink)
        self.assertNotIn("shell.setDraft(text)", sink)

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

    def test_approval_dock_reuses_the_existing_flow_without_nested_snapshot_subscription(self):
        conversation = CONVERSATION.read_text(encoding="utf-8")
        start = conversation.index("function ApprovalDock")
        end = conversation.index("function ApprovalFlow", start)
        dock = conversation[start:end]
        self.assertIn("matched", dock)
        self.assertNotIn("useSession(", dock)

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

    def test_theme_settings_include_exact_skin_catalog_and_bing_wallpaper(self):
        theme = (ROOT / "web" / "dist" / "plugins" / "ui-theme.js").read_text(encoding="utf-8")
        host = (ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs").read_text(encoding="utf-8")
        for theme_id in (
            "light",
            "dark",
            "whale-song",
            "blue-fantasy",
            "harbor",
            "xp",
            "dragon-heir",
            "minecraft",
            "trading",
            "miku",
            "deepseek-official",
        ):
            self.assertIn(f'"{theme_id}"', theme)
            self.assertIn(f'"{theme_id}"', host)
        preferences = theme.split("const THEME_PREFERENCES = [", 1)[1].split("];", 1)[0]
        for retired in ("system", "catppuccin", "dracula", "nord", "tokyo-night", "linear", "notion"):
            self.assertNotIn(f'"{retired}"', preferences)
        self.assertIn("/__dsh-bing-wallpaper", theme)
        self.assertIn("/__dsh-bing-wallpaper", host)
        self.assertIn('"ui-wallpaper"', host)
        self.assertNotIn('"ui-history"', host)
        self.assertNotIn("HistoryMemorySettings", theme)
        self.assertNotIn('id: "history-memory"', theme)
        self.assertIn("NO_SKIN", theme)
        self.assertIn("BasicAppearanceSettings", theme)
        self.assertIn('NO_SKIN ? "外观" : "皮肤与壁纸"', theme)
        for required in ("SKIN_CATALOG", "dshSkinPicker", "当前皮肤", "data-dsh-skin-option", "选择后立即应用并持久化"):
            self.assertIn(required, theme)
        skin_settings = theme[theme.index("function SkinSettings"):theme.index("function BasicAppearanceSettings")]
        for retired_ui in ("全部皮肤", "浅色皮肤", "深色皮肤", "随机皮肤", "搜索皮肤", "主题详情"):
            self.assertNotIn(retired_ui, skin_settings)

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

    def test_release_workflow_gates_and_verifies_core_skin_executables(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
        verifier = (ROOT / "tools" / "verify_release_package.py").read_text(encoding="utf-8")
        package = (ROOT / "tools" / "package_release.py").read_text(encoding="utf-8")
        launcher = (ROOT / "crates" / "host" / "dsh-launcher" / "src" / "main.rs").read_text(encoding="utf-8")
        launcher_cargo = (ROOT / "crates" / "host" / "dsh-launcher" / "Cargo.toml").read_text(encoding="utf-8")
        skin_installer = (ROOT / "crates" / "host" / "dsh-skin-installer" / "src" / "main.rs").read_text(encoding="utf-8")
        self.assertIn("tools.tests.test_v012_surface_contract", workflow)
        self.assertIn("python tools/verify_release_package.py", workflow)
        self.assertIn("--variant core", workflow)
        self.assertIn("--variant skin", workflow)
        self.assertNotIn("--variant no-skin", workflow)
        self.assertIn("dsh-launcher", workflow)
        self.assertIn("dsh-skin-installer", workflow)
        self.assertIn("dsh-launcher", launcher_cargo)
        self.assertIn("zsui", launcher_cargo)
        self.assertIn("winresource", launcher_cargo)
        self.assertIn("window(", launcher)
        self.assertTrue((ROOT / "crates" / "host" / "dsh-launcher" / "build.rs").is_file())
        self.assertIn("Command::new", launcher)
        self.assertIn("GetUserDefaultUILanguage", launcher)
        self.assertIn("DeepSeek Harness-rs Launcher", launcher)
        self.assertIn("DeepSeek Harness-rs 启动器", launcher)
        self.assertIn("install_skins: \"安装皮肤\"", launcher)
        self.assertIn("InstallSkins", launcher)
        self.assertIn(".size(600, 390)", launcher)
        self.assertNotIn("DshServiceManager.ps1", package)
        self.assertNotIn("启动DeepSeek Harness-rs.cmd", package)
        self.assertNotIn("deepseek-harness-rs-web", package)
        self.assertIn("/DIconFile=$env:GITHUB_WORKSPACE", workflow)
        installer = (ROOT / "packaging" / "windows" / "deepseek-harness-rs.iss").read_text(encoding="utf-8")
        self.assertIn("SetupIconFile={#IconFile}", installer)
        self.assertIn('Name: "english"; MessagesFile: "compiler:Default.isl"', installer)
        self.assertIn('Name: "chinesesimp"; MessagesFile: "{#ChineseMessages}"', installer)
        self.assertIn("ShowLanguageDialog=auto", installer)
        self.assertIn("LanguageDetectionMethod=uilanguage", installer)
        self.assertIn('#if Variant == "core"', installer)
        self.assertIn('DefaultDirName={localappdata}\\Programs\\DeepSeek Harness-rs\\{#Variant}', installer)
        self.assertIn('Languages: chinesesimp', installer)
        self.assertIn('Languages: english', installer)
        self.assertIn("/DChineseMessages=$language", workflow)
        self.assertTrue((ROOT / "packaging" / "windows" / "ChineseSimplified.isl").is_file())
        self.assertIn("skin executable leaks bundled skin assets", verifier)
        self.assertIn("core archive leaks bundled skin assets", verifier)
        self.assertIn("skin_payload", package)
        self.assertIn("deepseek-harness-rs-skin", package)
        skin_cargo = (ROOT / "crates" / "host" / "dsh-skin-installer" / "Cargo.toml").read_text(encoding="utf-8")
        skin_main = (ROOT / "crates" / "host" / "dsh-skin-installer" / "src" / "main.rs").read_text(encoding="utf-8")
        self.assertIn("dsh-skin-installer", skin_cargo)
        self.assertIn("PAYLOAD_MARKER", skin_main)
        self.assertIn("zip::ZipArchive", skin_main)
        self.assertIn("skin_source = ROOT / \"target\" / \"release\"", package)
        self.assertIn("build_skin_payload(skin_source", package)
        self.assertIn("sys.path.insert", package)
        self.assertIn("PE executable", verifier)
        self.assertIn("ZipArchive", skin_installer)
        self.assertIn("PAYLOAD_MARKER", skin_installer)
        host = (ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs").read_text(encoding="utf-8")
        self.assertIn('!packaged_resource("web/dist/skins").is_dir()', host)


if __name__ == "__main__":
    unittest.main()
