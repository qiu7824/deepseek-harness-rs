"""Validate and publish only artifacts from a successful, tag-matched build."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import urllib.parse
import urllib.request

PLATFORMS = {"windows-x86_64", "linux-x86_64", "macos-x86_64", "macos-aarch64"}


def api(path: str):
    repository = os.environ["GITHUB_REPOSITORY"]
    req = urllib.request.Request(
        f"https://api.github.com/repos/{repository}/{path}",
        headers={"Authorization": "Bearer " + os.environ["GH_TOKEN"],
                 "Accept": "application/vnd.github+json", "User-Agent": "dsh-release-promotion"},
    )
    with urllib.request.urlopen(req, timeout=60) as response:
        return json.load(response)


def validate_run(run: dict, tag_sha: str, artifacts: list[dict]) -> None:
    if (run.get("conclusion") != "success" or run.get("status") != "completed"
            or run.get("path") != ".github/workflows/release.yml"
            or run.get("head_sha") != tag_sha):
        raise ValueError("build must be successful and match the release tag")
    selected = [a for a in artifacts if a["name"].startswith("deepseek-harness-rs-")]
    expected = {"deepseek-harness-rs-" + platform for platform in PLATFORMS}
    if len(selected) != 4 or {a["name"] for a in selected} != expected:
        raise ValueError("all four platform artifacts are required")
    if any(a.get("expired") or a.get("workflow_run", {}).get("head_sha") != tag_sha for a in selected):
        raise ValueError("artifact is expired or belongs to another commit")


def digest(path: Path) -> str:
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def validate_payload(directory: Path, tag: str) -> dict[str, str]:
    if re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", tag) is None:
        raise ValueError("invalid release tag")
    expected = {}
    for platform in sorted(PLATFORMS):
        manifest = directory / f"SHA256SUMS-{platform}.txt"
        rows = {}
        for line in manifest.read_text(encoding="utf-8").splitlines():
            match = re.fullmatch(r"([0-9a-f]{64})  (\S+)", line)
            if match is None:
                raise ValueError("invalid checksum entry")
            sha, name = match.groups()
            prefix = f"deepseek-harness-rs-{tag}-{platform}-"
            if not name.startswith(prefix) or "/" in name or "\\" in name or name in rows:
                raise ValueError("checksum filename is not owned by this platform")
            rows[name] = sha
        from release_variants import expected_artifacts
        variant_names = ["core", "skin", "free"] if any("-free-" in n or "-free." in n for n in rows) else ["core", "skin"]
        if set(rows) != expected_artifacts(f"deepseek-harness-rs-{tag}-{platform}", platform.split("-")[0], variant_names):
            raise ValueError("platform artifact set is incomplete")
        expected.update(rows)
    actual = {p.name for p in directory.iterdir() if p.is_file() and not p.name.startswith("SHA256SUMS")}
    if actual != set(expected):
        raise ValueError("downloaded payload does not match the platform manifests")
    for name, sha in expected.items():
        if digest(directory / name) != sha:
            raise ValueError("artifact checksum mismatch: " + name)
    (directory / "SHA256SUMS.txt").write_text(
        "".join(f"{expected[name]}  {name}\n" for name in sorted(expected)), encoding="utf-8")
    return expected


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=["identity", "payload", "verify"])
    parser.add_argument("--run-id", type=int)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--directory", type=Path, default=Path("dist"))
    args = parser.parse_args()
    if re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", args.tag) is None:
        raise ValueError("invalid release tag")
    if args.mode == "identity":
        run = api(f"actions/runs/{args.run_id}")
        obj = api("git/ref/tags/" + urllib.parse.quote(args.tag, safe=""))["object"]
        while obj["type"] == "tag":
            obj = api("git/tags/" + obj["sha"])["object"]
        if obj["type"] != "commit":
            raise ValueError("tag does not identify a commit")
        artifacts = api(f"actions/runs/{args.run_id}/artifacts?per_page=100")["artifacts"]
        validate_run(run, obj["sha"], artifacts)
        print(json.dumps({"run": args.run_id, "tag": args.tag, "revision": obj["sha"]}))
    else:
        expected = validate_payload(args.directory, args.tag)
        if args.mode == "verify":
            expected["SHA256SUMS.txt"] = digest(args.directory / "SHA256SUMS.txt")
            release = api("releases/tags/" + args.tag)
            remote = {a["name"]: a.get("digest") for a in release["assets"]}
            if release["draft"] or remote != {name: "sha256:" + sha for name, sha in expected.items()}:
                raise ValueError("published asset set or checksum differs")
            notes = Path("release/notes") / (args.tag + ".md")
            if release["body"].strip() != notes.read_text(encoding="utf-8").strip():
                raise ValueError("published release notes differ")
        print(json.dumps({"tag": args.tag, "verifiedFiles": len(expected)}))


if __name__ == "__main__":
    main()
