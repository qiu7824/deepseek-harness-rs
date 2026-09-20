# Windows native sandbox

This standalone Rust workspace provides the DSH native Windows process backend.
The primary application communicates with the bridge through literal argv and
standard streams. The bridge uses authenticated IPC to a separate local-account
command runner. Setup is explicit; an uninitialized backend does not dispatch commands.

## Build

```powershell
cargo build --locked --release --workspace --manifest-path native/windows-sandbox/Cargo.toml --target-dir target/native-windows-sandbox
cargo test --locked -p dsh-windows-native --manifest-path native/windows-sandbox/Cargo.toml --target-dir target/native-windows-sandbox
```

Distribute `dsh-windows-native.exe`, `dsh-command-runner.exe`, and
`dsh-windows-sandbox-setup.exe` together, with `engine/LICENSE`, `engine/NOTICE`,
and `UPSTREAM.json`. Initial setup requests elevation explicitly; the helper's
asInvoker manifest also permits non-elevated workspace refreshes. Normal
commands execute under a restricted sandbox account.

Bare Windows command names follow PATHEXT; extensionless POSIX wrappers are not
selected ahead of Windows launchers. npm/npx wrappers resolve to the matching
installed npm JavaScript entry point and node.exe, preserving literal argv
without cmd.exe interpolation. Other scripts require an explicit interpreter.

## Protocol

`--status --native-home <absolute-directory>` returns readiness without exposing
credentials. `--setup --native-home <directory> --workspace <project>` performs
explicit provisioning. Runtime reads may be supplied as `--runtime-root` or
`--read-root`. Run a command using `--mode read-only|workspace-write`, a workspace,
an optional command timeout in milliseconds, and `-- <program> <literal args>`.
`--tty` requests ConPTY; a console attached to the bridge is also detected.
`--network enabled` preserves normal networking. Requests for `restricted`
networking currently fail closed before dispatch because the offline network
acceptance test has not passed on the validated host. Provisioned firewall
objects alone are not treated as evidence of network isolation.

The bridge returns the child's exit code. Runner-owned timeouts return 124;
setup/transport failures return 125 with `DSH_NATIVE_SANDBOX_FAILED` evidence.
Missing setup never enables unconfined execution.

The host loads `windows-sandbox.json` from its data directory only at startup.
It requires version 1, backend `windows-native`, an absolute runner path and
stateDirectory, and SHA-256 identities for the bridge (`sha256`), command runner
(`commandRunnerSha256`) and setup helper (`setupSha256`). Remove this explicit
selection and restart to use the existing AppContainer backend. An optional
`workspaces` array explicitly limits native routing to the named project roots;
other projects continue using AppContainer. A failure in a selected native
workspace never falls back to unconfined execution.

The pool has two read-only account slots and four write-capable slots per
initialized workspace. Write accounts are never reused for another workspace
or read-only execution. An exclusive lease and a recorded runner process
lifetime fence account reuse. Helpers start suspended, receive a broker-only
process DACL, and are resumed before any untrusted command is dispatched.
Full slots return `NATIVE_SLOT_BUSY`. An interrupted startup with unverifiable
process identity is quarantined instead of reusing the account.

Initialize each writable workspace explicitly. Setup requires all existing
native executions to be idle. It does not occur automatically in a command's
failure/retry path. Read-only slots and writable workspace slots use different
OS principals. Credentials and account ownership are bound to their canonical
state path and the initializing user's SID.

## Upstream and local changes

The engine is derived from OpenAI Codex at the immutable revision recorded in
`UPSTREAM.json`, under Apache-2.0. Dependency versions are locked. DSH changes
product account names and ownership markers, group and helper names, firewall
rules and WFP keys; no existing Codex identities are used. Account/group
collisions without DSH ownership markers are rejected before password changes.
Executable helper copies are placed in a state-owner-scoped ProgramData directory;
DPAPI credentials remain in the protected state directory. Identity is resolved
from the OS token SID, not an environment display name. Offline identities use
WFP connection block definitions covering IPv4/IPv6; offline dispatch remains
disabled until actual connection-denial acceptance passes.
Upstream metrics exporters are not initialized and setup metrics settings are
not forwarded. Command lifetime includes descendants and IPC disconnect cleanup.
The DSH bridge and execution policy adapter are maintained separately.

Pool state belongs to the OS principal that initialized it; another owner
receives an ownership error rather than resetting existing credentials.
Use a stable ProgramData state path and the canonical path returned by setup;
MSIX launcher AppData redirection is not a persistent installation location.
Local-account isolation is not a VM and does not promise that all
publicly readable host paths are hidden. Read/write and confidentiality claims
must match the accepted policy and the negative-test evidence.

Offline networking is unavailable in this version. Standard-user host launch
still needs validation in a genuine non-administrator Windows login; a lowered
token in the development desktop failed before the bridge initialized. Do not
equate that incomplete check with a successful standard-user acceptance test.
