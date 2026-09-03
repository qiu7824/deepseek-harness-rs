from __future__ import annotations

import argparse
import json
import urllib.request


DEFAULT_CATALOG_URL = "https://opencode.ai/zen/v1/models"
DEFAULT_MODEL_ID = "mimo-v2.5-free"


def fetch_model_ids(url: str, timeout: float = 20.0) -> set[str]:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/json",
            "User-Agent": "deepseek-harness-rs-release-verifier",
        },
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        payload = json.load(response)
    rows = payload.get("data")
    if not isinstance(rows, list):
        raise ValueError("model catalog response has no data list")
    ids = {
        row.get("id")
        for row in rows
        if isinstance(row, dict) and isinstance(row.get("id"), str)
    }
    if not ids:
        raise ValueError("model catalog response has no model ids")
    return ids


def verify(model_id: str, url: str = DEFAULT_CATALOG_URL) -> None:
    ids = fetch_model_ids(url)
    if model_id not in ids:
        raise ValueError(f"free model {model_id!r} is absent from {url}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default=DEFAULT_MODEL_ID)
    parser.add_argument("--url", default=DEFAULT_CATALOG_URL)
    args = parser.parse_args()
    verify(args.model, args.url)
    print(json.dumps({"url": args.url, "model": args.model, "available": True}))


if __name__ == "__main__":
    main()
