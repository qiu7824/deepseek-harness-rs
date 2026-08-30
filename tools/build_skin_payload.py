from __future__ import annotations

import argparse
import pathlib
import stat
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
SKINS = ROOT / "web" / "dist" / "skins"
MARKER = b"\n__DSH_SKIN_PAYLOAD__\n"
SCRIPT = r'''#!/usr/bin/env python3
import pathlib, sys, zipfile
payload = pathlib.Path(sys.argv[0]).read_bytes()
marker = b'\n__DSH_SKIN_PAYLOAD__\n'
pos = payload.rfind(marker)
if pos < 0:
    raise SystemExit('invalid skin installer payload')
archive = pathlib.Path(sys.argv[0]).with_suffix('.payload.zip')
archive.write_bytes(payload[pos + len(marker):])
target = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else pathlib.Path(__file__).resolve().parent / 'web' / 'dist'
with zipfile.ZipFile(archive) as package:
    package.extractall(target)
archive.unlink(missing_ok=True)
print(f'Skins installed to {target / "skins"}')
'''


def build_skin_payload(output: pathlib.Path) -> None:
    if not SKINS.is_dir():
        raise SystemExit(f"missing skin source tree: {SKINS}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="dsh-skin-payload-") as temporary:
        payload = pathlib.Path(temporary) / "skin_payload.zip"
        with zipfile.ZipFile(payload, "w", zipfile.ZIP_DEFLATED) as archive:
            for file in sorted(SKINS.rglob("*")):
                if file.is_file():
                    archive.write(file, pathlib.Path("skins") / file.relative_to(SKINS))
        output.write_bytes(SCRIPT.encode("utf-8") + MARKER + payload.read_bytes())
        output.chmod(output.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    build_skin_payload(pathlib.Path(args.output))


if __name__ == "__main__":
    main()
