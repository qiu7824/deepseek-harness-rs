from __future__ import annotations

import argparse
import hashlib
import io
import json
import pathlib
import re
import subprocess
import tarfile
import tempfile
import zipfile
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from free_model_evidence import package_defaults


ROOT = pathlib.Path(__file__).resolve().parents[1]
SKIN_MARKER = b"\n__DSH_SKIN_PAYLOAD_V1_4F92C3A7__\n"
SAFE_RELEASE_COMPONENT = re.compile(r"^[0-9A-Za-z][0-9A-Za-z._-]*$")


def validated_release_component(field: str, value: str) -> str:
    if SAFE_RELEASE_COMPONENT.fullmatch(value) is None or value in {".", ".."}:
        raise ValueError(f"invalid {field}: {value!r}")
    return value


def tree_files(root: pathlib.Path) -> dict[str, pathlib.Path]:
    return {
        path.relative_to(root).as_posix(): path
        for path in root.rglob("*")
        if path.is_file()
    }


def assert_same_tree(actual: pathlib.Path, expected: pathlib.Path) -> None:
    actual_files = tree_files(actual)
    expected_files = tree_files(expected)
    if set(actual_files) != set(expected_files):
        raise SystemExit(
            "extracted archive differs from verified stage: "
            f"missing={sorted(set(expected_files)-set(actual_files))[:20]}, "
            f"extra={sorted(set(actual_files)-set(expected_files))[:20]}"
        )
    for name, expected_path in expected_files.items():
        if actual_files[name].read_bytes() != expected_path.read_bytes():
            raise SystemExit(f"extracted archive byte mismatch: {name}")


def extract_portable_archive(archive: pathlib.Path, target: pathlib.Path) -> None:
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as package:
            package.extractall(target)
    else:
        with tarfile.open(archive, "r:gz") as package:
            package.extractall(target, filter="data")


