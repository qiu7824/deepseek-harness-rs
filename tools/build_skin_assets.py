from __future__ import annotations

import json
import pathlib
import re

import tinycss2


ROOT = pathlib.Path(__file__).resolve().parents[1]
SKIN_ROOT = ROOT / "web" / "dist" / "skins"
MARKET_SKINS = (
    "whale-song",
    "blue-fantasy",
    "harbor",
    "xp",
    "dragon-heir",
    "minecraft",
    "trading",
    "miku",
)
SCOPE = 'html[data-dsh-skin="{}"]'
ROOT_BODY_TOKEN = re.compile(r"^(?:--dsw-alias-|--dsw-specific-)")
OFFICIAL_DATA_HEAD = re.compile(r"^\[data-ds-[a-z0-9-]+")


def split_selector_list(selector: str) -> list[str]:
    parts: list[str] = []
    current: list[str] = []
    parens = 0
    brackets = 0
    quote: str | None = None
    index = 0
    while index < len(selector):
        char = selector[index]
        if quote is not None:
            current.append(char)
            if char == "\\" and index + 1 < len(selector):
                index += 1
                current.append(selector[index])
            elif char == quote:
                quote = None
            index += 1
            continue
        if char in ('"', "'"):
            quote = char
        elif char == "(":
            parens += 1
        elif char == ")":
            parens -= 1
        elif char == "[":
            brackets += 1
        elif char == "]":
            brackets -= 1
        if char == "," and parens == 0 and brackets == 0:
            parts.append("".join(current))
            current = []
        else:
            current.append(char)
        index += 1
    parts.append("".join(current))
    return parts


def scope_selector(selector: str, skin_id: str) -> str:
    scope = SCOPE.format(skin_id)
    stripped = selector.strip()
    if stripped == ":root" or stripped.startswith(":root "):
        return scope + stripped[len(":root") :]
    if stripped.startswith("html[data-ds-"):
        return scope + " body" + stripped[len("html") :]
    if stripped == "html" or stripped.startswith("html "):
        return scope + stripped[len("html") :]
    if stripped == "body" or stripped.startswith(("body ", "body[", "body:")):
        return f"{scope} {stripped}"
    if OFFICIAL_DATA_HEAD.match(stripped):
        return f"{scope} body{stripped}"
    return f"{scope} {stripped}"


def declaration_text(nodes: list[object]) -> str:
    return tinycss2.serialize(nodes).strip()


def root_clone(rule: object, skin_id: str) -> str:
    prelude = tinycss2.serialize(rule.prelude).strip()
    if not any(part.strip() in (":root", "html") for part in split_selector_list(prelude)):
        return ""
    declarations = tinycss2.parse_declaration_list(rule.content, skip_comments=True, skip_whitespace=True)
    copied: list[str] = []
    for declaration in declarations:
        if declaration.type != "declaration":
            continue
        name = declaration.name
        if ROOT_BODY_TOKEN.match(name) or name in ("background-color", "background-image"):
            value = tinycss2.serialize(declaration.value).strip()
            copied.append(f"{name}: {value}{' !important' if declaration.important and not name.startswith('--') else ''};")
    if not copied:
        return ""
    return f'\n{SCOPE.format(skin_id)} body {{\n  ' + "\n  ".join(copied) + "\n}\n"


def compile_rules(rules: list[object], skin_id: str) -> str:
    output: list[str] = []
    for rule in rules:
        if rule.type == "qualified-rule":
            selector = tinycss2.serialize(rule.prelude)
            scoped = ",".join(scope_selector(part, skin_id) for part in split_selector_list(selector))
            output.append(f"{scoped}{{{tinycss2.serialize(rule.content)}}}")
            output.append(root_clone(rule, skin_id))
            continue
        if rule.type == "at-rule" and rule.content is not None and rule.lower_at_keyword in (
            "media",
            "supports",
            "container",
            "layer",
            "scope",
            "document",
        ):
            nested = tinycss2.parse_rule_list(rule.content, skip_comments=False, skip_whitespace=False)
            output.append(
                f"@{rule.at_keyword} {tinycss2.serialize(rule.prelude).strip()}"
                + "{"
                + compile_rules(nested, skin_id)
                + "}"
            )
            continue
        output.append(tinycss2.serialize([rule]))
    return "".join(output)


def compile_css(source: str, skin_id: str, main: bool) -> str:
    rules = tinycss2.parse_stylesheet(source, skip_comments=False, skip_whitespace=False)
    compiled = compile_rules(rules, skin_id)
    if main:
        scope = SCOPE.format(skin_id)
        compiled += f'\n{scope} [id="root"] {{ background: transparent; }}\n'
        compiled += f'{scope} body {{ --shiki-background: var(--dsw-alias-markdown-code-block); }}\n'
    return compiled


def compile_market_skin(skin_id: str) -> None:
    directory = SKIN_ROOT / skin_id
    manifest = json.loads((directory / "skin.json").read_text(encoding="utf-8"))
    source = (directory / manifest["contributes"]["stylesheet"]).read_text(encoding="utf-8")
    compiled_stylesheet = f"compiled-{pathlib.Path(manifest['contributes']['stylesheet']).name}"
    (directory / compiled_stylesheet).write_text(compile_css(source, skin_id, True), encoding="utf-8")
    patches = manifest["contributes"].get("patches")
    if patches:
        source = (directory / patches).read_text(encoding="utf-8")
        compiled_patches = f"compiled-{pathlib.Path(patches).name}"
        (directory / compiled_patches).write_text(compile_css(source, skin_id, False), encoding="utf-8")


def build() -> None:
    for skin_id in MARKET_SKINS:
        compile_market_skin(skin_id)


if __name__ == "__main__":
    build()
