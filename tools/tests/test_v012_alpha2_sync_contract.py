import json
import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
PLUGINS = ROOT / "web" / "dist" / "plugins"
UPSTREAM = pathlib.Path(r"D:/deepwork/_upstream_deepseek_harness_v012a2")


class V012Alpha2SyncContractTests(unittest.TestCase):
    def source(self, name: str) -> str:
        return (PLUGINS / name).read_text(encoding="utf-8")

    def upstream(self, relative: str) -> str:
        return (UPSTREAM / relative).read_text(encoding="utf-8")

    def test_connection_recovery_surface_is_synced(self):
        connection = self.source("connection.js")
        settings = self.source("ui-settings-general.js")
        for required in (
            "MANUAL_RECONNECT",
            "NETWORK_STATE_CHANGED",
            "reconnect()",
            "setNetworkAvailable(available)",
            'emitState("disconnected")',
            "watchBrowserNetwork(controller)",
            "stateListeners",
            "registerGenerationSource(source)",
        ):
            self.assertIn(required, connection)
        for required in (
            "ConnectionRecoveryIndicator",
            'connection.error": "连接异常"',
            'connection.retry": "立即重连"',
            'connection.connected": "连接成功"',
            "triggerButton.current?.focus()",
            "connectionState: connection.state",
            "connection.reconnect()",
        ):
            self.assertIn(required, settings)

    def test_fixture_host_describe_satisfies_the_strict_ready_schema(self):
        connection = self.source("connection.js")
        fixture_home = connection.index('const FIXTURE_HOME = "/home/fixture"')
        fixture_describe = connection.index('describe: (request) => ok(request, {', fixture_home)
        fixture_describe_end = connection.index('}),', fixture_describe)
        describe = connection[fixture_describe:fixture_describe_end]
        self.assertIn("home: FIXTURE_HOME", describe)
        self.assertIn('version: "0.0.0-fixture"', describe)
        self.assertIn('cwd: "/tmp/fixture"', describe)
        self.assertIn("attachedSessions", describe)
        self.assertIn("canOpenPath: true", describe)

    def test_connection_generation_source_has_one_runtime_owner(self):
        connection = self.source("connection.js")
        gateway = self.source("api-gateway-client.js")
        runtime = self.source("client-runtime.js")
        self.assertIn("generationListeners", connection)
        self.assertIn("generationReadyTimeoutMs", connection)
        self.assertIn("new ConnectionController(source", connection)
        self.assertIn("connection.generation.getSnapshot()", gateway)
        self.assertNotIn("connection.start({", runtime)
        self.assertIn("createLegacyRemoteEventSource(connection, onMuxEnvelope, onHostEnvelope)", gateway)
        self.assertIn('if (endpoint !== "$events")', gateway)
        self.assertIn("legacyRemoteEvents(signal)", gateway)
        self.assertIn('signal.removeEventListener("abort", done)', gateway)
        self.assertIn('this.ownerCtx.emit("connection/mux-envelope", envelope)', gateway)
        self.assertIn('this.ownerCtx.emit("connection/host-envelope", envelope)', gateway)
        self.assertIn('ctx.on("connection/mux-envelope"', runtime)
        self.assertIn('sessions.handleMuxEnvelope(envelope)', runtime)
        self.assertIn('workspaces.handleHostEnvelope(envelope)', runtime)
        self.assertIn('endpoint === "$events"', gateway)
        self.assertIn("this.legacyStreamOpen !== void 0", gateway)

    def test_generation_ready_precedes_legacy_stream_pumps(self):
        gateway = self.source("api-gateway-client.js")
        legacy_start = gateway.index("function createLegacyRemoteEventSource")
        legacy_end = gateway.index("//#region lib/types/client/index.js", legacy_start)
        legacy = gateway[legacy_start:legacy_end]
        self.assertIn("const description = await connection.api.host.describe", legacy)
        self.assertIn('yield { type: "ready", clientId, host: { home: description.result.value.home } }', legacy)
        self.assertIn("const mux = connection.api.events.mux", legacy)
        self.assertIn("const host = connection.api.events.host", legacy)
        self.assertLess(legacy.index("yield { type: \"ready\""), legacy.index("const mux ="))
        self.assertIn("ready(opening.host)", gateway)
        proxy = (ROOT / "crates" / "host" / "apiproxy" / "src" / "proxy.rs").read_text(encoding="utf-8")
        host_types = (ROOT / "crates" / "host" / "apiproxy" / "src" / "api" / "host.rs").read_text(encoding="utf-8")
        self.assertIn("pub home: String", host_types)
        self.assertIn("home: self.defaults.dsh_home", proxy)

    def test_connection_reconnect_has_single_restart_owner(self):
        connection = self.source("connection.js")
        start = connection.index("reconnect() {")
        end = connection.index("setNetworkAvailable(available)", start)
        reconnect = connection[start:end]
        self.assertNotIn("this.start()", reconnect)
        self.assertNotIn("this.loop()", reconnect)
        self.assertIn("this.current?.abort(MANUAL_RECONNECT)", reconnect)

    def test_manual_reconnect_cannot_hang_after_aborting_the_current_streams(self):
        connection = self.source("connection.js")
        start = connection.index("const failed = new Promise")
        end = connection.index("emitState(state)", start)
        loop = connection[start:end]
        self.assertIn("this.source(ac.signal, reportReady)", loop)
        self.assertIn("rejectSourceLost", loop)

    def test_connection_controller_state_machine_runs_in_a_node_harness(self):
        harness = (ROOT / "tools" / "tests" / "connection_controller_harness.js").read_text(
            encoding="utf-8"
        )
        for required in (
            "manual reconnect creates exactly one replacement generation",
            "natural end creates exactly one replacement generation",
            "offline pauses retry",
            "online resumes exactly one generation",
            'readFileSync(bundlePath, "utf8")',
        ):
            self.assertIn(required, harness)

    def test_node_connection_harness_is_a_release_gate(self):
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("node tools/tests/connection_controller_harness.js", workflow)

    def test_release_stage_contains_the_alpha2_reliability_bundles(self):
        stage = (ROOT / "tools" / "stage_release_web.py").read_text(encoding="utf-8")
        for required in (
            "web/dist/plugins/connection.js",
            "web/dist/plugins/ui-settings-general.js",
            "web/dist/plugins/ui-permission.js",
            "web/dist/plugins/ui-input-trigger.js",
        ):
            self.assertIn(required, stage)
        package = (ROOT / "tools" / "package_release.py").read_text(encoding="utf-8")
        workflow = (ROOT / ".github" / "workflows" / "release.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn('staged_web = ROOT / "target" / "release" / "web" / "dist"', package)
        self.assertIn("python tools/stage_release_web.py", workflow)

    def test_schedule_projection_and_header_catalog_are_composed(self):
        schedule = (ROOT / "crates" / "schedule" / "schedule" / "src" / "lib.rs").read_text(encoding="utf-8")
        cargo = (ROOT / "crates" / "schedule" / "schedule" / "Cargo.toml").read_text(encoding="utf-8")
        manifest = json.loads((PLUGINS / "manifest.json").read_text(encoding="utf-8"))
        self.assertIn("schedule_projection_definition", schedule)
        self.assertIn("register(ctx, schedule_projection_definition())", schedule)
        projection = (ROOT / "crates" / "schedule" / "schedule" / "src" / "projection.rs").read_text(encoding="utf-8")
        framework = (ROOT / "crates" / "session" / "session-projection" / "src" / "index.rs").read_text(encoding="utf-8")
        self.assertIn("header.seed_length.unwrap_or(0)", projection)
        self.assertIn("if event.seq < seed_length", projection)
        self.assertIn("Fn(&SessionHeader)", framework)
        self.assertIn("dsh-session-projection", cargo)
        self.assertTrue(any(row["url"] == "/plugins/ui-schedule.js" for row in manifest["entries"]))
        ui = self.source("ui-schedule.js")
        self.assertIn("ScheduleCatalogAction", ui)
        self.assertIn('useProjection("schedule")', ui)
        self.assertIn('name: "conversation.session.header.actions"', ui)
        self.assertNotIn("useDismissOnOutsidePointer", ui)

    def test_turn_usage_and_time_details_are_synced(self):
        chat = self.source("ui-conversation.js")
        for required in (
            "TurnUsagePanel",
            "TurnTimePanel",
            'aria-haspopup": "dialog"',
            'message.turnUsage.title": "本轮用量"',
            'message.turnTime.duration": "本轮总用时"',
            'message.turnTime.ttft": "首 token 用时（TTFT）"',
            "turnUsageBuckets(data.tokenUsage)",
            'event.type === "step/start"',
            'event.type === "step/end"',
            'event.type === "llm/retry-started"',
            'event.type === "assistant/chunk"',
            'let state = { kind: "idle" }',
            "const closeOpen = (route) =>",
            "let sawEnd = false",
            "cacheRead.every(turnUsageCount)",
            "function isTurnUsageSessionEvent(event)",
            ".filter(isTurnUsageSessionEvent)",
            "deriveTurnTokenUsage(context.matches",
            "...tokenUsage === void 0 ? {} : { tokenUsage }",
            "usageAction:",
        ):
            self.assertIn(required, chat)
        self.assertIn('const optionalNumber = (key)', chat)
        self.assertIn('normalized.cacheReadTokens !== void 0', chat)
        self.assertIn('normalized.cacheWriteTokens !== void 0', chat)
        self.assertIn('normalized.reasoningTokens !== void 0', chat)

    def test_plugin_inventory_is_grouped_by_agent_preset_and_global_scope(self):
        host_types = (ROOT / "crates" / "host" / "plugin-inventory" / "src" / "types.rs").read_text(encoding="utf-8")
        host_index = (ROOT / "crates" / "host" / "plugin-inventory" / "src" / "index.rs").read_text(encoding="utf-8")
        ui = self.source("ui-settings-plugin-inventory.js")
        self.assertIn("agent_presets", host_types)
        self.assertIn("composition_inventory", host_index)
        self.assertIn('get_typed::<Arc<AgentPresets>>("agentPresets", false)', host_index)
        self.assertIn("presets.list().await", host_index)
        remotes = self.source("api-remotes.js")
        self.assertIn('"agentPresets": array(object({', remotes)
        self.assertIn('literal("conditional")', remotes)
        self.assertIn("PresetPluginEnablement", host_types)
        self.assertIn('skip_serializing_if = "Vec::is_empty"', host_types)
        self.assertNotIn("DSH_AGENT_PRESETS_DIR", host_index)
        self.assertNotIn("current_exe()", host_index)
        for required in (
            "fallbackPreset",
            "chosenPreset",
            'data-plugin-scope": "preset"',
            'data-plugin-scope": "global"',
            "presetProvidedDetail",
        ):
            self.assertIn(required, ui)

    def test_permission_labels_are_localized(self):
        permission = self.source("ui-permission.js")
        for required in (
            'preset.readOnly": "仅可查看"',
            'preset.workspaceWrite": "可写入工作区"',
            'preset.fullAccess": "完全权限"',
            "displayPermissionPreset(option.value, option.name, t)",
        ):
            self.assertIn(required, permission)

    def test_input_and_tool_polish_are_synced(self):
        conversation = self.source("ui-conversation.js")
        trigger = self.source("ui-input-trigger.js")
        tool = self.source("ui-tool.js")
        self.assertIn("margin-right:4px", conversation)
        self.assertIn("scrollbar-track{margin-top:8px", conversation)
        self.assertIn("stale-while-revalidate", trigger)
        self.assertIn("drill claim before input mutation", trigger)
        self.assertIn("underline dotted", tool)
        self.assertIn("diffTotals", tool)
        self.assertNotIn("ctx.remote.$host", tool)

    def test_home_logo_hover_animation_is_synced(self):
        conversation = self.source("ui-conversation.js")
        self.assertIn("HeroFish", conversation)
        self.assertIn('attributeName: "d"', conversation)
        self.assertIn("prefers-reduced-motion: reduce", conversation)
        self.assertIn("FISH_LOGO_VIEWBOX ?? { width: 23.16, height: 17.04 }", conversation)

    def test_web_search_errors_name_the_real_endpoint(self):
        provider = (ROOT / "crates" / "web" / "web-search-deepseek" / "src" / "lib.rs").read_text(encoding="utf-8")
        self.assertIn("search_endpoint_error", provider)
        self.assertIn("The web search request used endpoint", provider)
        self.assertIn("Settings > Plugins > Plugin configuration > Web search", provider)

    def test_remote_error_vocabulary_is_shared(self):
        protocol = (ROOT / "crates" / "core" / "typert-protocol" / "src" / "lib.rs").read_text(encoding="utf-8")
        rpc = (ROOT / "crates" / "host" / "apiproxy" / "src" / "api" / "rpc.rs").read_text(encoding="utf-8")
        gateway = self.source("api-gateway-client.js")
        self.assertIn("pub struct RemoteError", protocol)
        self.assertIn("is_dsh_remote_error", protocol)
        self.assertIn('GATEWAY_INTERNAL_CODE: &str = "gateway/internal"', rpc)
        self.assertIn("isDSHRemoteError", gateway)
        self.assertIn('new RemoteError("gateway/internal"', gateway)
        self.assertIn("rebuiltFailure", gateway)
        connection = self.source("connection.js")
        self.assertIn("const rpcErrorSchema = object({", connection)
        self.assertIn("code: string()", connection)
        self.assertIn("details: record(string(), unknown())", connection)
        self.assertNotIn('code: literal("gateway/internal")', connection)

    def test_ignorable_compatibility_is_retained(self):
        event = (ROOT / "crates" / "core" / "session" / "src" / "types.rs").read_text(encoding="utf-8")
        coordinator = (ROOT / "crates" / "session" / "session-persistence" / "src" / "coordinator.rs").read_text(encoding="utf-8")
        sqlite = (ROOT / "crates" / "session" / "session-persistence-sqlite" / "src" / "schema.rs").read_text(encoding="utf-8")
        self.assertIn("pub ignorable: Option<bool>", event)
        self.assertIn("event.ignorable == Some(true)", coordinator)
        self.assertIn("ignorable         INTEGER", sqlite)

    def test_product_version_matches_alpha2(self):
        cargo = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        cli = (ROOT / "crates" / "host" / "dsh-cli" / "Cargo.toml").read_text(encoding="utf-8")
        web = json.loads((ROOT / "web" / "package.json").read_text(encoding="utf-8"))
        self.assertIn('version = "0.1.2-alpha.2"', cargo)
        self.assertIn("version.workspace = true", cli)
        self.assertEqual(web["version"], "0.1.2-alpha.2")


if __name__ == "__main__":
    unittest.main()
