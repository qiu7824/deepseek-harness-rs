from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import tarfile
import zipfile


ROOT = pathlib.Path(__file__).resolve().parents[1]
SKIN_MARKER = b"\n__DSH_SKIN_PAYLOAD__\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=["windows", "linux", "macos"], required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--variant", choices=["core", "skin"], required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()

    suffix = f"deepseek-harness-rs-v{args.version}-{args.platform}-{args.arch}-{args.variant}"
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
        prefix + "PACKAGE.json",
        prefix + "PLUGIN_SECURITY.md",
        prefix + "web/dist/index.html",
        prefix + "web/dist/plugins/ui-theme.js",
        prefix + "plugins/dsh-context-jump/lib/client.js",
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
        if SKIN_MARKER not in payload:
            raise SystemExit("skin executable has no embedded skin payload")
        embedded = payload.rsplit(SKIN_MARKER, 1)[1]
        with zipfile.ZipFile(__import__("io").BytesIO(embedded)) as package:
            embedded_names = package.namelist()
            if not any(name.startswith("skins/") for name in embedded_names):
                raise SystemExit("skin executable has an empty skin payload")
            if any(name.startswith("web/") for name in embedded_names):
                raise SystemExit("skin executable leaks bundled skin assets outside its payload")

    expected_manifest = {
        "name": suffix,
        "version": args.version,
        "platform": args.platform,
        "arch": args.arch,
        "variant": args.variant,
        "entry": f"dsh-launcher{executable_suffix}",
        "host": f"deepseek-harness-rs{executable_suffix}",
        "skin_payload": expected_payload if args.variant == "skin" else None,
    }
    if manifest != expected_manifest:
        raise SystemExit(f"unexpected PACKAGE.json: {manifest}")
    if "NO_SKIN" not in theme or "BasicAppearanceSettings" not in theme:
        raise SystemExit("theme bundle is missing the runtime no-skin boundary")

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    print(
        json.dumps(
            {"archive": str(archive), "bytes": archive.stat().st_size, "sha256": digest},
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()
