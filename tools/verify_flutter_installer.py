"""Verify the Windows Flutter installer without executing it."""
import argparse
import json
from pathlib import Path

from package_flutter_release import inventory
from verify_installer_package import verify_windows


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--stage", type=Path, required=True)
    args = parser.parse_args()
    manifest = json.loads((args.stage / "DESKTOP.json").read_text(encoding="utf-8"))
    if manifest["platform"] != "windows" or manifest["files"] != inventory(args.stage):
        raise ValueError("Flutter staged inventory is invalid")
    verify_windows(args.package.resolve(), args.stage.resolve())


if __name__ == "__main__":
    main()
