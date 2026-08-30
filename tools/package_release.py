from __future__ import annotations

import argparse
import json
import pathlib
import shutil
import stat
import tarfile
import zipfile

from build_skin_payload import build_skin_payload

ROOT = pathlib.Path(__file__).resolve().parents[1]



def copy_tree(src: pathlib.Path, dst: pathlib.Path) -> None:
    if src.exists():
        shutil.copytree(src, dst, dirs_exist_ok=True)


def binary_name(platform: str, stem: str) -> str:
    return f"{stem}.exe" if platform == "windows" else stem


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=["windows", "linux", "macos"], required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--variant", choices=["core", "skin"], default="core")
    parser.add_argument("--version", required=True)
    args = parser.parse_args()

    suffix = f"deepseek-harness-rs-v{args.version}-{args.platform}-{args.arch}-{args.variant}"
    stage = ROOT / "dist" / suffix
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)

    core_source = ROOT / "target" / "release" / binary_name(args.platform, "dsh")
    launcher_source = ROOT / "target" / "release" / binary_name(args.platform, "dsh-launcher")
    core_output = binary_name(args.platform, "deepseek-harness-rs")
    launcher_output = binary_name(args.platform, "dsh-launcher")
    shutil.copy2(core_source, stage / core_output)
    shutil.copy2(launcher_source, stage / launcher_output)
    if args.platform != "windows":
        for executable in (stage / core_output, stage / launcher_output):
            executable.chmod(
                executable.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
            )

    copy_tree(ROOT / "release" / "plugins", stage / "plugins")
    copy_tree(ROOT / "web" / "dist", stage / "web" / "dist")
    shutil.rmtree(stage / "web" / "dist" / "skins", ignore_errors=True)
    copy_tree(ROOT / "config" / "agent-presets", stage / "config" / "agent-presets")
    for name in ["README.md", "README.zh.md", "LICENSE", "THIRD_PARTY_NOTICES.md"]:
        if (ROOT / name).exists():
            shutil.copy2(ROOT / name, stage / name)
    shutil.copy2(ROOT / "release" / "PLUGIN_SECURITY.md", stage / "PLUGIN_SECURITY.md")

    entry = launcher_output
    skin_payload = None
    if args.variant == "skin":
        skin_payload = binary_name(args.platform, "deepseek-harness-rs-skin")
        build_skin_payload(stage / skin_payload)

    manifest = {
        "name": suffix,
        "version": args.version,
        "platform": args.platform,
        "arch": args.arch,
        "variant": args.variant,
        "entry": entry,
        "host": core_output,
        "skin_payload": skin_payload,
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
