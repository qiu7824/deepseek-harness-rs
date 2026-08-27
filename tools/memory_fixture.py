"""Synthetic dense session fixtures for memory acceptance tests."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sys
from collections.abc import Iterable, Iterator


MAX_FIXTURE_EVENTS = 1_000_000


def generate_events(
    *, total_events: int = 68_000, message_groups: int = 40
) -> Iterator[dict[str, object]]:
    """Yield a deterministic dense stream without retaining the full fixture."""
    if message_groups <= 0:
        raise ValueError("message_groups must be positive")
    boundary_events = message_groups * 2
    if total_events < boundary_events or total_events > MAX_FIXTURE_EVENTS:
        raise ValueError(
            f"total_events must be between {boundary_events} and {MAX_FIXTURE_EVENTS}"
        )

    delta_events = total_events - boundary_events
    per_group, remainder = divmod(delta_events, message_groups)
    seq = 0
    for group in range(message_groups):
        yield {
            "seq": seq,
            "type": "user/message",
            "data": {"content": f"synthetic-user-{group:04d}"},
        }
        seq += 1
        group_deltas = per_group + int(group < remainder)
        for delta in range(group_deltas):
            yield {
                "seq": seq,
                "type": "assistant/reasoning-delta",
                "data": {"content": f"synthetic-delta-{group:04d}-{delta:06d}"},
            }
            seq += 1
        yield {
            "seq": seq,
            "type": "assistant/message",
            "data": {"content": f"synthetic-assistant-{group:04d}"},
        }
        seq += 1


def write_jsonl(
    output: pathlib.Path, events: Iterable[dict[str, object]]
) -> dict[str, int]:
    output.parent.mkdir(parents=True, exist_ok=True)
    count = 0
    total_bytes = 0
    digest = hashlib.sha256()
    with output.open("wb") as stream:
        for event in events:
            line = json.dumps(
                event, ensure_ascii=False, separators=(",", ":"), sort_keys=True
            ).encode("utf-8") + b"\n"
            stream.write(line)
            digest.update(line)
            count += 1
            total_bytes += len(line)
    return {"events": count, "bytes": total_bytes, "sha256": digest.hexdigest()}


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Generate a synthetic dense event fixture")
    parser.add_argument("--output", required=True)
    parser.add_argument("--events", type=int, default=68_000)
    parser.add_argument("--message-groups", type=int, default=40)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    output = pathlib.Path(args.output).resolve()
    summary = write_jsonl(
        output,
        generate_events(total_events=args.events, message_groups=args.message_groups),
    )
    print(json.dumps(summary, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
