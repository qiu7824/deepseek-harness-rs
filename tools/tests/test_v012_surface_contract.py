import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
CONVERSATION = ROOT / "web" / "dist" / "plugins" / "ui-conversation.js"
THEME = ROOT / "web" / "dist" / "plugins" / "ui-theme.js"
BASE_CSS = ROOT / "web" / "dist" / "assets" / "index-CSGf6Qzd.css"
HOST = ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs"
SUBAGENT_TOOL = ROOT / "crates" / "subagent" / "tool-subagent" / "src" / "lib.rs"
CODEX = ROOT / "crates" / "subagent" / "subagent-codex" / "src" / "lib.rs"


class V012SurfaceContractTests(unittest.TestCase):
    def test_conversation_width_is_draggable_and_persisted(self):
        source = CONVERSATION.read_text(encoding="utf-8")
        for required in (
            'const WIDTH_PREF_KEY = "dsh.conversation.contentWidth"',
            "function WidthHandle",
            'localStorage.setItem(WIDTH_PREF_KEY',
            '"data-width-handle": side',
            "--dsh-chat-user-width",
            "--dsh-conversation-column-width",
        ):
            self.assertIn(required, source)

    def test_content_font_size_is_persisted_and_applied(self):
        theme = THEME.read_text(encoding="utf-8")
        host = HOST.read_text(encoding="utf-8")
        for required in (
            "function FontSizeRow",
            "setFontSize(px)",
            '"fontSize"',
            "FONT_SIZE_MIN",
            "FONT_SIZE_MAX",
            "--dsh-content-font-size",
        ):
            self.assertIn(required, theme)
        self.assertIn('"fontSize"', host)

    def test_cjk_latin_autospace_preserves_literal_surfaces(self):
        source = BASE_CSS.read_text(encoding="utf-8")
        self.assertIn("text-autospace:normal", source)
        self.assertIn("text-autospace:no-autospace", source)
        for literal_surface in ("[data-diff]", "[data-read]", "[data-search]", "[data-terminal]"):
            self.assertIn(literal_surface, source)

    def test_subagent_call_schema_exposes_complete_route(self):
        source = SUBAGENT_TOOL.read_text(encoding="utf-8")
        for field in ("provider", "model", "reasoning_effort", "max_tokens"):
            self.assertIn(f'"{field}"', source)
        self.assertIn("requested provider/model route", source)
        self.assertIn("schema_exposes_optional_per_call_route", source)
        self.assertIn("resolve_requested_provider(&provider, requested_provider)", source)

    def test_codex_provider_forwards_model(self):
        source = CODEX.read_text(encoding="utf-8")
        self.assertIn('thread_params["model"] = Value::String(model)', source)
        self.assertIn("configured model", source)

    def test_claude_code_provider_is_composed(self):
        host = HOST.read_text(encoding="utf-8")
        cargo = (ROOT / "crates" / "host" / "dsh-host" / "Cargo.toml").read_text(encoding="utf-8")
        self.assertIn("dsh-subagent-claude-code", cargo)
        self.assertIn("dsh_subagent_claude_code::apply", host)

    def test_skin_catalog_matches_product_scope(self):
        theme = THEME.read_text(encoding="utf-8")
        host = HOST.read_text(encoding="utf-8")
        skin_root = ROOT / "web" / "dist" / "skins"
        market = (
            "whale-song",
            "blue-fantasy",
            "harbor",
            "xp",
            "dragon-heir",
            "minecraft",
            "trading",
            "miku",
        )
        expected = ("light", "dark", *market, "deepseek-official")
        for skin in expected:
            self.assertIn(f'"{skin}"', theme)
            self.assertIn(f'"{skin}"', host)
        preferences = theme.split("const THEME_PREFERENCES = [", 1)[1].split("];", 1)[0]
        self.assertEqual(tuple(__import__("re").findall(r'"([a-z0-9-]+)"', preferences)), expected)
        self.assertNotIn('"system"', preferences)
        for retired in ("catppuccin", "dracula", "nord", "tokyo-night", "linear", "notion"):
            self.assertNotIn(f'"{retired}"', preferences)
        self.assertIn("dshSkinPicker", theme)
        self.assertIn('"data-dsh-skin-option": skin.id', theme)
        self.assertIn('document.documentElement.setAttribute("data-dsh-skin", skinId)', theme)
        self.assertIn("activateSkinAssets", theme)
        self.assertIn("await activateSkinAssets(id)", theme)
        self.assertIn("restoreActiveSkinAssets", theme)
        self.assertIn("NO_SKIN ? BasicAppearanceSettings", theme)
        catalog = theme.split("const SKIN_CATALOG = Object.freeze([", 1)[1].split("]);", 1)[0]
        self.assertEqual(catalog.count("{ id:"), 11)
        self.assertNotIn('id: "system"', catalog)
        for skin in market:
            directory = skin_root / skin
            for filename in ("skin.css", "skin.json"):
                self.assertTrue((directory / filename).is_file(), f"missing {skin}/{filename}")
            manifest = (directory / "skin.json").read_text(encoding="utf-8")
            self.assertIn(f'"id": "{skin}"', manifest)
            self.assertTrue((directory / "compiled-skin.css").is_file(), f"missing {skin}/compiled-skin.css")
            if '"patches":' in manifest:
                self.assertTrue((directory / "patches.css").is_file(), f"missing {skin}/patches.css")
                self.assertTrue((directory / "compiled-patches.css").is_file(), f"missing {skin}/compiled-patches.css")
            if '"entry": "hooks.mjs"' in manifest:
                self.assertTrue((directory / "hooks.mjs").is_file(), f"missing {skin}/hooks.mjs")
        self.assertIn('"light",\n            "dark",', host)
        host_preferences = host.split("let theme_preference_schema", 1)[1].split("]", 1)[0]
        self.assertNotIn('"system"', host_preferences)
        for retired in ("catppuccin", "dracula", "nord", "tokyo-night", "linear", "notion"):
            self.assertNotIn(f'"{retired}"', host_preferences)
        self.assertIn('Data::String("light".to_string())', host)
        official = skin_root / "deepseek-official" / "skin.css"
        self.assertTrue(official.is_file(), "missing official DeepSeek Harness skin")
        official_css = official.read_text(encoding="utf-8")
        for reference_color in ("#1e232c", "#4d6bfe", "#f9f8f8", "#101113", "#0a0a0a", "#6799fe"):
            self.assertIn(reference_color, official_css.lower())

    def test_legacy_theme_preferences_have_a_startup_migration(self):
        host = HOST.read_text(encoding="utf-8")
        self.assertIn("fn migrate_legacy_theme_settings", host)
        self.assertIn("retired_theme_preferences_migrate_to_default_light", host)
        self.assertIn("migrate_legacy_theme_settings(document)", host)


if __name__ == "__main__":
    unittest.main()
