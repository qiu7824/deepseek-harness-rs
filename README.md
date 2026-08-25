# DeepSeek Harness Rust

DeepSeek Harness Rust is a Rust migration of the DeepSeek Harness Host. It serves the browser application through the production `dsh web` entry point while preserving session, tool, plugin, storage, and RPC compatibility boundaries.

> This project is a prerelease. Treat the compatibility matrix and each GitHub Release note as the authoritative status.

[中文说明](README.zh.md)

## Downloads

Download a complete package from [GitHub Releases](https://github.com/qiu7824/deepseek-harness-rs/releases):

- `dsh-windows-x86_64.zip`
- `dsh-linux-x86_64.tar.gz`
- `dsh-macos-x86_64.tar.gz`
- `dsh-macos-aarch64.tar.gz`

A complete package contains the binary, `web/dist`, `config/agent-presets`, bundled Web plugins, and security documentation. Copying only the binary does not provide a complete Web installation.

## Quick start

Windows:

```powershell
.\dsh.exe web
```

Linux and macOS:

```bash
./dsh web
```

The default URL is:

```text
http://127.0.0.1:58080/
```

The Windows package also includes a PowerShell 7 + WPF service manager:

The simplest option is to double-click:

```text
windows\启动DSH管理器.cmd
```

If PowerShell 7 is missing, the CMD launcher displays and copies a one-command Windows x64 MSI installation command. Installation requires administrator approval; double-click the CMD again after installation. See the Chinese README for the exact localized command.

Or launch it from a terminal:

```powershell
pwsh -NoProfile -STA -WindowStyle Hidden -File .\windows\DshServiceManager.ps1
```

It starts, stops, and restarts the real `dsh.exe web` process and opens the Web UI or log directory. It does not configure providers, models, credentials, or accounts.

## Data, profiles, and workspaces

The default Windows data root is:

```text
%LOCALAPPDATA%\DeepSeek Harness
```

Use `DSH_HOME` to select an explicit data root. The session workspace is the only project-directory source; there is no duplicate “work directory” setting.

Profile plugins are stored under:

```text
<DSH_HOME>/profiles/<profile>/node_modules
```

Sessions, attachments, caches, settings, and plugin inventory are user data and must not be overwritten or cleaned by an upgrade package.

## Providers and protocols

The project does not configure a provider, API key, default model, or account on the user's behalf.

| Protocol/API | Status |
|---|---|
| DeepSeek/OpenAI-compatible Chat Completions | Connected to the production Rust adapter |
| OpenAI Responses | Explicit `api: openai-responses` route implemented; tool, reasoning, image, usage, and SSE fixtures pass; real-provider verification requires user configuration |
| Azure OpenAI Responses | Production provider closure incomplete |
| OpenAI Codex Responses | Production provider closure incomplete |
| Anthropic Messages | Not implemented |
| Bedrock Converse Stream | Not implemented |

See [`docs/protocol-matrix.md`](docs/protocol-matrix.md) for evidence and scope. A type name or crate alone is not proof of production support.

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
| Sessions, persistence, history paging | Connected to the production Host |
| DeepSeek streaming, reasoning, tools, images, usage | Implemented; final Release still requires real-provider verification |
| Subagents | In-process spawn/fork available; optional Codex/Claude Code providers are not installed by default |
| Workflows | Engine and tool implemented; latest Release still needs real-model E2E |
| Terminal | Persistent terminal lifecycle implemented |
| MCP | Client library implemented; not composed into the production Host configuration |
| LSP | Registry/tool libraries implemented; not composed into the production Host |
| ACP | Protocol entry exists; real prompt/cancel regression is not closed |

## Build

Rust is pinned to 1.97.1:

```bash
cargo build --release -p dsh-host-cli --bin dsh
```

Core gates:

```bash
cargo fmt --all -- --check
python tools/verify_product_surface.py
cargo test -p dsh-llm-deepseek --all-targets
cargo test -p dsh-host --test boot -- --test-threads=1
cargo test -p dsh-host-cli --test web -- --test-threads=1
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
- MCP and LSP are library-level implementations, not production Host features yet.
- ACP real prompt/cancel and Python SDK real-turn regressions remain open.
- The first `dsh-context-jump` release navigates rendered stable nodes; a full Turn/Step directory requires a formal timeline slot.
- Linux and macOS are considered published only after every GitHub Actions matrix asset succeeds.

## License

MIT. See [`LICENSE`](LICENSE) and [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).
