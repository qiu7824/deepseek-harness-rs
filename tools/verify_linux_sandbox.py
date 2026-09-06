"""Probe Linux filesystem confinement, optionally preparing a hosted CI profile."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import urllib.request

PROFILE_URL = "https://gitlab.com/apparmor/apparmor/-/raw/v4.0.3/profiles/apparmor/profiles/extras/bwrap-userns-restrict"
PROFILE_SHA256 = "a964037f6cf0df1099f14226b037eaedde6237c86e715188e93eb460b30be859"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--workdir", type=pathlib.Path, required=True)
    parser.add_argument("--prepare-ci-profile", action="store_true")
    args = parser.parse_args()
    binary = shutil.which("bwrap")
    if not binary:
        raise SystemExit("Install the distribution's bubblewrap package to run confined commands")
    workdir = args.workdir.resolve()
    workdir.mkdir(parents=True, exist_ok=True)
    restriction = pathlib.Path("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
    before = restriction.read_text().strip() if restriction.exists() else None
    base = [binary, "--ro-bind", "/", "/", "--dev", "/dev", "--proc", "/proc", "--die-with-parent"]

    def run(command):
        return subprocess.run(command, capture_output=True, text=True, timeout=30)

    probe = run(base + ["--", "/bin/true"])
    prepared = False
    if probe.returncode and args.prepare_ci_profile:
        if os.environ.get("GITHUB_ACTIONS") != "true" or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted":
            raise SystemExit("Automatic profile preparation is limited to explicitly selected GitHub-hosted CI")
        if "setting up uid map: Permission denied" not in probe.stderr:
            raise SystemExit("Sandbox preflight failed: " + probe.stderr[-4000:])
        request = urllib.request.Request(PROFILE_URL, headers={"User-Agent": "dsh-sandbox-validation"})
        with urllib.request.urlopen(request, timeout=30) as response:
            data = response.read(8193)
        if len(data) > 8192 or hashlib.sha256(data).hexdigest() != PROFILE_SHA256:
            raise SystemExit("AppArmor profile does not match the pinned upstream content")
        # The upstream profile strips capabilities from bwrap's children.
        # This is an ephemeral CI prerequisite, not an installer-side policy change.
        profile = workdir / "bwrap-userns-restrict"
        profile.write_bytes(data)
        loaded = run(["sudo", "-n", "apparmor_parser", "-r", str(profile)])
        if loaded.returncode:
            raise SystemExit("Cannot load the scoped bwrap profile: " + loaded.stderr[-4000:])
        prepared = True
        probe = run(base + ["--", "/bin/true"])
    if probe.returncode:
        raise SystemExit("Sandbox preflight failed: " + probe.stderr[-4000:])

    with tempfile.TemporaryDirectory(prefix="confinement-", dir=str(workdir)) as temporary:
        root = pathlib.Path(temporary).resolve()
        assert root.parent == workdir
        workspace = root / "workspace"
        workspace.mkdir()
        protected = root / "protected.txt"
        protected.write_text("protected", encoding="utf-8")
        script = 'printf allowed > allowed.txt; if (printf blocked > "$1") 2>/dev/null; then exit 91; fi'
        check = run(base + ["--bind", str(workspace), str(workspace), "--chdir", str(workspace), "--", "/bin/sh", "-c", script, "sh", str(protected)])
        if check.returncode or protected.read_text() != "protected" or (workspace / "allowed.txt").read_text() != "allowed":
            raise SystemExit("Filesystem confinement did not preserve its boundary: " + check.stderr[-4000:])
    after = restriction.read_text().strip() if restriction.exists() else None
    if after != before:
        raise SystemExit("The global user namespace restriction changed during validation")
    evidence = {"filesystemConfinement": True, "profilePrepared": prepared, "globalUserNamespaceRestriction": after, "profileSha256": PROFILE_SHA256 if prepared else None}
    (workdir / "evidence.json").write_text(json.dumps(evidence, indent=2), encoding="utf-8")
    print(json.dumps(evidence))


if __name__ == "__main__":
    main()
