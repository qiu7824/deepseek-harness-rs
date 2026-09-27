import pathlib
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[2]
CONVERSATION = ROOT / "web" / "dist" / "plugins" / "ui-conversation.js"
THEME = ROOT / "web" / "dist" / "plugins" / "ui-theme.js"
BASE_CSS = ROOT / "web" / "dist" / "assets" / "index-CSGf6Qzd.css"
HOST = ROOT / "crates" / "host" / "dsh-host" / "src" / "lib.rs"
SUBAGENT_TOOL = ROOT / "crates" / "subagent" / "tool-subagent" / "src" / "lib.rs"
CODEX = ROOT / "crates" / "subagent" / "subagent-codex" / "src" / "lib.rs"


class RustUiContractTests(unittest.TestCase):
    def test_appearance_uses_original_general_row_and_typography(self):
        theme = THEME.read_text(encoding="utf-8")
        host = HOST.read_text(encoding="utf-8")
        for removed in (
            "function FontSizeRow",
            "setFontSize(px)",
            '"fontSize"',
            "FONT_SIZE_MIN",
            "FONT_SIZE_MAX",
            'setProperty("--dsh-content-font-size"',
        ):
            self.assertNotIn(removed, theme)
        namespace = host.split('settings_namespace("ui-theme")', 1)[1].split('settings ui-theme:', 1)[0]
        self.assertNotIn('"fontSize"', namespace)
        self.assertIn('ctx.slots.inject("settings.general.item"', theme)
        self.assertIn('order: 10,', theme)
        self.assertNotIn('restoreBingWallpaper', theme)
        self.assertNotIn('/__dsh-bing-wallpaper', host)

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

    def test_skins_are_retired_and_modes_remain(self):
        theme = THEME.read_text(encoding="utf-8")
        host = HOST.read_text(encoding="utf-8")
        self.assertFalse((ROOT / "web" / "dist" / "skins").exists())
        self.assertFalse((ROOT / "release" / "plugins" / "dsh-skin-center").exists())
        # The prebuilt theme plugin keeps its no-skin boundary: only light and
        # dark are offered and any other stored preference falls back.
        self.assertIn('NO_SKIN && !["light", "dark"].includes(section.preference)', theme)
        self.assertIn('id: "appearance"', theme)
        self.assertIn('object.insert("noSkin".to_string(), serde_json::Value::Bool(true));', host)
        retired = host.split("const RETIRED:", 1)[1].split("];", 1)[0]
        for skin in ("blue-fantasy", "deepseek-official", "harbor", "miku", "minecraft", "trading", "xp"):
            self.assertIn(f'"{skin}"', retired)

    def test_legacy_theme_preferences_have_a_startup_migration(self):
        host = HOST.read_text(encoding="utf-8")
        self.assertIn("fn migrate_legacy_theme_settings", host)
        self.assertIn("retired_theme_preferences_migrate_to_default_light", host)
        self.assertIn("migrate_legacy_theme_settings(document)", host)


if __name__ == "__main__":
    unittest.main()
