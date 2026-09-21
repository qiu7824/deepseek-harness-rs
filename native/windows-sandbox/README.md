# Windows native sandbox

Windows execution uses the Codex-derived native engine in two explicit modes:

| Mode | Identity | Filesystem | Restricted networking |
| --- | --- | --- | --- |
| `elevated` (default) | Dedicated low-privilege account | Project write boundaries and protected host state | Account-scoped Windows Firewall and WFP rules |
| `unelevated` (fallback) | Restricted token derived from the host user | Read access follows the host user; writes remain restricted | Environment-level offline controls; weaker than elevated isolation |

AppContainer is no longer an execution backend. A failed native backend never
silently runs a command without confinement or switches implementation.

## Configuration and initialization

Use **Settings → Windows sandbox** to select the implementation and network
policy, and initialize each writable project. Elevated setup requests Windows
administrator authorization when necessary. Subsequent commands run from an
ordinary host account. Unelevated project permission preparation requires no account provisioning or administrator approval. Both modes require explicit project initialization; large-tree ACL preparation is kept outside command startup budgets.

```powershell
deepseek-harness-rs.exe sandbox status
deepseek-harness-rs.exe sandbox setup --workspace E:\projects\app --implementation elevated --network restricted
deepseek-harness-rs.exe sandbox configure --implementation unelevated --network enabled
```

New configurations default to elevated execution with restricted networking.
Existing configurations retain their previous network policy until changed.
Changes apply to subsequent commands and change the execution fingerprint;
existing task contracts require environment migration and fresh validation.

`windows-sandbox.json` resides in the active data directory. It contains version
1, backend `windows-native`, implementation `elevated` or `unelevated`, network
`enabled` or `restricted`, absolute runner and state paths, and SHA-256 identities
for all three helpers. Legacy `workspaces` remains readable but no longer routes
other projects to AppContainer. Writable accounts remain project-specific.

## Build and distribution

```powershell
cargo build --locked --release --workspace --manifest-path native/windows-sandbox/Cargo.toml --target-dir target/native-windows-sandbox
cargo test --locked --release --manifest-path native/windows-sandbox/Cargo.toml -p codex-windows-sandbox --lib
```

Distribute `dsh-windows-native.exe`, `dsh-command-runner.exe`, and
`dsh-windows-sandbox-setup.exe` together with license, notice and upstream identity.
Helper hashes are checked before dispatch. Setup version 6 includes the trusted
host's ACL-maintenance rights and read-only network-policy access; older setup
state requires explicit initialization before reuse.

## Execution protocol

The bridge accepts `--implementation elevated|unelevated`, `--native-home`,
`--workspace`, `--mode read-only|workspace-write`, `--network enabled|restricted`,
optional read/runtime/temp roots, timeout and terminal flags, and literal argv
after `--`. `--status` checks readiness; `--setup` initializes the selected mode.
Timeouts return 124, setup or transport failures return 125 with authenticated
startup evidence, and ordinary commands retain their exit codes. Child output
cannot impersonate startup failure merely by printing a runner marker.

Both modes use a private desktop and owned process trees. Completion, timeout,
cancellation and IPC loss reclaim descendants. Use the Host background-job
interface for long-running commands instead of detaching untracked processes.

Windows command discovery honors executable suffixes. npm/npx launchers resolve
to Node and the installed JavaScript entry point; arguments never pass through
`cmd.exe`. Other scripts require an explicit interpreter.

## Isolation and lifecycle

Elevated execution reserves two read-only slots and four writable slots per
initialized workspace. Writable identities are not reused for other projects or
read-only execution. Leases, process creation identity and quarantine protect
reuse. Setup requires affected native executions to be idle.

Offline rules have stable account-specific identifiers and cover IPv4/IPv6
socket allocation, connection and receive authorization. Runtime checks reject
missing or disabled filters instead of trusting a stale setup marker. The
unelevated fallback advertises its environment-based network controls.

Host HTTP control access requires the host principal and rejects restricted
tokens, including same-user unelevated children. Credentials remain protected
in the owned state directory. Initialization refuses foreign ownership and
workspace overlap with helper binaries or private state. Publicly readable
host files are not promised to be hidden as in a virtual machine.

## Upstream

The engine derives from OpenAI Codex at the revision recorded in `UPSTREAM.json`
under Apache-2.0. DSH uses separate account names, ownership markers, helper paths,
firewall rules and WFP object keys. Existing Codex identities are not reused.
Upstream metrics exporters are not initialized by the DSH bridge.
