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
    boundary_events = message_groups * 10
    if total_events < boundary_events or total_events > MAX_FIXTURE_EVENTS:
        raise ValueError(
            f"total_events must be between {boundary_events} and {MAX_FIXTURE_EVENTS}"
        )

    delta_events = total_events - boundary_events
    per_group, remainder = divmod(delta_events, message_groups)
    seq = 0
    base_time = 1_700_000_000_000
    def event(kind, data, **extra):
        nonlocal seq
        result = {"seq": seq, "time": base_time + seq, "type": kind, "data": data, **extra}
        seq += 1
        return result

    for group in range(message_groups):
        turn = group + 1
        position = {"turn": turn, "step": 1}
        yield event("turn/start", {"turn": turn})
        yield event("user/message", {
            "id": f"memory-user-{group:04d}", "role": "user",
            "content": [{"type": "text", "text": f"synthetic-user-{group:04d}"}],
            "source": {"kind": "user", "rpcId": f"memory-request-{group:04d}"},
        }, surfaceOp="append")
        yield event("step/start", position)
        chunk_start = seq
        yield event("assistant/chunk", {**position, "chunk": {"type": "block-start", "index": 0, "blockType": "reasoning"}})
        group_deltas = per_group + int(group < remainder)
        fragments = []
        for delta in range(group_deltas):
            fragment = f"synthetic-delta-{group:04d}-{delta:06d}\n"
            fragments.append(fragment)
            yield event("assistant/chunk", {**position, "chunk": {"type": "reasoning-delta", "index": 0, "text": fragment}})
        reasoning = {"type": "reasoning", "text": "".join(fragments)}
        yield event("assistant/chunk", {**position, "chunk": {"type": "block-end", "index": 0, "block": reasoning}})
        yield event("assistant/chunk", {**position, "chunk": {"type": "usage", "usage": {"inputTokens":4096, "cacheReadTokens":4096, "outputTokens":128, "reasoningTokens":16}}})
        yield event("assistant/chunk", {**position, "chunk": {"type": "finish", "reason": {"kind": "stop"}}})
        sources = list(range(chunk_start, seq))
        yield event("assistant/message", {**position, "message": {
            "id": f"memory-assistant-{group:04d}", "role": "assistant",
            "content": [reasoning, {"type": "text", "text": f"synthetic-assistant-{group:04d}"}],
            "source": {"kind": "model", "provider": "memory-fixture", "model": "fixture"},
        }}, surfaceOp="append", sourceEventSeqs=sources)
        yield event("step/end", position)
        yield event("turn/end", {"turn": turn, "reason": {"kind": "completed"}})


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
