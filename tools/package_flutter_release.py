"""Package a native Flutter client with the matching, verified Web core."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

from package_release import binary_name, validated_release_component
from verify_release_version import verify_build_identity

ROOT = Path(__file__).resolve().parents[1]
FLUTTER_REVISION = "6a19cca56475dbfba1478ee68d7bd0c2ef891da1"


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def inventory(root: Path) -> dict:
    result = {}
    for path in sorted(root.rglob("*")):
        name = path.relative_to(root).as_posix()
        if name == "DESKTOP.json":
            continue
        if path.is_symlink():
            target = os.readlink(path)
            if os.path.isabs(target) or not path.resolve().is_relative_to(root.resolve()) or not path.exists():
                raise ValueError(f"bundle link escapes its root or is broken: {name}")
            result[name] = {"link": target}
        elif path.is_file():
            result[name] = {"sha256": digest(path), "bytes": path.stat().st_size,
                            "executable": bool(path.stat().st_mode & 0o111)}
        elif not path.is_dir():
            raise ValueError(f"unsupported bundle entry: {name}")
    return result


def require(root: Path, names: list[str]) -> None:
    for name in names:
        if not (root / name).is_file():
            raise ValueError(f"missing desktop runtime file: {name}")


def stage(client: Path, core: Path, destination: Path, platform: str, arch: str,
          version: str, revision: str) -> dict:
    for field, value in [("version", version), ("arch", arch)]:
        validated_release_component(field, value)
    if (platform, arch) not in {("windows", "x86_64"), ("linux", "x86_64"), ("macos", "x86_64"), ("macos", "aarch64")}:
        raise ValueError("unsupported desktop platform/architecture")
    package = json.loads((core / "PACKAGE.json").read_text(encoding="utf-8"))
    if any(package.get(key) != value for key, value in {
        "version": version, "platform": platform, "arch": arch, "variant": "core",
        "host": binary_name(platform, "deepseek-harness-rs"),
    }.items()):
        raise ValueError("desktop and core package identities differ")
    require(core, [package["host"], binary_name(platform, "dsh-remote-helper"),
                   "web/dist/index.html", "runtime/node/" + binary_name(platform, "node")])
    inventory(client)
    core_inventory = inventory(core)
    if platform == "windows":
        require(client, ["dsh_desktop.exe", "flutter_windows.dll", "data/app.so", "data/icudtl.dat"])
    elif platform == "linux":
        require(client, ["dsh_desktop", "lib/libflutter_linux_gtk.so", "data/icudtl.dat", "lib/libapp.so"])
    else:
        info = plistlib.loads((client / "Contents/Info.plist").read_bytes())
        executable = info.get("CFBundleExecutable")
        if not isinstance(executable, str) or Path(executable).name != executable:
            raise ValueError("invalid macOS application executable")
        require(client, ["Contents/MacOS/" + executable,
                         "Contents/Frameworks/FlutterMacOS.framework/FlutterMacOS",
                         "Contents/Frameworks/App.framework/App"])
    if destination.exists():
        raise ValueError("desktop output already exists")
    destination.mkdir(parents=True)
    if platform == "macos":
        app = destination / "DeepSeek Harness.app"
        shutil.copytree(client, app, symlinks=True)
        host = app / "Contents/Resources/host"
    else:
        shutil.copytree(client, destination, dirs_exist_ok=True, symlinks=True)
        host = destination / "host"
    if host.exists():
        raise ValueError("client build contains an unexpected bundled Host")
    shutil.copytree(core, host, symlinks=True)
    if inventory(host) != core_inventory:
        raise ValueError("bundled Host differs from verified core")
    if platform != "windows":
        host_binary = host / package["host"]
        host_binary.chmod(host_binary.stat().st_mode | 0o111)
        if platform == "linux":
            executable = destination / "dsh_desktop"
            executable.chmod(executable.stat().st_mode | 0o111)
    return {"schemaVersion": 1, "version": version, "platform": platform, "arch": arch,
            "sourceRevision": revision, "flutterRevision": FLUTTER_REVISION,
            "entry": "DeepSeek Harness.app" if platform == "macos" else binary_name(platform, "dsh_desktop"),
            "hostRoot": host.relative_to(destination).as_posix(), "core": package}


def write_manifest(root: Path, metadata: dict) -> None:
    metadata = {**metadata, "files": inventory(root)}
    (root / "DESKTOP.json").write_text(json.dumps(metadata, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def verify_archive(archive: Path, prefix: str, expected: dict) -> None:
    actual = {}
    seen = set()
    manifest = None
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as source:
            for entry in source.infolist():
                if entry.is_dir():
                    continue
                if not entry.filename.startswith(prefix + "/"):
                    raise ValueError("foreign archive root")
                name = entry.filename[len(prefix) + 1:]
                if name in seen:
                    raise ValueError("duplicate archive member")
                seen.add(name)
                with source.open(entry) as stream:
                    if name == "DESKTOP.json":
                        manifest = json.load(stream)
                        continue
                    if name in actual:
                        raise ValueError("duplicate archive member")
                    actual[name] = {"sha256": hashlib.file_digest(stream, "sha256").hexdigest(), "bytes": entry.file_size,
                                    "executable": bool((entry.external_attr >> 16) & 0o111)}
    else:
        with tarfile.open(archive, "r:gz") as source:
            for entry in source:
                if entry.isdir():
                    continue
                if not entry.name.startswith(prefix + "/"):
                    raise ValueError("foreign archive root")
                name = entry.name[len(prefix) + 1:]
                if name in seen:
                    raise ValueError("duplicate archive member")
                seen.add(name)
                if entry.issym():
                    actual[name] = {"link": entry.linkname}
                elif entry.isfile():
                    with source.extractfile(entry) as stream:
                        if name == "DESKTOP.json":
                            manifest = json.load(stream)
                            continue
                        actual[name] = {"sha256": hashlib.file_digest(stream, "sha256").hexdigest(), "bytes": entry.size,
                                        "executable": bool(entry.mode & 0o111)}
                else:
                    raise ValueError("unsupported archive member")
    if manifest != expected or actual != expected["files"]:
        raise ValueError("desktop archive differs from its verified inventory")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=["windows", "linux", "macos"], required=True)
    parser.add_argument("--arch", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--client", type=Path, required=True)
    parser.add_argument("--core", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    args = parser.parse_args()
    version = validated_release_component("version", args.version)
    arch = validated_release_component("arch", args.arch)
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    host = args.core / binary_name(args.platform, "deepseek-harness-rs")
    info = json.loads(subprocess.check_output([str(host.resolve()), "--build-info"], text=True, timeout=15))
    verify_build_identity(info, version, revision)
    prefix = f"deepseek-harness-rs-v{version}-{args.platform}-{arch}-flutter"
    args.output_dir.mkdir(parents=True, exist_ok=True)
    args.work_dir.mkdir(parents=True, exist_ok=True)
    final_stage = args.output_dir / prefix
    suffix = "zip" if args.platform == "windows" else "tar.gz"
    output = args.output_dir / f"{prefix}-portable.{suffix}"
    if final_stage.exists() or output.exists():
        raise ValueError("desktop output already exists; use a new output directory")
    with tempfile.TemporaryDirectory(prefix="flutter-package-", dir=args.work_dir) as temporary:
        root = Path(temporary) / prefix
        metadata = stage(args.client, args.core, root, args.platform, arch, version, revision)
        if args.platform == "macos":
            app = root / "DeepSeek Harness.app"
            expected_arch = "arm64" if arch == "aarch64" else arch
            app_info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
            for binary in (app / "Contents/MacOS" / app_info["CFBundleExecutable"], app / "Contents/Resources/host/deepseek-harness-rs"):
                if expected_arch not in subprocess.check_output(["lipo", "-archs", str(binary)], text=True).split():
                    raise ValueError("macOS client and Host architecture mismatch")
            subprocess.run(["codesign", "--force", "--deep", "--sign", "-", str(app)], check=True)
            subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
            if inventory(app / "Contents/Resources/host") != inventory(args.core):
                raise ValueError("application signing changed the verified Host payload")
        write_manifest(root, metadata)
        expected = json.loads((root / "DESKTOP.json").read_text(encoding="utf-8"))
        archive = Path(temporary) / output.name
        if suffix == "zip":
            if any("link" in entry for entry in expected["files"].values()):
                raise ValueError("Windows desktop archives cannot contain links")
            with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as package:
                for path in sorted(root.rglob("*")):
                    package.write(path, path.relative_to(root.parent).as_posix())
        else:
            with tarfile.open(archive, "w:gz") as package:
                package.add(root, arcname=prefix)
        verify_archive(archive, prefix, expected)
        shutil.copytree(root, final_stage, symlinks=True)
        shutil.copy2(archive, output)
    print(json.dumps({"stage": str(final_stage), "archive": str(output), "sha256": digest(output)}))


if __name__ == "__main__":
    main()
