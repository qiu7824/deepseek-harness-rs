"""Prepare affected Flutter SDKs for UTF-8 paths and recoverable asset builds."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil

BEFORE = """  void _writeMessage({required String message}) {
    _process?.stdin.write('Content-Length: ${message.length}\\r\\n\\r\\n$message');
  }"""
AFTER = """  void _writeMessage({required String message}) {
    final bytes = utf8.encode(message);
    final process = _process;
    if (process == null) return;
    process.stdin.add(utf8.encode('Content-Length: ${bytes.length}\\r\\n\\r\\n'));
    process.stdin.add(bytes);
  }"""

SHADER_PATH_HELPER = """import 'dart:ffi';
import 'dart:io' show Platform;

import 'package:ffi/ffi.dart';
import 'package:path/path.dart' as paths;

typedef _NativeShortPath = Uint32 Function(Pointer<Utf16>, Pointer<Utf16>, Uint32);
typedef _ShortPath = int Function(Pointer<Utf16>, Pointer<Utf16>, int);

/// Read the existing filesystem name accepted by Windows native include readers.
/// This query creates no drive, directory, link or filesystem alias.
String nativeShaderPath(String path) {
  if (!Platform.isWindows || path.codeUnits.every((unit) => unit < 128)) return path;
  final query = DynamicLibrary.open('kernel32.dll')
      .lookupFunction<_NativeShortPath, _ShortPath>('GetShortPathNameW');
  String? existing(String value) {
    final source = value.toNativeUtf16(allocator: calloc);
    try {
      final length = query(source, nullptr, 0);
      if (length == 0 || length > 32768) return null;
      final output = calloc<Uint16>(length).cast<Utf16>();
      try {
        final written = query(source, output, length);
        return written > 0 && written < length ? output.toDartString() : null;
      } finally {
        calloc.free(output);
      }
    } finally {
      calloc.free(source);
    }
  }
  final resolved = existing(path);
  if (resolved != null) return resolved;
  final parent = existing(paths.dirname(path));
  return parent == null ? path : paths.join(parent, paths.basename(path));
}
"""


def prepare(sdk: Path, backup: Path) -> dict:
    sdk = sdk.resolve()
    path = sdk / "packages/flutter_tools/lib/src/dart/analysis.dart"
    original = path.read_bytes()
    text = original.decode("utf-8").replace("\r\n", "\n")
    if AFTER in text:
        return {"changed": False, "utf8Framing": True}
    if text.count(BEFORE) != 1:
        raise ValueError("Flutter analysis transport differs; review UTF-8 framing before patching")
    digest = hashlib.sha256(original).hexdigest()
    backup = backup.resolve() / digest
    backup.mkdir(parents=True, exist_ok=True)
    saved = backup / "analysis.dart"
    if saved.exists() and saved.read_bytes() != original:
        raise ValueError("SDK backup identity mismatch")
    saved.write_bytes(original)
    updated = text.replace(BEFORE, AFTER).encode("utf-8")
    if path.read_bytes() != original:
        raise ValueError("SDK changed while preparing the framing fix")
    path.write_bytes(updated)
    # A missing stamp makes the official launcher rebuild its own tool snapshot.
    # Running clients and their source, preferences and outputs are untouched.
    stamp = sdk / "bin/cache/flutter_tools.stamp"
    if stamp.is_file():
        shutil.copy2(stamp, backup / "flutter_tools.stamp")
        stamp.unlink()
    result = {"changed": True, "utf8Framing": True, "sdk": str(sdk),
              "beforeSha256": digest, "afterSha256": hashlib.sha256(updated).hexdigest(),
              "backup": str(backup)}
    (backup / "repair.json").write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return result


def prepare_shader_paths(sdk: Path, backup: Path) -> dict:
    if os.name != "nt":
        return {"changed": False, "applicable": False}
    sdk = sdk.resolve()
    directory = sdk / "packages/flutter_tools/lib/src/build_system/tools"
    path = directory / "shader_compiler.dart"
    helper = directory / "native_shader_path.dart"
    original = path.read_bytes()
    text = original.decode("utf-8").replace("\r\n", "\n")
    original_text = text
    previous_helper = helper.read_bytes() if helper.exists() else None
    helper_text = None if previous_helper is None else previous_helper.decode("utf-8").replace("\r\n", "\n")
    if previous_helper is not None:
        if helper_text != SHADER_PATH_HELPER and hashlib.sha256(previous_helper).hexdigest() != "3ffac0e0ecfa53985751dbcf096e48dfbdf1e75d671005f73173c16527e1c9d0":
            raise ValueError("Native shader path helper differs from the recorded implementation")
    if "import 'native_shader_path.dart';" not in text:
        if text.count("import '../depfile.dart';") != 1:
            raise ValueError("Flutter shader compiler import layout differs")
        text = text.replace("import '../depfile.dart';", "import '../depfile.dart';\nimport 'native_shader_path.dart';")
    text = text.replace('nativeShaderIncludePath(', 'nativeShaderPath(')
    before = ["'--include=${input.parent.path}'", "'--include=$shaderLibPath'", "'--input=${input.path}'", "'--sl=$outputPath'", "'--spirv=$outputPath.spirv'", "'--depfile=$depfilePath'"]
    after = ["'--include=${nativeShaderPath(input.parent.path)}'", "'--include=${nativeShaderPath(shaderLibPath)}'",
             "'--input=${nativeShaderPath(input.path)}'", "'--sl=${nativeShaderPath(outputPath)}'",
             "'--spirv=${nativeShaderPath('$outputPath.spirv')}'", "'--depfile=${nativeShaderPath(depfilePath)}'"]
    for old, new in zip(before, after):
        if text.count(new) == 1:
            continue
        if text.count(old) != 1:
            raise ValueError("Flutter shader compiler differs; review include path handling before patching")
        text = text.replace(old, new)
    if text == original_text and helper_text == SHADER_PATH_HELPER:
        return {"changed": False, "applicable": True}
    digest = hashlib.sha256(original).hexdigest()
    destination = backup.resolve() / ("shader-" + digest)
    destination.mkdir(parents=True, exist_ok=True)
    (destination / path.name).write_bytes(original)
    if previous_helper is not None:
        (destination / helper.name).write_bytes(previous_helper)
    if path.read_bytes() != original:
        raise ValueError("Shader compiler changed while preparing the path fix")
    helper.write_text(SHADER_PATH_HELPER, encoding="utf-8")
    path.write_text(text, encoding="utf-8")
    stamp = sdk / "bin/cache/flutter_tools.stamp"
    if stamp.is_file():
        shutil.copy2(stamp, destination / "flutter_tools.stamp")
        stamp.unlink()
    result = {"changed": True, "applicable": True, "driveMappingsCreated": False,
              "beforeSha256": digest, "afterSha256": hashlib.sha256(path.read_bytes()).hexdigest(),
              "backup": str(destination)}
    (destination / "repair.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def prepare_asset_recovery(sdk: Path, backup: Path) -> dict:
    path = sdk.resolve() / "packages/flutter_tools/lib/src/commands/test.dart"
    original = path.read_bytes()
    text = original.decode("utf-8").replace("\r\n", "\n")
    marker = "    // Rebuild interrupted bundles even when the manifest was written first."
    if marker in text:
        return {"changed": False}
    anchor = "    final DateTime lastModified = manifest.lastModifiedSync();"
    if text.count(anchor) != 1:
        raise ValueError("Flutter test asset cache differs; review output validation before patching")
    guard = marker + """
    if (entries.keys.any((key) => !globals.fs.file(
      globals.fs.path.join('build', 'unit_test_assets', key),
    ).existsSync())) {
      return true;
    }
"""
    digest = hashlib.sha256(original).hexdigest()
    destination = backup.resolve() / ("assets-" + digest)
    destination.mkdir(parents=True, exist_ok=True)
    (destination / "test.dart").write_bytes(original)
    if path.read_bytes() != original:
        raise ValueError("Flutter test command changed during asset cache preparation")
    path.write_text(text.replace(anchor, guard + anchor), encoding="utf-8")
    stamp = sdk.resolve() / "bin/cache/flutter_tools.stamp"
    if stamp.is_file():
        shutil.copy2(stamp, destination / stamp.name)
        stamp.unlink()
    result = {"changed": True, "beforeSha256": digest,
              "afterSha256": hashlib.sha256(path.read_bytes()).hexdigest(), "backup": str(destination)}
    (destination / "repair.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--backup-dir", type=Path, required=True)
    args = parser.parse_args()
    result = {"analysis": prepare(args.sdk, args.backup_dir),
              "shader": prepare_shader_paths(args.sdk, args.backup_dir),
              "assets": prepare_asset_recovery(args.sdk, args.backup_dir)}
    print(json.dumps(result, ensure_ascii=True))


if __name__ == "__main__":
    main()
