"""Stream Flutter's machine reporter and annotate the actual failed tests."""
from __future__ import annotations

import argparse
import codecs
import json
import subprocess
import sys


ANNOTATION_CHARS = 4000
MAX_FAILURE_ANNOTATIONS = 20


def escape_annotation(value: str) -> str:
    return value.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


class Failures:
    def __init__(self) -> None:
        self.names: dict[int, str] = {}
        self.errors: dict[int | None, list[str]] = {}
        self.failed: dict[int | None, None] = {}

    def consume(self, line: str) -> None:
        try:
            event = json.loads(line)
        except (ValueError, TypeError):
            return
        if not isinstance(event, dict):
            return
        kind = event.get("type")
        if kind == "testStart":
            test = event.get("test")
            if isinstance(test, dict) and isinstance(test.get("id"), int):
                self.names[test["id"]] = str(test.get("name", "unnamed Flutter test"))
        elif kind == "error":
            test_id = event.get("testID")
            if not isinstance(test_id, int):
                test_id = None
            self.failed[test_id] = None
            message = str(event.get("error", "Unknown Flutter test error"))
            stack = event.get("stackTrace")
            if stack:
                message += "\n" + str(stack)
            # Retain the beginning of each error, independently of later passing tests.
            messages = self.errors.setdefault(test_id, [])
            if len(messages) < 4:
                messages.append(message[:ANNOTATION_CHARS])
        elif kind == "testDone" and event.get("result") in ("error", "failure"):
            test_id = event.get("testID")
            self.failed[test_id if isinstance(test_id, int) else None] = None

    def annotate(self, returncode: int) -> None:
        if not returncode:
            return
        if not self.failed:
            print(
                "::error title=Flutter test::"
                + escape_annotation(f"Flutter test exited with code {returncode}; see the full machine log."),
                flush=True,
            )
            return
        for test_id in list(self.failed)[:MAX_FAILURE_ANNOTATIONS]:
            name = self.names.get(test_id, f"Flutter test ID {test_id}" if test_id is not None else "Flutter test runner")
            detail = "\n".join(self.errors.get(test_id, ["The machine reporter marked this test as failed."]))
            message = (name + "\n" + detail)[:ANNOTATION_CHARS]
            print("::error title=Flutter test failure::" + escape_annotation(message), flush=True)
        remaining = len(self.failed) - MAX_FAILURE_ANNOTATIONS
        if remaining > 0:
            print(
                "::error title=Flutter test failure::"
                + escape_annotation(f"{remaining} additional failing tests are recorded in the full machine log."),
                flush=True,
            )


def run(command: list[str]) -> int:
    failures = Failures()
    decoder = codecs.getincrementaldecoder("utf-8")(errors="replace")
    pending = ""
    ends_with_newline = True
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as child:
        assert child.stdout is not None

        def emit(value: str) -> None:
            nonlocal pending, ends_with_newline
            if not value:
                return
            sys.stdout.write(value)
            sys.stdout.flush()
            ends_with_newline = value.endswith(("\r", "\n"))
            pending += value
            lines = pending.split("\n")
            pending = lines.pop()
            for line in lines:
                failures.consume(line)

        while chunk := child.stdout.read1(4096):
            emit(decoder.decode(chunk))
        emit(decoder.decode(b"", final=True))
        if pending:
            failures.consume(pending)
        returncode = child.wait()
    if returncode and not ends_with_newline:
        print()
    failures.annotate(returncode)
    return returncode


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("a Flutter machine-reporting command is required after --")
    sys.stdout.reconfigure(encoding="utf-8", errors="replace", newline="")
    return run(command)


if __name__ == "__main__":
    raise SystemExit(main())
