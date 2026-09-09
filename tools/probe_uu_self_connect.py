"""Bounded, account-matched UU self-connect diagnostic; never sends desktop input."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import time


class ProbeError(RuntimeError):
    pass


def valid_id(value):
    return isinstance(value, str) and re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,255}", value) is not None


def local_identity(path, expected_user):
    if not expected_user or path.stat().st_size > 1024 * 1024:
        raise ProbeError("LOCAL_IDENTITY_UNAVAILABLE")
    general, fields = False, {}
    for line in path.read_text(encoding="utf-8-sig").splitlines():
        line = line.strip()
        if line.startswith("["):
            general = line == "[General]"
        elif general and "=" in line:
            name, value = line.split("=", 1)
            name = name.strip()
            if name not in ("userId", "deviceId"):
                continue
            if name in fields:
                raise ProbeError("AMBIGUOUS_LOCAL_IDENTITY")
            fields[name] = value.strip().strip('"')
    if fields.get("userId") != str(expected_user):
        raise ProbeError("LOCAL_ACCOUNT_MISMATCH")
    if not valid_id(fields.get("deviceId")):
        raise ProbeError("LOCAL_IDENTITY_UNAVAILABLE")
    return fields["deviceId"]


def connection_ids(data):
    rows = data.get("connected_devices")
    if not isinstance(rows, list) or len(rows) > 256:
        raise ProbeError("INVALID_CONNECTION_STATUS")
    result = set()
    for row in rows:
        device = row if isinstance(row, str) else next(
            (row.get(key) for key in ("deviceId", "targetId", "device_id") if row.get(key)), None
        ) if isinstance(row, dict) else None
        if not valid_id(device):
            raise ProbeError("INVALID_CONNECTION_STATUS")
        result.add(device)
    return result


def receipt_has_target(data, target):
    rows = data.get("devices")
    return isinstance(rows, list) and any(
        isinstance(row, dict) and row.get("success") is not False
        and (row.get("targetId") or row.get("deviceId") or row.get("device_id")) == target
        for row in rows
    )


def cli_call(cli, arguments):
    try:
        child = subprocess.run(
            [str(cli), *arguments], capture_output=True, timeout=12,
            creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
        )
    except subprocess.TimeoutExpired:
        raise ProbeError("CLI_TIMEOUT") from None
    except OSError:
        raise ProbeError("CLI_UNAVAILABLE") from None
    if len(child.stdout) > 1024 * 1024 or len(child.stderr) > 8192:
        raise ProbeError("CLI_OUTPUT_LIMIT")
    if child.returncode:
        raise ProbeError("CLI_EXIT_" + str(child.returncode))
    try:
        envelope = json.loads(child.stdout.decode("utf-8-sig"))
    except (ValueError, UnicodeError):
        raise ProbeError("CLI_INVALID_JSON") from None
    if not isinstance(envelope, dict) or envelope.get("success") is not True:
        raise ProbeError("CLI_OPERATION_REJECTED")
    if not isinstance(envelope.get("data"), dict):
        raise ProbeError("CLI_INVALID_DATA")
    return envelope["data"]


def probe_connection(target, call, sleep=time.sleep, samples=6):
    """Preserve existing connections; a successful CLI envelope is not connection proof."""
    before = connection_ids(call(["device", "status"]))
    result = {"status": "not-established", "desktopInputSent": False,
              "mediaVerification": "not-run", "terminalExecution": "not-run",
              "beforeConnectionCount": len(before), "statusSamples": []}
    if target in before:
        result.update(status="already-connected-unverified", cleanup="existing-connection-preserved")
        return result
    try:
        receipt = call(["device", "connect", target])
        result["receiptNamesTarget"] = receipt_has_target(receipt, target)
        for _ in range(samples):
            sleep(0.5)
            connected = connection_ids(call(["device", "status"]))
            present = target in connected
            result["statusSamples"].append(present)
            if present:
                result["status"] = "connected-media-unverified"
                break
    except ProbeError as error:
        result.update(status="error", error=str(error))
    finally:
        # This probe owns only the newly requested local target. A disconnect
        # also cancels a pending request that has not appeared in status yet.
        try:
            call(["device", "disconnect", target])
            after = connection_ids(call(["device", "status"]))
            result["cleanup"] = "confirmed" if target not in after else "pending"
            result["existingConnectionsPreserved"] = before.issubset(after)
        except ProbeError as error:
            result.update(cleanup="unverified", cleanupError=str(error))
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--cli", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--probe-connection", action="store_true", help="Request the account-matched local target, then clean up only that request")
    args = parser.parse_args()
    report = {"schemaVersion": 1, "globalSupportConclusion": "undetermined",
              "webSimulationProvesUUConnection": False, "desktopInputSent": False}
    try:
        if not args.cli.is_absolute() or args.cli.name.lower() != "uuyc-cli.exe" or not args.cli.is_file():
            raise ProbeError("INVALID_CLI_PATH")
        call = lambda words: cli_call(args.cli, words)
        user = call(["user", "info"])
        identity = user.get("userId")
        if isinstance(identity, bool) or not isinstance(identity, (str, int)):
            raise ProbeError("ACCOUNT_UNAVAILABLE")
        path = Path(os.environ["ProgramData"]) / "Netease/GameViewer/user_info.ini"
        local = local_identity(path, identity)
        rows = call(["device", "list"]).get("devices")
        if not isinstance(rows, list) or len(rows) > 256:
            raise ProbeError("INVALID_DEVICE_LIST")
        own = [row for row in rows if isinstance(row, dict) and row.get("deviceId") == local]
        report.update(accountMatchesLocalIdentity=True,
                      localIdentityHash=hashlib.sha256(local.encode()).hexdigest()[:16],
                      deviceCount=len(rows), localListed=bool(own),
                      localOnline=any(row.get("isOnline") is True for row in own))
        if args.probe_connection:
            report["connection"] = probe_connection(local, call)
        else:
            report["connection"] = {"status": "not-run"}
    except (ProbeError, OSError, UnicodeError, KeyError) as error:
        report["error"] = str(error) if isinstance(error, ProbeError) else type(error).__name__
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(report, ensure_ascii=True))
    return 1 if "error" in report or report.get("connection", {}).get("status") == "error" else 0


if __name__ == "__main__":
    raise SystemExit(main())
