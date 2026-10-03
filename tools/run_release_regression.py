"""Stream a release check and expose its bounded failure tail to Actions."""
from __future__ import annotations

import argparse
import codecs
from pathlib import Path
import subprocess
import sys


TAIL_LINES = 60
ANNOTATION_CHARS = 4000


def escape_annotation(value: str) -> str:
    return value.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def run(command: list[str], log_path: Path) -> int:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    tail = ""
    decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
    with log_path.open("w", encoding="utf-8", errors="replace", newline="") as log:
        with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as child:
            assert child.stdout is not None

            def emit(value: str) -> None:
                nonlocal tail
                if not value:
                    return
                sys.stdout.write(value)
                sys.stdout.flush()
                log.write(value)
                log.flush()
                tail = (tail + value)[-ANNOTATION_CHARS:]

            while chunk := child.stdout.read1(4096):
                emit(decoder.decode(chunk))
            emit(decoder.decode(b"", final=True))
            returncode = child.wait()

    if returncode:
        prefix = f"Command exited with code {returncode}.\n"
        excerpt = "".join(tail.splitlines(keepends=True)[-TAIL_LINES:])
        message = prefix + excerpt[-(ANNOTATION_CHARS - len(prefix)):]
        if tail and not tail.endswith(("\r", "\n")):
            print()
        print("::error title=Host regression::" + escape_annotation(message), flush=True)
    return returncode


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("a child command is required after --")
    sys.stdout.reconfigure(encoding="utf-8", errors="replace", newline="")
    return run(command, args.log)


if __name__ == "__main__":
    raise SystemExit(main())
