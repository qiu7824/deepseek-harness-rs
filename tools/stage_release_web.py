from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import shutil


ROOT = pathlib.Path(__file__).resolve().parents[1]
SOURCE = ROOT / "web" / "dist"

REQUIRED_RELIABILITY_BUNDLES = (
    "web/dist/plugins/connection.js",
    "web/dist/plugins/ui-settings-general.js",
    "web/dist/plugins/ui-permission.js",
    "web/dist/plugins/ui-input-trigger.js",
)


def stage_release_web(target: pathlib.Path) -> dict[str, object]:
    if not SOURCE.is_dir():
        raise FileNotFoundError(f"missing web distribution: {SOURCE}")

    missing = [relative for relative in REQUIRED_RELIABILITY_BUNDLES if not (ROOT / relative).is_file()]
    if missing:
        raise FileNotFoundError(f"missing required reliability bundles: {', '.join(missing)}")

    temporary = sorted(
        path.relative_to(SOURCE).as_posix()
        for path in SOURCE.rglob("*")
        if path.is_file()
        and (path.name.startswith(".hermes-tmp") or path.name.endswith(".tmp"))
    )
    if temporary:
        raise ValueError(f"temporary files cannot be staged: {', '.join(temporary)}")

    manifest_path = SOURCE / "plugins" / "manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    for entry in manifest.get("entries", []):
        bundle = SOURCE / entry["url"].lstrip("/")
        if not bundle.is_file():
            raise FileNotFoundError(f"manifest bundle is missing: {entry['url']}")
        actual = hashlib.sha256(bundle.read_bytes()).hexdigest()[:16]
        if entry.get("rev") != actual:
            raise ValueError(
                f"manifest revision is stale for {entry['url']}: "
                f"{entry.get('rev')} != {actual}"
            )

    if target.exists():
        shutil.rmtree(target)
    shutil.copytree(SOURCE, target)

    files = sorted(path for path in target.rglob("*") if path.is_file())
    digest = hashlib.sha256()
    for path in files:
        relative = path.relative_to(target).as_posix().encode("utf-8")
        digest.update(relative)
        digest.update(b"\0")
        digest.update(path.read_bytes())
        digest.update(b"\0")

    return {
        "source": str(SOURCE),
        "target": str(target),
        "files": len(files),
        "sha256": digest.hexdigest(),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--target",
        type=pathlib.Path,
        default=ROOT / "target" / "release" / "web" / "dist",
    )
    args = parser.parse_args()
    print(json.dumps(stage_release_web(args.target.resolve()), ensure_ascii=False))


if __name__ == "__main__":
    main()