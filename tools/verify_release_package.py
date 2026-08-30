from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import tarfile
import zipfile


ROOT = pathlib.Path(__file__).resolve().parents[1]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=["windows", "linux", "macos"], required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--variant", choices=["full", "no-skin"], required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()

    suffix = f"deepseek-harness-rs-v{args.version}-{args.platform}-{args.arch}-{args.variant}"
    archive = ROOT / "dist" / (
        f"{suffix}-portable.zip" if args.platform == "windows" else f"{suffix}-portable.tar.gz"
    )
    if not archive.is_file():
        raise SystemExit(f"missing release archive: {archive}")

    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as package:
            names = set(package.namelist())
            read = package.read
            prefix = f"{suffix}/"
            manifest = json.loads(read(prefix + "PACKAGE.json"))
            theme = read(prefix + "web/dist/plugins/ui-theme.js").decode("utf-8")
    else:
        with tarfile.open(archive, "r:gz") as package:
            members = {member.name: member for member in package.getmembers() if member.isfile()}
            names = set(members)
            prefix = f"{suffix}/"

            def read(name: str) -> bytes:
                handle = package.extractfile(members[name])
                if handle is None:
                    raise SystemExit(f"unreadable archive member: {name}")
                return handle.read()

            manifest = json.loads(read(prefix + "PACKAGE.json"))
            theme = read(prefix + "web/dist/plugins/ui-theme.js").decode("utf-8")

    required = {
        prefix + manifest["entry"],
        prefix + "PACKAGE.json",
        prefix + "PLUGIN_SECURITY.md",
        prefix + "web/dist/index.html",
        prefix + "web/dist/plugins/ui-theme.js",
        prefix + "plugins/dsh-context-jump/lib/client.js",
    }
    if args.platform == "windows":
        required.update(
            {
                prefix + "DshServiceManager.ps1",
                prefix + "启动DeepSeek Harness-rs.cmd",
            }
        )
    else:
        required.add(prefix + "deepseek-harness-rs-web")
    missing = sorted(required - names)
    if missing:
        raise SystemExit(f"archive is missing required entries: {missing}")

    marker = prefix + "NO_SKIN"
    if (marker in names) != (args.variant == "no-skin"):
        raise SystemExit("NO_SKIN marker does not match the declared package variant")
    skin_assets = [name for name in names if name.startswith(prefix + "web/dist/skins/")]
    if args.variant == "no-skin" and skin_assets:
        raise SystemExit(f"no-skin archive leaks bundled skin assets: {skin_assets[:5]}")
    if args.variant == "full" and not skin_assets:
        raise SystemExit("full archive is missing bundled skin assets")
    if manifest != {
        "name": suffix,
        "version": args.version,
        "platform": args.platform,
        "arch": args.arch,
        "variant": args.variant,
        "entry": manifest["entry"],
    }:
        raise SystemExit(f"unexpected PACKAGE.json: {manifest}")
    if args.variant == "full":
        for skin in ("whale-song", "blue-fantasy", "harbor", "xp", "dragon-heir", "minecraft", "trading", "miku", "deepseek-official"):
            if f'"{skin}"' not in theme:
                raise SystemExit(f"full package theme bundle is missing {skin}")
    if "NO_SKIN" not in theme or "BasicAppearanceSettings" not in theme:
        raise SystemExit("theme bundle is missing the no-skin runtime boundary")

    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    print(json.dumps({"archive": str(archive), "bytes": archive.stat().st_size, "sha256": digest}, ensure_ascii=False))


if __name__ == "__main__":
    main()
