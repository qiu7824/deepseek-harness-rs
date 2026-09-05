# DeepSeek Harness Rust

DeepSeek Harness Rust is a Rust migration of the DeepSeek Harness Host. It serves the browser application through the production `dsh web` entry point while preserving session, tool, plugin, storage, and RPC compatibility boundaries.

> This project is a prerelease. Treat the compatibility matrix and each GitHub Release note as the authoritative status.

Current release line: `0.1.3-alpha.6`.

The Rust edition maintains its own bounded conversation history, targeted navigation, native launcher and themes. Release numbers identify the Rust release line; they do not claim complete Node feature or on-disk format parity.

[中文说明](README.zh.md)

## Downloads

Download a complete package from [GitHub Releases](https://github.com/qiu7824/deepseek-harness-rs/releases):

- `deepseek-harness-rs-v0.1.3-alpha.6-windows-x86_64-{core,skin,free}-portable.zip`
- `deepseek-harness-rs-v0.1.3-alpha.6-linux-x86_64-{core,skin,free}-portable.tar.gz`
- `deepseek-harness-rs-v0.1.3-alpha.6-macos-{x86_64,aarch64}-{core,skin,free}-portable.tar.gz`
- matching Windows `setup.exe`, Linux `.deb`, and macOS `.pkg` installers

A complete package contains the binary, `web/dist`, `config/agent-presets`, bundled Web plugins, and security documentation. Copying only the binary does not provide a complete Web installation.

## Quick start

The default download is the `core` package, which contains no extension skins. Launch the shared ZSUI native manager:

```text
Windows: dsh-launcher.exe
Linux/macOS: ./dsh-launcher
```

The default URL is:

```text
http://127.0.0.1:58080/
```

The launcher is built with ZSUI at a fixed commit and requires no CMD, PowerShell, WebView, or extra runtime. It starts, stops, and restarts the real `deepseek-harness-rs web` process and opens the Web UI or log directory. The Windows installer and launcher automatically use Simplified Chinese or English from the operating-system UI language.

For extension skins, download the separate `skin` package and run `deepseek-harness-rs-skin` (`.exe` on Windows). It installs only the skin payload into the adjacent `web/dist/skins`; the default `core` archive never bundles skin assets.

The `free` package uses the same Rust runtime and Web UI as `core`, with only the anonymous models that passed release verification. Exact IDs in the [official model directory](https://opencode.ai/zen/v1/models) and official input/output/cache-read prices are checked before streaming inference and a tool-result round trip. Evidence in `free-model-verification.json` is less than 24 hours old and tied to the packaged binary hash. Settings provide the current free catalog, verification results, and controls to test and add eligible models; no credentials or skin payload are bundled.

## Data, profiles, and workspaces

The Rust core starts without Node. JavaScript/TypeScript Code Mode and some external tools require an optional Node installation; environment settings report the detected executable, version, and capabilities. Model catalogs synchronize account access and reasoning metadata while preserving display preferences. The code graph indexes the active workspace automatically and links local relationships to source locations.

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
- `dsh-composer-expand`: expandable composer.
- `dsh-context-jump`: a Codex-style left-side conversation rail with hover titles, click-to-jump marks, top/bottom controls, and `Alt+Up/Down` navigation without replacing the native session header.
- `dsh-web-preview-rs`: session-scoped Rust-native workspace browser for Markdown/source, images, media, PDF, isolated HTML sites, text/element annotations, and drop-to-workspace uploads. Project execution accepts only Host-detected fixed argv after a one-shot 60-second confirmation challenge and requires full WorkspaceWrite OS-sandbox enforcement, credential scrubbing, managed process trees, and bounded logs.

## Capability status

| Capability | Status |
|---|---|
| Sessions, persistence, history paging | Strong `SessionSeq` / `SessionLogOffset` coordinates, explicit bounded reads, and v0 JSONL/Zstd `seedLength` compatibility are implemented |
| DeepSeek streaming, reasoning, tools, images, usage | Implemented; final Release still requires real-provider verification |
| Subagents | Continuable direct parent/child messaging uses `send_message({ agent_id, message })` in both directions; optional Codex/Claude Code providers are not installed by default |
| Web fetch | Rust-native `web_fetch` is implemented with public HTTP(S)-only, redirect/DNS/IP, timeout, size, and cancellation bounds |
| Model discovery | Saved Profile headers can be resolved server-side without returning credentials to the browser; the model picker supports filtered search and visible-only selection |
| Workflows | Engine remains available; the PTC/code preset deliberately omits the generic `workflow` tool while retaining `run_code` and Ralph |
| Terminal | Persistent terminal lifecycle implemented |
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

- The generic pi-ai provider catalog has not been fully ported.
- LSP remains a library-level implementation without production Host composition.
- ACP real prompt/cancel and Python SDK real-turn regressions remain open.
- The first `dsh-context-jump` release navigates rendered stable nodes; a full Turn/Step directory requires a formal timeline slot.
- Linux and macOS are considered published only after every GitHub Actions matrix asset succeeds.

## License

MIT. See [`LICENSE`](LICENSE) and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
