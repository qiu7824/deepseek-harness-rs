from __future__ import annotations

import argparse
import json
import pathlib
import re
import shutil
import stat
import sys
import tarfile
import zipfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from build_skin_payload import build_skin_payload
from verify_release_version import verify as verify_release_version

ROOT = pathlib.Path(__file__).resolve().parents[1]
SAFE_RELEASE_COMPONENT = re.compile(r"^[0-9A-Za-z][0-9A-Za-z._-]*$")


def validated_release_component(field: str, value: str) -> str:
    if SAFE_RELEASE_COMPONENT.fullmatch(value) is None or value in {".", ".."}:
        raise ValueError(f"invalid {field}: {value!r}")
    return value



def copy_tree(src: pathlib.Path, dst: pathlib.Path) -> None:
    if src.exists():
        shutil.copytree(src, dst, dirs_exist_ok=True)


def binary_name(platform: str, stem: str) -> str:
    return f"{stem}.exe" if platform == "windows" else stem


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=["windows", "linux", "macos"], required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--variant", choices=["core", "skin", "free"], default="core")
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    arch = validated_release_component("arch", args.arch)
    version = validated_release_component("version", args.version)

    core_source = ROOT / "target" / "release" / binary_name(args.platform, "dsh")
    verify_release_version(version, core_source)

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
    staged_web = ROOT / "target" / "release" / "web" / "dist"
    if not staged_web.is_dir():
        raise SystemExit(f"missing staged web distribution: {staged_web}")
    copy_tree(staged_web, stage / "web" / "dist")
    shutil.rmtree(stage / "web" / "dist" / "skins", ignore_errors=True)
    copy_tree(ROOT / "config" / "agent-presets", stage / "config" / "agent-presets")
    (stage / "docs").mkdir(exist_ok=True)
    shutil.copy2(ROOT / "docs" / "storage-compatibility.md", stage / "docs" / "storage-compatibility.md")
    for name in ["README.md", "README.zh.md", "LICENSE", "THIRD_PARTY_NOTICES.md"]:
        if (ROOT / name).exists():
            shutil.copy2(ROOT / name, stage / name)
    shutil.copy2(ROOT / "release" / "PLUGIN_SECURITY.md", stage / "PLUGIN_SECURITY.md")

    if args.variant == "free":
        (stage / "settings.json").write_text(
            json.dumps(
                {
                    "llm-pi-ai": {
                        "providers": {
                            "opencode-free": {
                                "displayName": "OpenCode 免费模型",
                                "keyless": True,
                                "api": "openai-completions",
                                "baseURL": "https://opencode.ai/zen/v1",
                                "models": [
                                    {
                                        "id": "mimo-v2.5-free",
                                        "name": "MiMo V2.5 Free",
                                        "reasoningEfforts": False,
                                    }
                                ],
                            }
                        }
                    },
                    "agent-default-model": {
                        "provider": "opencode-free",
                        "model": "mimo-v2.5-free",
                    },
                },
                ensure_ascii=False,
                indent=2,
            ),
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
