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

ROOT = pathlib.Path(__file__).resolve().parents[1]
SAFE_RELEASE_COMPONENT = re.compile(r"^[0-9A-Za-z][0-9A-Za-z._-]*$")


def verified_free_model(path: pathlib.Path) -> dict:
    report = json.loads(path.read_text(encoding="utf-8"))
    if report.get("url") != "https://opencode.ai/zen/v1/models" or report.get("model") != "ling-3.0-flash-fin-free":
        raise ValueError("free model verification does not match package defaults")
    if report.get("pricingSource") != "https://opencode.ai/docs/zen/":
        raise ValueError("free model verification requires official pricing evidence")
    if not all(report.get(key) is True for key in ("available", "freePricingVerified", "harnessVerified", "inference", "streaming", "toolCall", "toolResult", "anonymous")):
        raise ValueError("free model verification is incomplete")
    if re.fullmatch(r"[a-f0-9]{64}", str(report.get("binarySha256", ""))) is None:
        raise ValueError("free model verification must identify the tested runtime")
    verified_at = datetime.fromisoformat(report["verifiedAt"])
    if verified_at.tzinfo is None:
        raise ValueError("free model verification timestamp requires a timezone")
    age = (datetime.now(timezone.utc) - verified_at).total_seconds()
    if age < -60 or age > 86400:
        raise ValueError("free model verification must be from the last 24 hours")
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
    shutil.copy2(ROOT / "docs" / "learning-and-capabilities.zh.md", stage / "docs" / "learning-and-capabilities.zh.md")
    for name in ["README.md", "README.zh.md", "LICENSE", "THIRD_PARTY_NOTICES.md"]:
        if (ROOT / name).exists():
            shutil.copy2(ROOT / name, stage / name)
    shutil.copy2(ROOT / "release" / "PLUGIN_SECURITY.md", stage / "PLUGIN_SECURITY.md")

    if args.variant == "free":
        (stage / "free-model-verification.json").write_text(json.dumps(free_verification, ensure_ascii=False, indent=2), encoding="utf-8")
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
                                        "id": "ling-3.0-flash-fin-free",
                                        "name": "Ling 3.0 Flash Fin Free",
                                        "contextWindow": 262144,
                                        "maxTokens": 16384,
                                        "reasoningEfforts": False,
                                    }
                                ],
                            }
                        }
                    },
                    "agent-default-model": {
                        "provider": "opencode-free",
                        "model": "ling-3.0-flash-fin-free",
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
