"""Exercise the real Inno directory hooks using a complete, isolated payload.

Never runs the distributable installer: the derived verifier has its own AppId,
no shortcuts, no registry registration, no uninstaller and no post-install run.
Its Pascal hook rejects every destination outside --scratch.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
import pathlib
import re
import subprocess
import uuid

ROOT = pathlib.Path(__file__).resolve().parents[1]

def require_warning_free_compile(log: pathlib.Path) -> None:
    output = log.read_text(encoding="utf-8-sig", errors="replace")
    if re.search(r"(?im)^\s*Warning:", output):
        raise SystemExit(f"Inno compile emitted warnings; see {log}")

def run(command: list[str], log: pathlib.Path, timeout: int = 120) -> int:
    startup = subprocess.STARTUPINFO()
    startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    startup.wShowWindow = 0
    with log.open("wb") as output:
        process = subprocess.Popen(command, stdout=output, stderr=subprocess.STDOUT,
                                   startupinfo=startup, creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            return process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"],
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False)
            raise RuntimeError(f"isolated installer command timed out; see {log}")

def prepare(stage: pathlib.Path, scratch: pathlib.Path) -> tuple[pathlib.Path, dict]:
    manifest = json.loads((stage / "PACKAGE.json").read_text(encoding="utf-8"))
    if manifest.get("platform") != "windows" or manifest.get("variant") not in ("core", "skin", "free"):
        raise ValueError("a complete Windows release stage is required")
    for name in ("dsh-launcher.exe", "deepseek-harness-rs.exe"):
        path = stage / name
        if path.stat().st_size < 1024 * 1024 or path.read_bytes()[:2] != b"MZ":
            raise ValueError(f"missing or invalid real release executable: {name}")
    for name in ("web/dist/index.html", "config/agent-presets/standard/agent.cordis.yml"):
        if not (stage / name).is_file():
            raise ValueError(f"incomplete release payload: {name}")
    raw = (ROOT / "packaging/windows/deepseek-harness-rs.iss").read_bytes()
    language = (ROOT / "packaging/windows/ChineseSimplified.isl").read_bytes()
    if not raw.startswith(b"\xef\xbb\xbf") or not language.startswith(b"\xef\xbb\xbf"):
        raise ValueError("Inno 6.1.2 requires UTF-8 BOM for both script and language")
    source = raw.decode("utf-8-sig")
    default = r"DefaultDirName=D:\Program Files (x86)\DeepSeek Harness-rs\{#Variant}"
    if default not in source or "UsePreviousAppDir=yes" not in source:
        raise ValueError("installer destination policy changed")
    for app_id in re.findall(r'#define MyAppId "\{\{([A-F0-9-]+)\}"', source):
        source = source.replace(app_id, str(uuid.uuid4()).upper())
    source = source.replace(default, f"DefaultDirName={scratch / 'installed'}")
    source = source.replace('AppName={#MyAppName}', 'AppName=Isolated installer behavior verification')
    source = re.sub(r'(?m)^OutputBaseFilename=.*$', 'OutputBaseFilename=isolated-verification', source)
    source = re.sub(r'(?ms)^\[Icons\].*?(?=^\[Code\])', '', source)
    source = source.replace('[Setup]', '[Setup]\nCreateUninstallRegKey=no\nUninstallable=no\nCloseApplications=no\nRestartApplications=no\n')
    guard = str(scratch).replace("'", "''").rstrip("\\") + "\\"
    target = "  Directory := ExpandFileName(WizardDirValue);"
    if source.count(target) != 1:
        raise ValueError("cannot install scratch guard: expected exactly one directory assignment")
    source = source.replace(target,
        "  Directory := ExpandFileName(WizardDirValue);\n"
        f"  if Pos(Lowercase('{guard}'), Lowercase(AddBackslash(Directory))) <> 1 then\n"
        "  begin\n    Result := 'Isolated verifier refuses paths outside its scratch directory';\n    Exit;\n  end;")
    expected_guard = f"  if Pos(Lowercase('{guard}'), Lowercase(AddBackslash(Directory))) <> 1 then"
    if source.count(expected_guard) != 1 or "Isolated verifier refuses paths outside its scratch directory" not in source:
        raise ValueError("compiled verifier must include the exact scratch containment guard")
    if any(section in source for section in ("[Run]", "[Icons]", "[Registry]", "[UninstallRun]", "[UninstallDelete]")):
        raise ValueError("verifier must never run the product or create shortcuts")
    for directive in ("CreateUninstallRegKey=no", "Uninstallable=no", "CloseApplications=no", "RestartApplications=no"):
        if source.count(directive) != 1:
            raise ValueError(f"missing verifier isolation directive: {directive}")
    path = scratch / "isolated-verification.iss"
    path.write_text(source, encoding="utf-8-sig")
    return path, manifest

def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--compiler", type=pathlib.Path, required=True)
    parser.add_argument("--stage", type=pathlib.Path, required=True)
    parser.add_argument("--scratch", type=pathlib.Path, required=True)
    args = parser.parse_args()
    if os.name != "nt":
        raise SystemExit("Windows is required")
    scratch = args.scratch.resolve()
    if not scratch.is_absolute() or scratch == pathlib.Path(scratch.anchor) or scratch == ROOT or ROOT in scratch.parents:
        raise SystemExit("scratch must be an independent absolute operation directory")
    if scratch.exists() and any(scratch.iterdir()):
        raise SystemExit("scratch directory must be empty")
    scratch.mkdir(parents=True, exist_ok=True)
    stage = args.stage.resolve()
    script, manifest = prepare(stage, scratch)
    code = run([str(args.compiler.resolve()), f'/DSourceDir={stage}', f'/DOutputDir={scratch}',
                f'/DChineseMessages={ROOT / "packaging/windows/ChineseSimplified.isl"}',
                f'/DIconFile={ROOT / "packaging/windows/deepseek-black.ico"}',
                f'/DVariant={manifest["variant"]}', str(script)], scratch / "compile.log")
    if code:
        raise SystemExit(f"Inno compile failed ({code}); see {scratch / 'compile.log'}")
    require_warning_free_compile(scratch / "compile.log")
    executable = scratch / "isolated-verification.exe"
    missing_code = run([str(args.compiler.resolve()), f'/DSourceDir={scratch / "missing-payload"}', f'/DOutputDir={scratch}',
                        f'/DChineseMessages={ROOT / "packaging/windows/ChineseSimplified.isl"}',
                        f'/DIconFile={ROOT / "packaging/windows/deepseek-black.ico"}',
                        f'/DVariant={manifest["variant"]}', str(script)], scratch / "missing-payload-compile.log")
    if missing_code == 0:
        raise SystemExit("compiler accepted a missing launcher/core payload")
    destination = scratch / "installed"
    code = run([str(executable), "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/NOICONS", "/LANG=chinesesimp",
                f"/DIR={destination}", f"/LOG={scratch / 'install.log'}"], scratch / "install-process.log")
    if code:
        raise SystemExit(f"isolated installation failed ({code}); see install.log")
    for path in stage.rglob("*"):
        if path.is_file() and hashlib.sha256(path.read_bytes()).digest() != hashlib.sha256((destination / path.relative_to(stage)).read_bytes()).digest():
            raise SystemExit(f"installed payload differs: {path.relative_to(stage)}")
    blocked = scratch / "not-a-directory"
    blocked.write_text("directory selection barrier", encoding="utf-8")
    code = run([str(executable), "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/NOICONS", "/LANG=chinesesimp",
                f"/DIR={blocked / 'app'}", f"/LOG={scratch / 'invalid-directory.log'}"], scratch / "invalid-process.log")
    if code == 0 or blocked.read_text(encoding="utf-8") != "directory selection barrier":
        raise SystemExit("invalid installation directory was not rejected safely")
    result = {"installedPayloadVerified": True, "invalidDirectoryRejected": True, "missingPayloadCompileGuards": True,
              "noRegistryOrShortcuts": True, "noPostInstallRun": True, "visualWizardVerified": False,
              "scratch": str(scratch), "stage": str(stage)}
    (scratch / "result.json").write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(result, ensure_ascii=False))

if __name__ == "__main__":
    main()