def read_archive_file(archive: pathlib.Path, name: str) -> bytes:
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as package:
            return package.read(name)
    with tarfile.open(archive, "r:gz") as package:
        member = package.getmember(name)
        handle = package.extractfile(member)
        if handle is None:
            raise SystemExit(f"unreadable archive member: {name}")
        return handle.read()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=["windows", "linux", "macos"], required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--variant", choices=["core", "skin", "free"], required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    arch = validated_release_component("arch", args.arch)
    version = validated_release_component("version", args.version)

    suffix = f"deepseek-harness-rs-v{version}-{args.platform}-{arch}-{args.variant}"
    archive = ROOT / "dist" / (
        f"{suffix}-portable.zip" if args.platform == "windows" else f"{suffix}-portable.tar.gz"
    )
    if not archive.is_file():
        raise SystemExit(f"missing release archive: {archive}")

    file_bytes: dict[str, bytes] = {}
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as package:
            names = set(package.namelist())
            prefix = f"{suffix}/"
            manifest_bytes = package.read(prefix + "PACKAGE.json")
            theme_bytes = package.read(prefix + "web/dist/plugins/ui-theme.js")
            if prefix + "settings.defaults.json" in names:
                file_bytes[prefix + "settings.defaults.json"] = package.read(
                    prefix + "settings.defaults.json"
                )
            skin_name = prefix + f"deepseek-harness-rs-skin{'.exe' if args.platform == 'windows' else ''}"
            if skin_name in names:
                file_bytes[skin_name] = package.read(skin_name)
    else:
        with tarfile.open(archive, "r:gz") as package:
            members = {member.name: member for member in package.getmembers() if member.isfile()}
            names = set(members)
            prefix = f"{suffix}/"

            def read_member(name: str) -> bytes:
                handle = package.extractfile(members[name])
                if handle is None:
                    raise SystemExit(f"unreadable archive member: {name}")
                return handle.read()

            manifest_bytes = read_member(prefix + "PACKAGE.json")
            theme_bytes = read_member(prefix + "web/dist/plugins/ui-theme.js")
            if prefix + "settings.defaults.json" in names:
                file_bytes[prefix + "settings.defaults.json"] = read_member(
                    prefix + "settings.defaults.json"
                )
            skin_name = prefix + f"deepseek-harness-rs-skin{'.exe' if args.platform == 'windows' else ''}"
            if skin_name in names:
                file_bytes[skin_name] = read_member(skin_name)
    manifest = json.loads(manifest_bytes)
    theme = theme_bytes.decode("utf-8")

    executable_suffix = ".exe" if args.platform == "windows" else ""
    launcher = prefix + f"dsh-launcher{executable_suffix}"
    host = prefix + f"deepseek-harness-rs{executable_suffix}"
    required = {
        launcher,
        host,
        prefix + "deepseek-black.ico",
        prefix + "PACKAGE.json",
        prefix + "PLUGIN_SECURITY.md",
        prefix + "web/dist/index.html",
        prefix + "web/dist/plugins/ui-theme.js",
        prefix + "plugins/dsh-context-jump/lib/client.js",
        prefix + "plugins/dsh-skin-center/lib/client.js",
    }

    missing = sorted(required - names)
    if missing:
        raise SystemExit(f"archive is missing required entries: {missing}")

    forbidden_launchers = {
        prefix + "DshServiceManager.ps1",
        prefix + "启动DeepSeek Harness-rs.cmd",
        prefix + "deepseek-harness-rs-web",
    }
    leaked_launchers = sorted(forbidden_launchers & names)
    if leaked_launchers:
        raise SystemExit(f"archive leaks retired launcher assets: {leaked_launchers}")

    skin_assets = [name for name in names if name.startswith(prefix + "web/dist/skins/")]
    if skin_assets:
        message = (
            "core archive leaks bundled skin assets"
            if args.variant == "core"
            else "skin executable leaks bundled skin assets"
        )
        raise SystemExit(f"{message}: {skin_assets[:5]}")

    expected_payload = f"deepseek-harness-rs-skin{executable_suffix}"
    skin_entry = prefix + expected_payload
    has_skin_payload = skin_entry in names
    if has_skin_payload != (args.variant == "skin"):
        raise SystemExit("skin payload presence does not match package variant")
    if has_skin_payload:
        payload = file_bytes[skin_entry]
        if args.platform == "windows" and not payload.startswith(b"MZ"):
            raise SystemExit("Windows skin payload is not a PE executable")
        if args.platform != "windows" and payload.startswith(b"#!"):
            raise SystemExit("skin payload depends on a script runtime")
        if SKIN_MARKER not in payload:
            raise SystemExit("skin executable has no embedded skin payload")
        embedded = payload.rsplit(SKIN_MARKER, 1)[1]
        with zipfile.ZipFile(io.BytesIO(embedded)) as package:
            embedded_names = package.namelist()
            if not any(name.startswith("skins/") for name in embedded_names):
                raise SystemExit("skin executable has an empty skin payload")
            if any(name.startswith("web/") for name in embedded_names):
                raise SystemExit("skin executable leaks bundled skin assets outside its payload")

    expected_manifest = {
        "name": suffix,
        "version": version,
        "platform": args.platform,
        "arch": arch,
        "variant": args.variant,
        "entry": f"dsh-launcher{executable_suffix}",
        "host": f"deepseek-harness-rs{executable_suffix}",
        "skin_payload": expected_payload if args.variant == "skin" else None,
        "default_skin": "deepseek-official" if args.variant == "skin" else None,
    }
    if manifest != expected_manifest:
        raise SystemExit(f"unexpected PACKAGE.json: {manifest}")
    if manifest["variant"] != args.variant:
        raise SystemExit("package manifest variant does not match the requested variant")
    settings_entry = prefix + "settings.defaults.json"
    forbidden_skin_paths = sorted(
        name for name in names if "/skins/" in name or "/skin.json" in name
    )
    if forbidden_skin_paths:
        raise SystemExit(
            f"archive leaks physical skin assets outside the embedded skin payload: {forbidden_skin_paths[:5]}"
        )
    if args.variant == "free":
        evidence_entry = prefix + "free-model-verification.json"
        if evidence_entry not in names:
            raise SystemExit("free archive is missing inference verification")
        evidence = json.loads(read_archive_file(archive, evidence_entry))
        digest = hashlib.sha256(read_archive_file(archive, prefix + manifest["host"])).hexdigest()
        try:
            expected_defaults = package_defaults(evidence, digest)
        except (ValueError, KeyError, TypeError) as error:
            raise SystemExit(f"free archive has invalid inference verification: {error}") from error
        if settings_entry not in names:
            raise SystemExit("free archive is missing its package defaults")
        settings = json.loads(file_bytes[settings_entry])
        if settings != expected_defaults:
            raise SystemExit("free archive model defaults differ from the individually verified routes")
    elif args.variant == "skin":
        if settings_entry not in names:
            raise SystemExit("skin archive is missing its default skin settings")
        settings = json.loads(file_bytes[settings_entry])
        if settings != {"ui-theme": {"preference": "deepseek-official"}}:
            raise SystemExit("skin archive does not select the official skin")
    elif settings_entry in names:
        raise SystemExit("core archive unexpectedly carries package defaults")
    if manifest.get("default_skin") != ("deepseek-official" if args.variant == "skin" else None):
        raise SystemExit("package default skin does not match the release variant")
    if "NO_SKIN" not in theme or "BasicAppearanceSettings" not in theme:
        raise SystemExit("theme bundle is missing the runtime no-skin boundary")

    forbidden_names = sorted(
        name
        for name in names
        if "/.hermes-tmp" in name or name.endswith(".tmp")
    )
    if forbidden_names:
        raise SystemExit(f"archive leaks temporary files: {forbidden_names[:5]}")

    staged_root = ROOT / "dist" / suffix
    staged_host = staged_root / f"deepseek-harness-rs{executable_suffix}"
    release_host = ROOT / "target" / "release" / f"dsh{executable_suffix}"
    if not staged_host.is_file() or not release_host.is_file():
        raise SystemExit("release host binary is missing from stage or target")
    if hashlib.sha256(staged_host.read_bytes()).digest() != hashlib.sha256(release_host.read_bytes()).digest():
        raise SystemExit("packaged host does not match target/release host")
    archived_host = read_archive_file(archive, host)
    if hashlib.sha256(archived_host).digest() != hashlib.sha256(release_host.read_bytes()).digest():
        raise SystemExit("archive host does not match target/release host")
    if args.platform == "windows":
        with tempfile.TemporaryDirectory() as temporary:
            extracted_parent = pathlib.Path(temporary)
            extract_portable_archive(archive, extracted_parent)
            extracted_root = extracted_parent / suffix
            if not extracted_root.is_dir():
                raise SystemExit(f"archive root is missing: {extracted_root}")
            assert_same_tree(extracted_root, staged_root)
            extracted_host = extracted_root / f"deepseek-harness-rs{executable_suffix}"
            version_output = subprocess.check_output(
                [str(extracted_host), "--version"], text=True
            ).strip()
            if version_output.rsplit(" ", 1)[-1] != version:
                raise SystemExit(f"packaged host version mismatch: {version_output}")

    plugin_manifest_name = prefix + "web/dist/plugins/manifest.json"
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as package:
            plugin_manifest = json.loads(package.read(plugin_manifest_name))
            for entry in plugin_manifest["entries"]:
                bundle_name = prefix + "web/dist" + entry["url"]
                actual = hashlib.sha256(package.read(bundle_name)).hexdigest()[:16]
                if entry["rev"] != actual:
                    raise SystemExit(f"archive manifest revision is stale: {entry['url']}")
    else:
        with tarfile.open(archive, "r:gz") as package:
            members = {member.name: member for member in package.getmembers() if member.isfile()}
            manifest_handle = package.extractfile(members[plugin_manifest_name])
            if manifest_handle is None:
                raise SystemExit("archive plugin manifest is unreadable")
            plugin_manifest = json.loads(manifest_handle.read())
            for entry in plugin_manifest["entries"]:
                bundle_name = prefix + "web/dist" + entry["url"]
                handle = package.extractfile(members[bundle_name])
                if handle is None:
                    raise SystemExit(f"archive bundle is unreadable: {entry['url']}")
                actual = hashlib.sha256(handle.read()).hexdigest()[:16]
                if entry["rev"] != actual:
                    raise SystemExit(f"archive manifest revision is stale: {entry['url']}")

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    print(
        json.dumps(
            {"archive": str(archive), "bytes": archive.stat().st_size, "sha256": digest},
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()
