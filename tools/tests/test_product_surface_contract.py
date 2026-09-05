import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
RUNTIME = ROOT / "web" / "dist" / "plugins" / "client-runtime.js"
CONVERSATION = ROOT / "web" / "dist" / "plugins" / "ui-conversation.js"
CONNECTION = ROOT / "web" / "dist" / "plugins" / "connection.js"


class ProductSurfaceContractTests(unittest.TestCase):
    def test_pinned_inno_compiler_receives_utf8_bom_and_correct_chinese_text(self):
        for relative in ("packaging/windows/deepseek-harness-rs.iss", "packaging/windows/ChineseSimplified.isl"):
            raw = (ROOT / relative).read_bytes()
            self.assertTrue(raw.startswith(b"\xef\xbb\xbf"), f"Inno 6.1.2 requires a UTF-8 BOM: {relative}")
            text = raw.decode("utf-8-sig", errors="strict")
            self.assertNotIn("\ufffd", text)
        language = (ROOT / "packaging/windows/ChineseSimplified.isl").read_text(encoding="utf-8-sig")
        self.assertIn("LanguageName=简体中文", language)
        self.assertIn("SetupAppTitle=安装", language)
        installer = (ROOT / "packaging/windows/deepseek-harness-rs.iss").read_text(encoding="utf-8-sig")
        for required in ("dsh-launcher.exe", "deepseek-harness-rs.exe", "PACKAGE.json"):
            self.assertIn(f'#error The installer payload is missing {required}', installer)
        self.assertIn('Check: InstalledRuntimeReady', installer)

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

    def test_reasoning_rows_default_open_and_remain_manually_collapsible(self):
        conversation = CONVERSATION.read_text(encoding="utf-8")
        start = conversation.index("function ReasoningRow({ text, running, t })")
        end = conversation.index("//#endregion", start)
        reasoning_row = conversation[start:end]
        self.assertIn("react.useState)(true)", reasoning_row)
        self.assertIn("open: expanded", reasoning_row)
        self.assertIn("expandable: true", reasoning_row)
        self.assertIn("setExpanded((value) => !value)", reasoning_row)
        self.assertNotIn("react.useState)(running)", reasoning_row)

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

    def test_subagent_and_queue_image_delivery_surface_is_enabled(self):
        runtime = RUNTIME.read_text(encoding="utf-8")
        conversation = CONVERSATION.read_text(encoding="utf-8")
        subagents = (ROOT / "crates" / "host" / "apiproxy" / "src" / "api" / "subagents.rs").read_text(encoding="utf-8")
        proxy = (ROOT / "crates" / "host" / "apiproxy" / "src" / "proxy.rs").read_text(encoding="utf-8")
        continuation = (ROOT / "crates" / "subagent" / "subagent" / "src" / "continuation.rs").read_text(encoding="utf-8")
        self.assertNotIn("SUBAGENT_IMAGE_UNSUPPORTED", runtime)
        self.assertIn("content,", runtime)
        self.assertIn("filter((block) => block.type !== \"image\")", runtime)
        self.assertIn("PromptContentPart", subagents)
        self.assertIn("subagent image input requires the attachments service", proxy)
        subagent_prompt = proxy[
            proxy.index("async fn subagent_prompt(") : proxy.index("async fn subagent_interrupt(")
        ]
        self.assertLess(
            subagent_prompt.index("max_encoded_bytes"),
            subagent_prompt.index("STANDARD.decode(data)"),
        )
        self.assertIn("max_images_per_message", subagent_prompt)
        self.assertIn("max_message_image_bytes", subagent_prompt)
        self.assertIn(".admit_followup(parent", subagent_prompt)
        self.assertIn("store.save_images(&pending_images)", subagent_prompt)
        self.assertLess(
            subagent_prompt.index("STANDARD.decode(data)"),
            subagent_prompt.index("store.save_images(&pending_images)"),
        )
        self.assertLess(
            subagent_prompt.index(".admit_followup(parent"),
            subagent_prompt.index("store.save_images(&pending_images)"),
        )
        self.assertIn(".followup(parent,", subagent_prompt)
        text_only_branch = subagent_prompt[
            subagent_prompt.index("if pending_images.is_empty()") : subagent_prompt.index(
                "let admission = match runtime", subagent_prompt.index("if pending_images.is_empty()")
            )
        ]
        self.assertIn('error.code == "CANCELLED"', text_only_branch)
        self.assertIn("RpcError::Cancelled", text_only_branch)
        self.assertIn("runtime.abort_followup(admission).await", subagent_prompt)
        self.assertIn("runtime.submit_followup(admission", subagent_prompt)
        self.assertNotIn("store\n                        .save_image(", subagent_prompt)
        self.assertIn("MODEL_DOES_NOT_SUPPORT_IMAGES", continuation)
        self.assertIn("async fn cold_materialize(", continuation)
        self.assertIn("self.cold_materialize(parent", continuation)
        submit_followup = continuation[
            continuation.index("pub fn submit_followup(") : continuation.index(
                "pub async fn abort_followup", continuation.index("pub fn submit_followup(")
            )
        ]
        self.assertNotIn("prepare_submit(", submit_followup)
        self.assertNotIn("submit_admitted(", submit_followup)
        self.assertIn("self.commit_admitted(", submit_followup)
        self.assertIn("fn commit_admitted(", continuation)
        self.assertIn("let message_id = runtime.submit_followup(admission", subagent_prompt)
        self.assertIn("match store.save_images(&pending_images).await", subagent_prompt)
        self.assertIn("runtime.abort_followup(admission).await", subagent_prompt)
        self.assertIn("QueueImageThumb", conversation)
        self.assertIn("conversation.resolveImage(sessionId, attachment)", conversation)
        self.assertNotIn("ctx.uiConversation.imageUrl", conversation)

    def test_schedule_lifecycle_observes_scoped_agent_creation(self):
        schedule = (
            ROOT / "crates" / "schedule" / "schedule" / "src" / "lib.rs"
        ).read_text(encoding="utf-8")
        listener = schedule[
            schedule.index('ctx.on(\n        "agent/session-start"') : schedule.index(
                "let disposer", schedule.index('ctx.on(\n        "agent/session-start"')
            )
        ]
        self.assertIn("EventOptions::default().global(true)", listener)
        self.assertIn("for root in registry.roots()", schedule)
        self.assertIn("attach_root(root)", schedule)



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
        self.assertIn("answered || !pending.rememberable", conversation)
        self.assertIn("this.wait.payload.rememberable === true", conversation)
        self.assertIn('outcome === "allowed-always" && !this.rememberable', conversation)
        self.assertNotIn("approval.data.rememberable", conversation)
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

    def test_skin_center_preserves_catalog_and_theme_activation(self):
        theme = (ROOT / "web" / "dist" / "plugins" / "ui-theme.js").read_text(encoding="utf-8")
        host = (ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs").read_text(encoding="utf-8")
        skin = (ROOT / "release" / "plugins" / "dsh-skin-center" / "lib" / "client.js").read_text(encoding="utf-8")
        package = (ROOT / "release" / "plugins" / "dsh-skin-center" / "package.json").read_text(encoding="utf-8")
        for theme_id in (
            "light",
            "dark",
            "blue-fantasy",
            "harbor",
            "xp",
            "minecraft",
            "trading",
            "miku",
            "deepseek-official",
        ):
            self.assertIn(f'"{theme_id}"', theme)
            self.assertIn(f'"{theme_id}"', host)
            self.assertIn(f'"{theme_id}"', skin)
        for removed in ("whale-song", "dragon-heir"):
            self.assertNotIn(f'"{removed}"', theme)
            self.assertNotIn(f'"{removed}"', skin)
        preferences = theme.split("const THEME_PREFERENCES = [", 1)[1].split("];", 1)[0]
        for retired in ("system", "catppuccin", "dracula", "nord", "tokyo-night", "linear", "notion"):
            self.assertNotIn(f'"{retired}"', preferences)
        self.assertNotIn("SkinSettings", theme)
        self.assertNotIn("dshSkinPicker", theme)
        self.assertIn('id: "appearance"', theme)
        self.assertIn('ctx.slots.inject("settings.general.item"', theme)
        self.assertIn('id: "skins"', skin)
        self.assertIn('label: "皮肤"', skin)
        self.assertIn("--dsw-alias-", skin)
        self.assertIn("settings.section", skin)
        self.assertIn("data-dsh-skin-center", skin)
        self.assertIn("data-dsh-skin-option", skin)
        self.assertIn('const offThemeChange = theme.ctx.on("theme/change", refresh)', skin)
        self.assertIn("return offThemeChange", skin)
        self.assertNotIn("theme.ctx.off", skin)
        self.assertIn('value: selected.id', skin)
        self.assertIn('disabled: busy', skin)
        self.assertIn('"data-busy": busy || undefined', skin)
        self.assertIn("theme.applyTheme(id)", skin)
        self.assertNotIn("theme.setTheme(id)", skin)
        self.assertIn('disabled: assetsReady === false && !["light", "dark"].includes(skin.id)', skin)
        self.assertIn('data-dsh-skin-card', skin)
        self.assertIn("Skin assets are not installed", skin)
        self.assertIn('"platform": "web"', package)
        self.assertIn("dsh-skin-center", (ROOT / "tools" / "stage_release_plugins.py").read_text(encoding="utf-8"))
        self.assertNotIn("dshSkinPicker{", skin)
        self.assertNotIn('settings_namespace("ui-wallpaper")', host)
        self.assertNotIn('/__dsh-bing-wallpaper', host)
        self.assertNotIn('"ui-history"', host)
        self.assertNotIn("HistoryMemorySettings", theme)
        self.assertNotIn('id: "history-memory"', theme)
        self.assertIn("NO_SKIN", theme)
        self.assertIn("function AppearanceRow", theme)
        self.assertNotIn("BasicAppearanceSettings", theme)
        for retired_ui in ("全部皮肤", "浅色皮肤", "深色皮肤", "随机皮肤", "搜索皮肤", "主题详情"):
            self.assertNotIn(retired_ui, skin)

    def test_code_graph_and_sidebar_editor_contract(self):
        graph = (ROOT / "web" / "dist" / "plugins" / "ui-code-graph.js").read_text(encoding="utf-8")
        manifest = (ROOT / "web" / "dist" / "plugins" / "manifest.json").read_text(encoding="utf-8")
        sidebar = (ROOT / "release" / "plugins" / "dsh-better-sidebar" / "lib" / "client.js").read_text(encoding="utf-8")
        for required in ("代码图谱", "符号列表", "调用引用", "调用者", "被调用者", "调用链", "文件依赖", "影响面"):
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
        for required in ("Rust 核心", "PTC 代码模式运行依赖 · Node.js", "nodeStatusText", "refreshNode=1", "存储目录", "工作区", "host.describe", "workspace.list", "正式数据根"):
            self.assertIn(required, source)
        for required in ("普通会话、原生工具与预构建的 Web 界面由 Rust Host 运行，无需 Node.js", "PTC 代码模式执行 JavaScript / TypeScript 工具编排，必须使用兼容的 Node.js", "标准模式可直接使用 Rust 原生工具"):
            self.assertIn(required, source)

    def test_manual_windows_release_reads_the_workspace_version(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
        self.assertIn("$version = python tools/verify_release_version.py --print-version", workflow)
        self.assertNotIn("TrimStart('v')", workflow)

    def test_release_workflow_gates_and_verifies_core_skin_executables(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(encoding="utf-8")
        verifier = (ROOT / "tools" / "verify_release_package.py").read_text(encoding="utf-8")
        package = (ROOT / "tools" / "package_release.py").read_text(encoding="utf-8")
        launcher = (ROOT / "crates" / "host" / "dsh-launcher" / "src" / "main.rs").read_text(encoding="utf-8")
        launcher_cargo = (ROOT / "crates" / "host" / "dsh-launcher" / "Cargo.toml").read_text(encoding="utf-8")
        skin_installer = (ROOT / "crates" / "host" / "dsh-skin-installer" / "src" / "main.rs").read_text(encoding="utf-8")
        self.assertIn("tools.tests.test_rust_ui_contract", workflow)
        self.assertIn("python tools/verify_release_package.py", workflow)
        self.assertIn("--variant core", workflow)
        self.assertIn("--variant skin", workflow)
        self.assertNotIn("--variant no-skin", workflow)
        self.assertIn("dsh-launcher", workflow)
        self.assertIn("dsh-skin-installer", workflow)
        self.assertIn("dsh-launcher", launcher_cargo)
        self.assertIn("zsui", launcher_cargo)
        self.assertIn("winresource", launcher_cargo)
        self.assertIn("NativeWindowBuilder::new", launcher)
        self.assertTrue((ROOT / "crates" / "host" / "dsh-launcher" / "build.rs").is_file())
        self.assertIn("Command::new", launcher)
        self.assertIn("GetUserDefaultUILanguage", launcher)
        self.assertIn('title: "DeepSeek Harness-rs"', launcher)
        self.assertIn('subtitle: "本机服务与 Web 控制台"', launcher)
        self.assertNotIn('install_skins: "管理皮肤"', launcher)
        self.assertNotIn("InstallSkins", launcher)
        self.assertIn("const LAUNCHER_WINDOW_WIDTH: u32 = 680;", launcher)
        self.assertIn("const LAUNCHER_WINDOW_HEIGHT: u32 = 520;", launcher)
        self.assertIn(".size(LAUNCHER_WINDOW_WIDTH, LAUNCHER_WINDOW_HEIGHT)", launcher)
        self.assertIn("toggle(managed)", launcher)
        self.assertIn("primary_button(state.copy.open_web)", launcher)
        self.assertIn("ThemeColorToken::Surface", launcher)
        self.assertIn("builder.tray(tray)", launcher)
        self.assertIn("let close_command = ZsuiCommand::HideMainWindow", launcher)
        self.assertIn(".on_close_requested(close_command)", launcher)
        zsui_windows = ROOT / "crates" / "vendor" / "zsui" / "src" / "platform" / "windows"
        tray = (zsui_windows / "services" / "tray.rs").read_text(encoding="utf-8")
        window_proc = (zsui_windows / "window_proc.rs").read_text(encoding="utf-8")
        menu = (zsui_windows / "services" / "menu.rs").read_text(encoding="utf-8")
        self.assertIn("Shell_NotifyIconW", tray)
        self.assertIn("present_status_item_menu_at_cursor", tray)
        self.assertIn("dispatch_windows_win32_app_command", tray)
        self.assertIn("window_lifecycle_commands", window_proc)
        self.assertIn("WM_CLOSE", window_proc)
        self.assertIn("dispatch_windows_win32_window_view_input", menu)
        self.assertIn("route.dispatch_app_command(command.clone())", menu)
        self.assertIn("LauncherStateFile", launcher)
        self.assertIn("PROCESS_QUERY_LIMITED_INFORMATION", launcher)
        self.assertIn("stop_owned_child_on_drop", launcher)
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
        self.assertIn(r'DefaultDirName=D:\Program Files (x86)\DeepSeek Harness-rs\{#Variant}', installer)
        self.assertIn('UsePreviousAppDir=yes', installer)
        self.assertIn('DisableDirPage=no', installer)
        self.assertIn('PrepareToInstall', installer)
        self.assertIn('chinesesimp.DesktopShortcut=', installer)
        self.assertIn('chinesesimp.LauncherName=', installer)
        self.assertIn('english.DesktopShortcut=', installer)
        self.assertIn('english.LauncherName=', installer)
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
