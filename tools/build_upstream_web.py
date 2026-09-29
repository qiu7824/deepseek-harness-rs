"""Build an upstream DeepSeek Harness Node release's Web client from source.

The Rust Web client tracks the upstream React client. This script turns an
extracted upstream source release into built artifacts outside this
repository and records the Typert Remote contract the client expects from the
Host, so a port can compare it with the Rust gateway:

    python tools/build_upstream_web.py --source D:/deepseek-harness-node-v0.2.0-rc.1

Steps (each skipped when its output already exists, unless --fresh):
  1. copy the source into --work (symlinks the platform cannot create are
     copied as files or skipped when dangling);
  2. commit the copy to a local Git repository, because the client build
     records `git rev-parse HEAD`;
  3. install with the pnpm version pinned by `packageManager`, via corepack;
  4. compile the Host face (generates `lib/typert.remote-client.d.ts`), the
     client libraries and the Web shell. The Electron desktop bundle is not
     built: the Rust product ships its own desktop client.
  5. write the Remote contract inventory to --contract.
"""
from __future__ import annotations

import argparse
import json
import pathlib
import re
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
RPC_MAP = ROOT / "crates" / "host" / "apiproxy" / "src" / "api" / "rpc_map.rs"
ENTRY = re.compile(r"^\s*'([a-zA-Z$]+)/([a-zA-Z]+)':\s*\((.*?)\)\s*=>\s*(.+)$")
# Rust dotted methods that already serve an upstream namespace under an older name.
ALIASES = {"goals": ["goal"], "agentPresets": ["agentPreset"], "subagents": ["subagent"], "skills": ["capabilities", "skill"]}


def run(args: list[str], cwd: pathlib.Path) -> None:
    print("+", " ".join(args), flush=True)
    subprocess.run(args, cwd=cwd, check=True, shell=sys.platform == "win32")


def copy_source(source: pathlib.Path, work: pathlib.Path) -> None:
    ignore = shutil.ignore_patterns("node_modules", ".git")
    shutil.copytree(source, work, symlinks=False, ignore=ignore, ignore_dangling_symlinks=True)


def commit_snapshot(work: pathlib.Path, tag: str) -> None:
    if (work / ".git").is_dir():
        return
    run(["git", "init", "-q"], work)
    run(["git", "-c", "core.autocrlf=false", "add", "-A"], work)
    run(["git", "-c", "user.name=upstream-build", "-c", "user.email=upstream-build@localhost",
         "commit", "-qm", f"{tag} source snapshot"], work)


def pnpm(work: pathlib.Path) -> list[str]:
    manager = json.loads((work / "package.json").read_text(encoding="utf-8"))["packageManager"]
    return ["corepack", manager]


def build(work: pathlib.Path) -> None:
    tool = pnpm(work)
    if not (work / "node_modules").is_dir():
        run(tool + ["install", "--frozen-lockfile"], work)
    run(["node", "--max-old-space-size=4096", "./node_modules/typescript/bin/tsc", "-b", "tsconfig.host.json"], work)
    run(tool + ["exec", "tsdown", "--env.DSH_BUILD_FACE", "host"], work)
    run(tool + ["run", "build:lib:client"], work)
    run(tool + ["run", "build:web"], work)


def contract(work: pathlib.Path) -> list[dict[str, object]]:
    rust_text = RPC_MAP.read_text(encoding="utf-8")
    rust = set(re.findall(r'"([a-zA-Z]+\.[a-zA-Z.]+)"', rust_text))
    rows = []
    files = sorted(work.glob("packages/*/*/lib/typert.remote-client.d.ts")) + sorted(work.glob("apps/*/lib/typert.remote-client.d.ts"))
    for path in files:
        in_map = False
        for line in path.read_text(encoding="utf-8").splitlines():
            if "interface TypertRemoteMap" in line:
                in_map = True
                continue
            if in_map and line.strip() == "}":
                in_map = False
            match = ENTRY.match(line) if in_map else None
            if match is None:
                continue
            namespace, method, params, returns = match.groups()
            names = [namespace] + ALIASES.get(namespace, [])
            rows.append({
                "endpoint": f"{namespace}/{method}",
                "params": [p.strip() for p in re.split(r",\s*(?![^<]*>)", params) if p.strip() and not p.strip().startswith("signal")],
                "returns": returns.strip().rstrip(";"),
                "stream": "RemoteStreamHandle" in returns,
                "rustEquivalent": sorted(r for r in rust if r.split(".", 1)[0] in names and r.split(".", 1)[1].lower() == method.lower()),
                "package": path.relative_to(work).parts[0] + "/" + "/".join(path.relative_to(work).parts[1:-2]),
            })
    return sorted(rows, key=lambda row: row["endpoint"])


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", type=pathlib.Path, required=True, help="extracted upstream source release")
    parser.add_argument("--work", type=pathlib.Path, help="build directory outside this repository")
    parser.add_argument("--contract", type=pathlib.Path, help="Remote contract inventory JSON to write")
    parser.add_argument("--fresh", action="store_true", help="delete --work and rebuild from --source")
    args = parser.parse_args()

    source = args.source.resolve()
    tag = "dsh-v" + json.loads((source / "package.json").read_text(encoding="utf-8"))["version"]
    work = (args.work or ROOT.parent / "upstream" / tag).resolve()
    if ROOT in work.parents or work == ROOT:
        raise SystemExit("--work must be outside this repository")
    if args.fresh and work.exists():
        shutil.rmtree(work)
    if not work.exists():
        copy_source(source, work)
    commit_snapshot(work, tag)
    build(work)

    rows = contract(work)
    target = args.contract or ROOT / "docs" / "upstream" / f"remote-contract-{tag}.json"
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(json.dumps({"release": tag, "endpoints": rows}, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    streams = sum(1 for row in rows if row["stream"])
    matched = sum(1 for row in rows if row["rustEquivalent"])
    print(f"{tag}: {len(rows)} Remote endpoints ({streams} streams), {matched} with a same-named Rust method")
    print(f"web shell: {work / 'apps' / 'web' / 'dist'}")
    print(f"contract: {target}")


if __name__ == "__main__":
    main()
