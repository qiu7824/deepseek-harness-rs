# DeepSeek Harness Rust

[简体中文](README.md) | [English](README.en.md)

DeepSeek Harness Rust is a Rust migration of the DeepSeek Harness Host. It serves the browser application through the production `dsh web` entry point while preserving session, tool, plugin, storage, and RPC compatibility boundaries.

> This project is a prerelease. Treat the compatibility matrix and each GitHub Release note as the authoritative status.

Current release line: [`0.1.3-alpha.38-r6`](https://github.com/qiu7824/deepseek-harness-rs/releases/tag/v0.1.3-alpha.38-r6). See the [alpha.38 release notes](release/notes/v0.1.3-alpha.38.md) for the full changes. Check `--build-info` and the packaged build manifests to confirm source and installer identity.

Implementation and verification scope for file isolation, execution receipts, manually managed skill versions, and message recovery is recorded in the [reliability review](docs/hermes-agent-reliability-review-20260928.zh.md). Its alpha.37 test history includes task acceptance and sample validation mechanisms that were retired in alpha.38.

See the [v0.1.7-rc.2 evaluation](docs/upstream-v0.1.7-rc.2-evaluation.zh.md) and [development plan](docs/plans/更新计划.md) for cross-version adaptation and remaining work.

The Rust edition maintains its own bounded conversation history, targeted navigation, native launcher and themes. Release numbers identify the Rust release line; they do not claim complete Node feature or on-disk format parity.

Teams are available from the conversation header and Settings. Model search keeps its full height, model rows expose a compact delete action, and focus-only composer tips can be switched off in General settings. Computer Use compatibility is documented in [the capability matrix](docs/computer-use-compatibility.zh.md).

## 0.1.3-alpha.38 recent changes

- **Flutter desktop experience**: consistent light/dark themes, text scaling and semantic icons; command search, focus navigation and configurable shortcuts. Workspace drafts and history reading positions persist, and hidden previews release resources. A compact model/reasoning entry uses layered menus, cached catalogs and the latest selection intent.
- **Desktop interaction fixes**: fix the window-close crash and save drafts before closing while the background Host and tasks continue. Reorganize account access, the plugin page and sidebar menus. Settled replies show an artifact card with inline previews and file actions.
- **Model protocol compatibility**: compatible tool input schemas prevent Devin / Claude from rejecting root-level composition. Local argument validation still uses the original tool rules, and Devin errors retain their actual protocol codes.
- **Office and file management**: native `office_read` reads DOCX paragraphs/tables and XLSX cells; `office_write` produces real DOCX/XLSX files. Overwrites and file management require manual approval. Structural checks do not replace content or visual verification.
- **Image generation and editing**: generated and edited images become session attachments for reuse. Explicitly text-only models receive references and descriptions; vision models retain the original images.
- **File isolation and execution**: isolate private directories, attachments and managed temporary files by session. Native sandboxes support exact read roots and long Windows paths. PowerShell correctly initializes its working directory in managed execution copies while the source project remains protected as read-only. Approval waits do not consume execution budgets, and repeated environment startup failures stop within a bounded policy.
- **Simplified execution flow**: retire the project task board, task contracts, task acceptance APIs, completion gates and automatic acceptance continuation. Goals, plans, background jobs and schedules continue independently; users manage skill versions manually.

Desktop verification scope is recorded in the [platform matrix](docs/desktop-platforms.zh.md) and [experience upgrade review](docs/desktop-fidelity/flutter-upgrade-2026-09-29.md). Historical changes remain in the [alpha.36](release/notes/v0.1.3-alpha.36.md), [alpha.31](release/notes/v0.1.3-alpha.31.md) and [alpha.22](release/notes/v0.1.3-alpha.22.md) release notes.

### UU display and manual handoff

1. Select the UU desktop adapter in Settings → Plugins → Computer Use and remote devices, then bind a device belonging to the signed-in account.
2. Open the workbench from the conversation header, click `+`, and select `Computer Use` to connect and view the UU screen in the browser.
3. Keep manual control while completing any system verification yourself. Enlarge the workbench or display as needed; agent control remains paused.
4. Return control to the agent when finished. If the earlier task has already stopped, send a continuation instruction; returning control does not silently repeat a completed task.

`uu_terminal` is the remote command-line tool. The desktop image is shown by the control panel above, not by terminal error output.

## Downloads

Complete Windows x86_64 packages:

| Edition | Installer | Portable package |
|---|---|---|
| Flutter desktop | [Download EXE](https://github.com/qiu7824/deepseek-harness-rs/releases/download/v0.1.3-alpha.38-r6/deepseek-harness-rs-v0.1.3-alpha.38-windows-x86_64-flutter-setup.exe) | [Download ZIP](https://github.com/qiu7824/deepseek-harness-rs/releases/download/v0.1.3-alpha.38-r6/deepseek-harness-rs-v0.1.3-alpha.38-windows-x86_64-flutter-portable.zip) |
| Web core | [Download EXE](https://github.com/qiu7824/deepseek-harness-rs/releases/download/v0.1.3-alpha.38-r6/deepseek-harness-rs-v0.1.3-alpha.38-windows-x86_64-core-setup.exe) | [Download ZIP](https://github.com/qiu7824/deepseek-harness-rs/releases/download/v0.1.3-alpha.38-r6/deepseek-harness-rs-v0.1.3-alpha.38-windows-x86_64-core-portable.zip) |

Linux, macOS and SHA-256 checksums are listed under **Assets** on the [alpha.38 release page](https://github.com/qiu7824/deepseek-harness-rs/releases/tag/v0.1.3-alpha.38-r6); use the assets actually uploaded there. Source ZIPs contain no compiled applications.

A complete package contains the Rust Host, `web/dist`, `config/agent-presets`, bundled Web plugins, Node and ripgrep runtimes, and security documentation. Flutter packages also contain the client, Flutter runtime libraries and assets, with a complete Host in the `host` subdirectory. Keep the entire installation or extracted directory so resources and bundled runtimes remain available.

Source builds and custom installations must provide [ripgrep (rg)](https://github.com/BurntSushi/ripgrep#installation) for file search. Linux system sandbox dependencies are described below.

## Quick start

For the Windows Flutter package, extract it and run `dsh_desktop.exe`. The client starts the bundled Host or connects to an existing local service. Closing the desktop window leaves the background service and its tasks running.

For the Web core package, extract it and run the ZSUI native launcher, then open the browser interface:

```text
Windows: dsh-launcher.exe
Linux/macOS: ./dsh-launcher
```

Confined Shell commands and native terminals on Linux use the system `bubblewrap` sandbox. DEB packages declare this dependency; portable installations should install `bubblewrap` through the distribution's package manager. Confined execution fails explicitly when the sandbox is missing or unavailable.
Ubuntu systems that restrict user namespaces may need an administrator to configure the distribution's recommended [bwrap AppArmor profile](https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007). The application does not change system protection policies automatically.

The default URL is:

```text
http://127.0.0.1:58080/
```

The launcher is built with ZSUI at a fixed commit and requires no CMD, PowerShell, WebView, or extra runtime. It starts, stops, and restarts the real `deepseek-harness-rs web` process and opens the Web UI or log directory. The Windows installer and launcher automatically use Simplified Chinese or English from the operating-system UI language.

Distribution consists of the **Web core and Flutter desktop client**, sharing the Rust Host and HTTP/WebSocket protocol. The Web package is `core`; model catalog and connection management remain available in Settings. Flutter source lives in [`apps/desktop_flutter`](apps/desktop_flutter); platform build and acceptance status is tracked in the [desktop platform matrix](docs/desktop-platforms.zh.md).

## Data, profiles, and workspaces

The Rust core starts without Node. Complete releases bundle Node for JavaScript/TypeScript Code Mode; source builds and some external tools still require their corresponding runtimes. Environment settings report the detected executable, version, and capabilities. Model catalogs synchronize account access and reasoning metadata while preserving display preferences. The code graph indexes the active workspace on demand and links local relationships to source locations. Opening a conversation does not automatically scan the whole project; inferred relationships and coverage limits remain visible.

New Windows installations default to `D:\Program Files (x86)\DeepSeek Harness-rs\<variant>`; upgrades retain the previous installation directory. If the default location is unavailable, choose another directory in the installer.

The default Windows data root is:

```text
%LOCALAPPDATA%\DeepSeek Harness
```

Select a data root with `DSH_HOME` or Settings → Directories and runtime. Restarting applies a verified copy of the data and retains the source. A failed migration restores the previous active paths and reports the error. Session workspaces remain the project-directory source; application data relocation does not move project files.

Profile plugins are stored under:

```text
<DSH_HOME>/profiles/<profile>/node_modules
```

Sessions, attachments, caches, settings, and plugin inventory are user data and must not be overwritten or cleaned by an upgrade package.

## Providers and protocols

Configure an API key or connect an account in Settings → Models. Credentials remain on the local device, with token renewal and sign-out support. Each model has a visibility switch, and reasoning levels prefer provider-supplied metadata.

| Protocol/API | Status |
|---|---|
| DeepSeek/OpenAI-compatible Chat Completions | Connected to the production Rust adapter |
| OpenAI Responses | Explicit `api: openai-responses` route implemented; tool, reasoning, image, usage, and SSE fixtures pass; real-provider verification requires user configuration |
| Azure OpenAI Responses | Production provider closure incomplete |
| OpenAI Codex Responses | Device authorization, token renewal, and Responses routing; users complete account authorization in Settings |
| Anthropic Messages | Native text, tool, image, thinking, and usage conversion using API keys; Claude subscriptions use the official Claude Code subagent |
| Bedrock Converse Stream | Not implemented |

Account quotas and per-request cache tokens are separate statistics. Cache-hit rates use only explicitly reported values: missing data is shown as unavailable, while an explicit zero remains zero. The ChatGPT / Codex account service does not receive explicit cache-breakpoint options intended for the public Responses API. Stable keys support reuse but do not guarantee a hit; consecutive long-prefix requests succeeded through the real account route while still reporting zero cache-read tokens.

See [`docs/protocol-matrix.md`](docs/protocol-matrix.md) for evidence and scope. A type name or crate alone is not proof of production support.

## Skills, MCP, and memory

Settings → Skills and MCP manages skill files and MCP servers, including enable/disable, editing, and connection tests. Memory settings support searching, toggling, and maintaining lessons from known errors. See the [capabilities guide](docs/learning-and-capabilities.zh.md).

## Web plugins

Pure Web plugins do not require Node, npm, or pnpm. The Rust Host validates, discovers, registers, and serves prebuilt client JavaScript.

### Installing third-party plugins

The Rust build directly installs pure Web plugins with this minimum layout:

```text
package.json
lib/client.js
```

`package.json` must declare a Web client export. GitHub sources must be pinned to an immutable 40-character commit SHA; branches, tags, and mutable default branches are rejected.

GitHub installations require an immutable 40-character commit SHA:

```bash
./dsh plugin --profile web add github:owner/repository#0123456789abcdef0123456789abcdef01234567
./dsh plugin --profile web list
./dsh plugin --profile web remove package-name
```

Restart `dsh web` after installation, then confirm the plugin is enabled under Settings → Plugins. To upgrade, review the new commit, remove the old package, and install again with the new commit SHA. The Rust installer validates package names, entry paths, symlinks, file sizes, and directory containment.

Compatibility:

- Pure Web plugin: supported.
- Web + Node Host plugin: only a standalone Web portion can load; the Node Host portion does not run.
- Node Host/native-only plugin: not executed by the Rust Host.

Plugins that require `require()`, npm lifecycle scripts, a Node service, native addons, or Host-side JavaScript cannot run directly inside the pure Rust process. Use a plugin-provided pure Web build or run the Host portion as a separate sidecar.

Web plugins run in the application origin and have page-level JavaScript capabilities. Install only trusted, reviewed code pinned to an immutable commit.

Bundled plugins:

- `dsh-voice-input`: browser speech input.
- `dsh-context-jump`: an indexed conversation rail that loads bounded history around user-message targets, with hover previews and keyboard navigation.
- `dsh-better-sidebar`: resizable workbench panes, workspace files, terminals and web previews, integrated with the native conversation interface.
- `dsh-sidebar-workbench-suite`: Markdown/code/structured-data viewers, background jobs and the shared browser/desktop control panel.

## Capability status

| Capability | Status |
|---|---|
| Sessions, persistence, history paging | Strong `SessionSeq` / `SessionLogOffset` coordinates, explicit bounded reads, and v0 JSONL/Zstd `seedLength` compatibility are implemented |
| DeepSeek streaming, reasoning, tools, images, usage | Implemented; final Release still requires real-provider verification |
| Subagents | Continuable direct parent/child messaging uses `send_message({ agent_id, message })` in both directions; optional Codex/Claude Code providers are not installed by default |
| Web fetch | Rust-native `web_fetch` is implemented with public HTTP(S)-only, redirect/DNS/IP, timeout, size, and cancellation bounds |
| Model discovery | Saved Profile headers can be resolved server-side without returning credentials to the browser; the model picker supports filtered search and visible-only selection |
| Workflows | Engine remains available; the PTC/code preset deliberately omits the generic `workflow` tool while retaining `run_code` and Ralph |
| Terminal | Persistent terminal lifecycle implemented; UU remote terminals additionally depend on target login, unlock, and terminal capability |
| UU desktop control | Isolated controller, dynamic client detection, live video, and input transport implemented; connection and lock-screen capture verified, application input pending unlocked-device validation |
| Screen annotations | Current screenshot, text, and normalized coordinates are saved in a durable user message; drafts are isolated by session |
| MCP | Production settings, stdio/HTTP connections, tool registration, enable/disable, and connection tests |
| LSP | Registry/tool libraries implemented; not composed into the production Host |
| ACP | Protocol entry exists; real prompt/cancel regression is not closed |


Sidebar support and its upstream compatibility limits are documented in [sidebar capabilities](docs/sidebar-capabilities.md); browser executors, model tools, and UU remote integration are described in [browser control](docs/browser-control-and-model-tools.zh.md).

## Build

Rust is pinned to 1.97.1:

```bash
cargo build --release -p dsh-host-cli --bin dsh -p dsh-launcher --bin dsh-launcher
```

Core gates:

```bash
cargo fmt --all -- --check
python tools/verify_product_surface.py
python -m unittest discover -s tools/tests -p "test_memory_*.py" -v
python tools/validate_memory_baseline.py --report docs/memory/production-baseline.jsonl --markdown docs/memory/production-baseline.md
cargo test -p dsh-llm-deepseek --all-targets
cargo test -p dsh-host --lib -- --test-threads=1
cargo test -p dsh-host-cli --lib -- --test-threads=1
```

## Security boundaries

- Remote plaintext HTTP is rejected; bounded loopback fixtures are the exception.
- Credentials are resolved through the credential service and are not stored in source, recordings, or Releases.
- Plugin package names, entry paths, symlinks, sizes, and traversal attempts fail closed.
- Windows tool execution uses AppContainer and approval policy boundaries.
- Release archives contain runtime assets only, not source tests, sessions, caches, or credentials.

See `PLUGIN_SECURITY.md` for the Web plugin trust boundary.

## Known limitations

- The upstream v0.2.0-rc.1 Web client build and Remote contract inventory are complete, but replacing the current Web interface still requires further adaptation. Releases use the existing Web interface; see the [Web migration plan](docs/plans/web-upstream-v0.2.0-rebase.zh.md).
- One-click automatic updates, complete onboarding, and native input-method, screen-reader and cross-platform desktop verification have remaining work. See the [desktop platform matrix](docs/desktop-platforms.zh.md).
- The generic pi-ai provider catalog has not been fully ported.
- UU live verification covers connection and the Windows lock screen; application input and remote terminal commands still require validation after the target is unlocked.
- Multi-account authorization, switching, and recovery pass fixture tests; complete authorization and switching between two real accounts remain unverified. High cache-hit rates on the account service have not been demonstrated.
- LSP remains a library-level implementation without production Host composition.
- ACP real prompt/cancel and Python SDK real-turn regressions remain open.
- Conversation navigation uses the user-message index and targeted history pages; full conversation data stays on the Host.
- Linux and macOS are considered published only after every GitHub Actions matrix asset succeeds.

## License

MIT. See [`LICENSE`](LICENSE) and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
