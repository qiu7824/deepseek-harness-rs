from __future__ import annotations

import argparse
import pathlib
import tempfile
import zipfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
SKINS = ROOT / "web" / "dist" / "skins"
MARKER = b"\n__DSH_SKIN_PAYLOAD_V1_4F92C3A7__\n"


def build_skin_payload(stub: pathlib.Path, output: pathlib.Path) -> None:
    if not SKINS.is_dir():
        raise SystemExit(f"missing skin source tree: {SKINS}")
    if not stub.is_file():
        raise SystemExit(f"missing native skin installer stub: {stub}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="dsh-skin-payload-") as temporary:
        payload = pathlib.Path(temporary) / "skin_payload.zip"
        with zipfile.ZipFile(payload, "w", zipfile.ZIP_DEFLATED) as archive:
            for file in sorted(SKINS.rglob("*")):
                if file.is_file():
                    archive.write(file, pathlib.Path("skins") / file.relative_to(SKINS))
        output.write_bytes(stub.read_bytes() + MARKER + payload.read_bytes())


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stub", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    build_skin_payload(pathlib.Path(args.stub), pathlib.Path(args.output))


if __name__ == "__main__":
    main()
