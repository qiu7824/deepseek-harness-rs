from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import zipfile
from datetime import datetime, timezone

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from verify_release_version import verify as verify_release_version
from stage_node_runtime import stage_node_runtime
from stage_search_runtime import stage_search_runtime
from native_sandbox_identity import IDENTITY_FILE, verify_directory as verify_native_directory

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


def verify_remote_helper(binary: pathlib.Path) -> None:
    if not binary.is_file():
        raise ValueError(f"missing remote helper: {binary}; build dsh-remote-execution --bin dsh-remote-helper")
    protocol_source = ROOT / "crates/workspace/remote-execution/src/protocol.rs"
    declared = re.search(r"pub const PROTOCOL_VERSION: u32 = ([0-9]+);", protocol_source.read_text(encoding="utf-8"))
    if declared is None:
        raise ValueError("remote helper protocol version is missing from source")
    output = subprocess.check_output([str(binary), "--protocol-version"], text=True, timeout=10).strip()
    if output != declared.group(1):
        raise ValueError(f"remote helper protocol mismatch: expected {declared.group(1)}, got {output!r}")


def verify_docx_runtime(root: pathlib.Path) -> None:
    runtime = root / "web/dist/plugins/docx-preview-runtime.js"
    canonical = root / "release/plugins/dsh-sidebar-workbench-suite/lib/docx.js"
    if not runtime.is_file() or not canonical.is_file() or runtime.read_bytes() != canonical.read_bytes():
        raise ValueError("core and sidebar DOCX renderers differ or are missing; rebuild the pinned sidebar assets before packaging")


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
    parser.add_argument("--variant", choices=["core"], default="core")
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    arch = validated_release_component("arch", args.arch)
    version = validated_release_component("version", args.version)

    staged_web = ROOT / "target" / "release" / "web" / "dist"
    verify_staged_web(ROOT / "web" / "dist", staged_web)
    verify_docx_runtime(ROOT)
    core_source = ROOT / "target" / "release" / binary_name(args.platform, "dsh")
    verify_release_version(version, core_source)
    remote_source = ROOT / "target" / "release" / binary_name(args.platform, "dsh-remote-helper")
    verify_remote_helper(remote_source)

    suffix = f"deepseek-harness-rs-v{version}-{args.platform}-{arch}-{args.variant}"
    stage = ROOT / "dist" / suffix
    if stage.exists():
        shutil.rmtree(stage)
    stage.mkdir(parents=True)
    stage_node_runtime(stage, args.platform, arch)
    stage_search_runtime(stage, args.platform, arch)

    launcher_source = ROOT / "target" / "release" / binary_name(args.platform, "dsh-launcher")
    core_output = binary_name(args.platform, "deepseek-harness-rs")
    launcher_output = binary_name(args.platform, "dsh-launcher")
    shutil.copy2(core_source, stage / core_output)
    shutil.copy2(launcher_source, stage / launcher_output)
    shutil.copy2(remote_source, stage / remote_source.name)
    if args.platform == "windows":
        native_source = ROOT / "target" / "native-windows-sandbox" / "release"
        verify_native_directory(ROOT, native_source)
        native_stage = stage / "native-sandbox"
        native_stage.mkdir()
        for helper in ("dsh-windows-native.exe", "dsh-command-runner.exe", "dsh-windows-sandbox-setup.exe"):
            if not (native_source / helper).is_file():
                raise SystemExit(f"missing native sandbox helper: {helper}; build native/windows-sandbox first")
            shutil.copy2(native_source / helper, native_stage / helper)
        for notice in ("LICENSE", "NOTICE"):
            shutil.copy2(ROOT / "native" / "windows-sandbox" / "engine" / notice, native_stage / notice)
        shutil.copy2(ROOT / "native" / "windows-sandbox" / "UPSTREAM.json", native_stage / "UPSTREAM.json")
        shutil.copy2(native_source / IDENTITY_FILE, native_stage / IDENTITY_FILE)
        native_hashes={name:hashlib.sha256((native_stage/name).read_bytes()).hexdigest() for name in ("dsh-windows-native.exe","dsh-command-runner.exe","dsh-windows-sandbox-setup.exe")}
        migration=(ROOT/'tools/native_install_upgrade.cjs').read_text(encoding='utf-8').replace('__DSH_NATIVE_EXPECTED_HASHES__',json.dumps(native_hashes))
        (stage/'runtime/native-install-upgrade.cjs').write_text(migration,encoding='utf-8')
        for controller in ("dsh-desktop-controller", "dsh-uu-controller"):
            controller_source = ROOT / "target" / "release" / f"{controller}.exe"
            if not controller_source.is_file():
                raise SystemExit(f"missing controller binary: build {controller} before packaging")
            shutil.copy2(controller_source, stage / controller_source.name)
    shutil.copy2(
        ROOT / "packaging" / "windows" / "deepseek-black.ico",
        stage / "deepseek-black.ico",
    )
    shutil.copy2(ROOT / "packaging" / "windows" / "deepseek-black.png", stage / "deepseek-black.png")
    if args.platform != "windows":
        for executable in (stage / core_output, stage / launcher_output, stage / remote_source.name):
            executable.chmod(
                executable.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH
            )

    copy_tree(ROOT / "release" / "plugins", stage / "plugins")
    shutil.rmtree(stage / "plugins" / "dsh-skin-center", ignore_errors=True)
    copy_tree(staged_web, stage / "web" / "dist")
    shutil.rmtree(stage / "web" / "dist" / "skins", ignore_errors=True)
    copy_tree(ROOT / "config" / "agent-presets", stage / "config" / "agent-presets")
    (stage / "docs").mkdir(exist_ok=True)
    shutil.copy2(ROOT / "docs" / "storage-compatibility.md", stage / "docs" / "storage-compatibility.md")
    shutil.copy2(ROOT / "docs" / "protocol-matrix.md", stage / "docs" / "protocol-matrix.md")
    shutil.copy2(ROOT / "docs" / "computer-use-compatibility.zh.md", stage / "docs" / "computer-use-compatibility.zh.md")
    shutil.copy2(ROOT / "docs" / "learning-and-capabilities.zh.md", stage / "docs" / "learning-and-capabilities.zh.md")
    shutil.copy2(ROOT / "docs" / "sidebar-capabilities.md", stage / "docs" / "sidebar-capabilities.md")
    shutil.copy2(ROOT / "docs" / "browser-control-and-model-tools.zh.md", stage / "docs" / "browser-control-and-model-tools.zh.md")
    shutil.copy2(ROOT / "docs" / "sidebar-extension-api.md", stage / "docs" / "sidebar-extension-api.md")
    shutil.copy2(ROOT / "docs" / "sidebar-workbench-suite.md", stage / "docs" / "sidebar-workbench-suite.md")
    shutil.copy2(ROOT / "docs" / "rust-conversation-scrolling.zh.md", stage / "docs" / "rust-conversation-scrolling.zh.md")
    shutil.copy2(ROOT / "docs" / "response-completeness.zh.md", stage / "docs" / "response-completeness.zh.md")
    shutil.copy2(ROOT / "docs" / "devin-subscription.zh.md", stage / "docs" / "devin-subscription.zh.md")
    shutil.copy2(ROOT / "docs" / "image-generation-and-task-models.zh.md", stage / "docs" / "image-generation-and-task-models.zh.md")
    shutil.copy2(ROOT / "docs" / "project-tasks-and-memory-import.zh.md", stage / "docs" / "project-tasks-and-memory-import.zh.md")
    for name in ["workspace-scratch-policy-design.zh.md", "workspace-scratch-open-source-study.zh.md", "ultra-codex-usage-reset-plan.zh.md", "upstream-v0.1.5-alpha.1-evaluation.zh.md", "upstream-v0.1.5-alpha.2-evaluation.zh.md", "uu-self-connect-probe.zh.md"]:
        shutil.copy2(ROOT / "docs" / name, stage / "docs" / name)
    for name in ["README.md", "README.zh.md", "README.en.md", "LICENSE", "THIRD_PARTY_NOTICES.md"]:
        if (ROOT / name).exists():
            shutil.copy2(ROOT / name, stage / name)
    shutil.copy2(ROOT / "release" / "PLUGIN_SECURITY.md", stage / "PLUGIN_SECURITY.md")
    release_notes = ROOT / "release" / "notes" / f"v{version}.md"
    if release_notes.is_file():
        (stage / "release" / "notes").mkdir(parents=True, exist_ok=True)
        shutil.copy2(release_notes, stage / "release" / "notes" / release_notes.name)

    entry = launcher_output
    manifest = {
        "name": suffix,
        "version": version,
        "platform": args.platform,
        "arch": arch,
        "variant": args.variant,
        "entry": entry,
        "host": core_output,
        "skin_payload": None,
        "default_skin": None,
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
