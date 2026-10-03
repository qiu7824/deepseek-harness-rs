#!/usr/bin/env python3
"""Explicit per-user launcher registration for a relocatable Linux bundle."""
from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

APPLICATION_ID = "io.deepseek.harness.dsh_desktop"
OWNER_KEY = "X-DeepSeek-Harness-Bundle="


def desktop_value(value: str) -> str:
    if any(ord(char) < 32 for char in value):
        raise ValueError("desktop launcher paths cannot contain control characters")
    return value.replace("\\", "\\\\")


def exec_argument(value: str) -> str:
    # Desktop Entry quoting is followed by general string escaping.
    if "=" in value:
        raise ValueError("desktop executable paths cannot contain an equal sign")
    escaped = value.replace("%", "%%")
    for char in ('\\', '"', '`', '$'):
        escaped = escaped.replace(char, "\\" + char)
    return desktop_value('"' + escaped + '"')


def owner(bundle: Path) -> str:
    return OWNER_KEY + hashlib.sha256(os.fsencode(bundle.resolve())).hexdigest()


def atomic_write(path: Path, data: bytes) -> None:
    if path.is_symlink():
        raise ValueError(f"launcher destination is a symbolic link: {path}")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".dsh-entry-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as stream:
            stream.write(data)
        os.chmod(temporary, 0o644)
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def integrate(bundle: Path, data_home: Path, *, remove: bool = False) -> bool:
    bundle = bundle.resolve()
    data_home = data_home.expanduser().absolute()
    desktop = data_home / "applications" / (APPLICATION_ID + ".desktop")
    icon = data_home / "icons/hicolor/scalable/apps" / (APPLICATION_ID + ".svg")
    if desktop.is_symlink() or icon.is_symlink():
        raise ValueError("launcher or icon destination is a symbolic link")
    current = desktop.read_text(encoding="utf-8") if desktop.exists() else ""
    ownership = owner(bundle)
    if remove:
        # A previously installed version must not remove a newer installation.
        if ownership not in current.splitlines():
            return False
        desktop.unlink()
        source_icon = bundle / "share/icons/hicolor/scalable/apps" / icon.name
        if icon.is_file() and source_icon.is_file() and icon.read_bytes() == source_icon.read_bytes():
            icon.unlink()
        return True

    executable = bundle / "dsh_desktop"
    template = bundle / "share/applications" / desktop.name
    source_icon = bundle / "share/icons/hicolor/scalable/apps" / icon.name
    for source in (executable, template, source_icon):
        if not source.is_file():
            raise ValueError(f"incomplete application bundle: {source}")
    if current and not any(line.startswith(OWNER_KEY) for line in current.splitlines()):
        raise ValueError("an unmanaged launcher already exists; preserve or remove it before installing")
    if icon.exists() and not current:
        raise ValueError("an unmanaged icon already exists; preserve or remove it before installing")
    replacements = {
        "Exec": exec_argument(str(executable)),
        "TryExec": desktop_value(str(executable)),
    }
    lines = []
    for line in template.read_text(encoding="utf-8").splitlines():
        key = line.partition("=")[0]
        lines.append(f"{key}={replacements[key]}" if key in replacements else line)
    lines.extend(["Path=" + desktop_value(str(bundle)), ownership])
    previous_icon = icon.read_bytes() if icon.is_file() else None
    atomic_write(icon, source_icon.read_bytes())
    try:
        atomic_write(desktop, ("\n".join(lines) + "\n").encode("utf-8"))
    except OSError:
        if previous_icon is None:
            icon.unlink()
        else:
            atomic_write(icon, previous_icon)
        raise
    return True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--install", action="store_true")
    action.add_argument("--uninstall", action="store_true")
    parser.add_argument("--data-home", type=Path,
                        default=Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share"))
    args = parser.parse_args()
    try:
        changed = integrate(Path(__file__).resolve().parent, args.data_home, remove=args.uninstall)
    except (OSError, ValueError) as error:
        parser.exit(1, f"{error}\n")
    if changed:
        refresh = shutil.which("update-desktop-database")
        if refresh:
            subprocess.run([refresh, str(args.data_home / "applications")], check=False)
        print("Desktop launcher removed." if args.uninstall else "Desktop launcher installed.")
    else:
        print("No desktop launcher belonging to this bundle is installed.")


if __name__ == "__main__":
    main()
