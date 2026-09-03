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

    def test_runtime_loads_one_targeted_page_for_an_unloaded_turn(self):
        source = RUNTIME.read_text(encoding="utf-8")
        for required in (
            "historyTargetSeq = null",
            "async loadAround(targetSeq, force = false)",
            "this.history({ afterSeq: targetSeq, maxMessages: HISTORY_PAGE_MESSAGES })",
            "eventContainsSeq(entry.event, targetSeq)",
            "async returnLatest()",
        ):
            self.assertIn(required, source)
        conversation = CONVERSATION.read_text(encoding="utf-8")
        for required in (
            'this.scopedSession("loadAround").loadAround(seq)',
            "loadAround: (seq) => scoped.loadAround(seq)",
            'useProjection("turnOutline")',
            "mergeTurnOutline(timeline, turnOutline)",
            "const TurnNavigator =",
            'className: "dshAlpha3TurnRail"',
            "loadAround(item.anchor.seq)",
            '"chat.turnNavigation.jumpLoad"',
        ):
            self.assertIn(required, conversation)
        self.assertNotIn("loadThrough(item.anchor.seq)", conversation)

    def test_single_live_page_compacts_stream_deltas_without_cutting_its_prefix(self):
        source = RUNTIME.read_text(encoding="utf-8")
        self.assertIn("function compactSingleHistoryPage", source)
        self.assertIn("compactSingleHistoryPage(this.events, this.views)", source)
        self.assertNotIn("this.events.splice(0, excess)", source)

    def test_connection_preserves_directional_history_fields(self):
        source = CONNECTION.read_text(encoding="utf-8")
        for required in (
            "hasMoreBefore: boolean()",
            "hasMoreAfter: boolean()",
            "firstSeq: number().int().nullable()",
            "lastSeq: number().int().nullable()",
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

    def test_context_jump_projection_snapshot_is_reference_stable_while_absent(self):
        source = (ROOT / "release" / "plugins" / "dsh-context-jump" / "lib" / "client.js").read_text(encoding="utf-8")
        self.assertIn("const EMPTY_RAIL_ENTRIES = []", source)
        self.assertIn("getSnapshot: () => currentFace()?.getSnapshot() ?? EMPTY_RAIL_ENTRIES", source)
        self.assertNotIn("getSnapshot: () => currentFace()?.getSnapshot() ?? []", source)

    def test_runtime_session_disposal_is_not_reported_as_durable_session_removal(self):
        host = (ROOT / "crates" / "host" / "apiproxy" / "src" / "proxy.rs").read_text(encoding="utf-8")
        disposed = host[host.index('// session/disposed'):host.index('// workspace/session-deleted')]
        self.assertIn("HostFrame::SessionStatus", disposed)
        self.assertIn("running: false", disposed)
        self.assertNotIn("HostFrame::SessionRemoved", disposed)

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


if __name__ == "__main__":
    unittest.main()
