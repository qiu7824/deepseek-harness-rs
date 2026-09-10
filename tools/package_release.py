from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import shutil
import stat
import sys
import tarfile
import zipfile
from datetime import datetime, timezone

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from build_skin_payload import build_skin_payload
from verify_release_version import verify as verify_release_version
from free_model_evidence import package_defaults, validated_models

ROOT = pathlib.Path(__file__).resolve().parents[1]
SAFE_RELEASE_COMPONENT = re.compile(r"^[0-9A-Za-z][0-9A-Za-z._-]*$")


def verified_free_model(path: pathlib.Path) -> dict:
    report = json.loads(path.read_text(encoding="utf-8"))
    validated_models(report)
    return report


def validated_release_component(field: str, value: str) -> str:
    if SAFE_RELEASE_COMPONENT.fullmatch(value) is None or value in {".", ".."}:
        raise ValueError(f"invalid {field}: {value!r}")
    return value



def copy_tree(src: pathlib.Path, dst: pathlib.Path) -> None:
    if src.exists():
        shutil.copytree(src, dst, dirs_exist_ok=True)


def binary_name(platform: str, stem: str) -> str:
    return f"{stem}.exe" if platform == "windows" else stem


def verify_staged_web(source: pathlib.Path, staged: pathlib.Path) -> None:
    instruction = "Run python tools/stage_release_web.py before packaging."
    if not staged.is_dir():
        raise ValueError(f"missing staged web distribution: {staged}. {instruction}")

    def inventory(directory: pathlib.Path, label: str) -> dict[str, tuple[str, str]]:
        if not directory.is_dir():
            raise ValueError(f"missing {label} web distribution: {directory}. {instruction}")
        entries = {}
        for path in [directory, *directory.rglob("*")]:
            relative = path.relative_to(directory).as_posix()
            if path.is_symlink() or getattr(path, "is_junction", lambda: False)():
                raise ValueError(f"linked {label} web entry is not supported: {relative}. {instruction}")
            if path.is_dir():
                entries[relative] = ("directory", "")
            elif path.is_file() and stat.S_ISREG(path.stat().st_mode):
                digest = hashlib.sha256()
                with path.open("rb") as stream:
                    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                        digest.update(chunk)
                entries[relative] = ("file", digest.hexdigest())
            else:
                raise ValueError(f"invalid {label} web entry: {relative}. {instruction}")
        return entries

    expected, actual = inventory(source, "source"), inventory(staged, "staged")
    missing, extra = sorted(expected.keys() - actual.keys()), sorted(actual.keys() - expected.keys())
    changed = sorted(name for name in expected.keys() & actual.keys() if expected[name] != actual[name])
    if missing or extra or changed:
        raise ValueError(
            f"stale staged web distribution: missing={missing}, extra={extra}, changed={changed}. {instruction}"
        )
    try:
        manifest = json.loads((source / "plugins" / "manifest.json").read_text(encoding="utf-8"))
        entries = manifest["entries"]
        if not isinstance(entries, list) or not entries:
            raise ValueError("manifest entries must be a nonempty list")
        for entry in entries:
            url = entry["url"]
            if not isinstance(url, str) or "\\" in url or ":" in url:
                raise ValueError("invalid manifest bundle URL")
            relative = pathlib.PurePosixPath(url.lstrip("/"))
            if ".." in relative.parts:
                raise ValueError("manifest bundle URL leaves the web distribution")
            bundle = expected.get(relative.as_posix())
            if bundle is None or bundle[0] != "file" or entry.get("rev") != bundle[1][:16]:
                raise ValueError(f"missing bundle or stale manifest revision: {url}")
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise ValueError(
            f"invalid web manifest: {error}. Rebuild web/dist and run python tools/stage_release_web.py before packaging."
        ) from error


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=["windows", "linux", "macos"], required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--variant", choices=["core", "skin", "free"], default="core")
    parser.add_argument("--version", required=True)
    parser.add_argument("--free-verification", type=pathlib.Path, default=ROOT / "target" / "free-model-verification.json")
    args = parser.parse_args()
    arch = validated_release_component("arch", args.arch)
    version = validated_release_component("version", args.version)
    free_verification = verified_free_model(args.free_verification) if args.variant == "free" else None

    staged_web = ROOT / "target" / "release" / "web" / "dist"
    verify_staged_web(ROOT / "web" / "dist", staged_web)
    core_source = ROOT / "target" / "release" / binary_name(args.platform, "dsh")
    verify_release_version(version, core_source)
    if free_verification is not None and free_verification["binarySha256"] != hashlib.sha256(core_source.read_bytes()).hexdigest():
        raise ValueError("free model verification belongs to a different runtime binary")

    suffix = f"deepseek-harness-rs-v{version}-{args.platform}-{arch}-{args.variant}"
    stage = ROOT / "dist" / suffix
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)

    launcher_source = ROOT / "target" / "release" / binary_name(args.platform, "dsh-launcher")
    core_output = binary_name(args.platform, "deepseek-harness-rs")
    launcher_output = binary_name(args.platform, "dsh-launcher")
    shutil.copy2(core_source, stage / core_output)
    shutil.copy2(launcher_source, stage / launcher_output)
    if args.platform == "windows":
        for controller in ("dsh-desktop-controller", "dsh-uu-controller"):
            controller_source = ROOT / "target" / "release" / f"{controller}.exe"
            if not controller_source.is_file():
                raise SystemExit(f"missing controller binary: build {controller} before packaging")
            shutil.copy2(controller_source, stage / controller_source.name)
    shutil.copy2(
        ROOT / "packaging" / "windows" / "deepseek-black.ico",
        stage / "deepseek-black.ico",
    )
    skin_source = ROOT / "target" / "release" / binary_name(args.platform, "dsh-skin-installer")
    if args.platform != "windows":
        for executable in (stage / core_output, stage / launcher_output):
            executable.chmod(
                executable.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
            )

    copy_tree(ROOT / "release" / "plugins", stage / "plugins")
    if args.variant != "skin":
        shutil.rmtree(stage / "plugins" / "dsh-skin-center", ignore_errors=True)
    copy_tree(staged_web, stage / "web" / "dist")
    shutil.rmtree(stage / "web" / "dist" / "skins", ignore_errors=True)
    copy_tree(ROOT / "config" / "agent-presets", stage / "config" / "agent-presets")
    (stage / "docs").mkdir(exist_ok=True)
    shutil.copy2(ROOT / "docs" / "storage-compatibility.md", stage / "docs" / "storage-compatibility.md")
    shutil.copy2(ROOT / "docs" / "learning-and-capabilities.zh.md", stage / "docs" / "learning-and-capabilities.zh.md")
    shutil.copy2(ROOT / "docs" / "sidebar-capabilities.md", stage / "docs" / "sidebar-capabilities.md")
    shutil.copy2(ROOT / "docs" / "browser-control-and-model-tools.zh.md", stage / "docs" / "browser-control-and-model-tools.zh.md")
    shutil.copy2(ROOT / "docs" / "sidebar-extension-api.md", stage / "docs" / "sidebar-extension-api.md")
    shutil.copy2(ROOT / "docs" / "sidebar-workbench-suite.md", stage / "docs" / "sidebar-workbench-suite.md")
    shutil.copy2(ROOT / "docs" / "rust-conversation-scrolling.zh.md", stage / "docs" / "rust-conversation-scrolling.zh.md")
    shutil.copy2(ROOT / "docs" / "response-completeness.zh.md", stage / "docs" / "response-completeness.zh.md")
    for name in ["workspace-scratch-policy-design.zh.md", "workspace-scratch-open-source-study.zh.md", "ultra-codex-usage-reset-plan.zh.md", "upstream-v0.1.5-alpha.1-evaluation.zh.md", "upstream-v0.1.5-alpha.2-evaluation.zh.md", "uu-self-connect-probe.zh.md"]:
        shutil.copy2(ROOT / "docs" / name, stage / "docs" / name)
    for name in ["README.md", "README.zh.md", "LICENSE", "THIRD_PARTY_NOTICES.md"]:
        if (ROOT / name).exists():
            shutil.copy2(ROOT / name, stage / name)
    shutil.copy2(ROOT / "release" / "PLUGIN_SECURITY.md", stage / "PLUGIN_SECURITY.md")

    if args.variant == "free":
        (stage / "free-model-verification.json").write_text(json.dumps(free_verification, ensure_ascii=False, indent=2), encoding="utf-8")
        (stage / "settings.json").write_text(
            json.dumps(package_defaults(free_verification, hashlib.sha256(core_source.read_bytes()).hexdigest()), ensure_ascii=False, indent=2),
            encoding="utf-8",
        )

    entry = launcher_output
    skin_payload = None
    if args.variant == "skin":
        skin_payload = binary_name(args.platform, "deepseek-harness-rs-skin")
        build_skin_payload(skin_source, stage / skin_payload)
        if args.platform != "windows":
            (stage / skin_payload).chmod(
                (stage / skin_payload).stat().st_mode
                | stat.S_IXUSR
                | stat.S_IXGRP
                | stat.S_IXOTH
            )

    default_skin = "deepseek-official" if args.variant == "skin" else None
    if default_skin is not None:
        (stage / "settings.defaults.json").write_text(
            json.dumps(
                {"ui-theme": {"preference": default_skin}},
                ensure_ascii=False,
                indent=2,
            ),
            encoding="utf-8",
        )
    elif args.variant == "free":
        (stage / "settings.defaults.json").write_text(
            (stage / "settings.json").read_text(encoding="utf-8"),
            encoding="utf-8",
        )
        (stage / "settings.json").unlink()

    manifest = {
        "name": suffix,
        "version": version,
        "platform": args.platform,
        "arch": arch,
        "variant": args.variant,
        "entry": entry,
        "host": core_output,
        "skin_payload": skin_payload,
        "default_skin": default_skin,
    }
    (stage / "PACKAGE.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2), encoding="utf-8"
    )

    if args.platform == "windows":
        output = ROOT / "dist" / f"{suffix}-portable.zip"
        with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
            for file in stage.rglob("*"):
                if file.is_file():
                    archive.write(file, pathlib.Path(suffix) / file.relative_to(stage))
    else:
        output = ROOT / "dist" / f"{suffix}-portable.tar.gz"
        with tarfile.open(output, "w:gz") as archive:
            archive.add(stage, arcname=suffix)
    print(stage)
    print(output)


if __name__ == "__main__":
    main()
