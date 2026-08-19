# deno_core 0.410.0 runtime comparison spike

## Verdict: PARTIAL

Windows V8 distribution/build feasibility was demonstrated, but the candidate runner did not compile and no frozen fixture program executed. All 13 fixtures are therefore reported as `UNSUPPORTED`; none are claimed as a pass.

## Commands and measured evidence

- Build command: `C:/Users/Administrator/.cargo/bin/cargo.exe build --release`
- First build elapsed: 154,000 ms
- Artifact bytes: 0 (no executable produced)
- Run command intended: `target/release/deno-core-runtime-spike.exe`
- Run attempted: no
- Toolchain: Cargo 1.97.1, rustc 1.97.1, `x86_64-pc-windows-msvc`
- Runtime dependency versions: `deno_core 0.410.0`, `v8 150.4.0`, `deno_v8 0.2.0`, `serde_v8 0.319.0`
- Frozen fixture source: `../fixtures.json`, SHA-256 `c60d164ed4ec003be74e9599fcb7a1a41b354ea9189d959a891722a555d6d3e2`, 13 fixtures

Cargo downloaded the V8 crate chain and the real Windows build reached `Compiling v8 v150.4.0`, then completed `deno_v8`, `serde_v8`, and `deno_core`. The V8 payload byte count was not separately captured before the hard stop. The first local CLI failure was:

```text
error[E0599]: no method named `handle_scope` found for struct `JsRuntime`
  --> src\main.rs:15:30
```

After changing to `JsRuntime::resolve`, the latest build still failed on value conversion:

```text
error[E0308]: mismatched types
  --> src\main.rs:15:8
  expected serde_json::Value, found deno_core::deno_v8::Global<deno_core::deno_v8::Value>
```

A later source edit removed that conversion, but the hard-stop instruction prohibited rebuilding or running it, so the current source remains unverified.

## Fixture outcome

| Fixture | Status | Evidence |
|---|---|---|
| typescript_erasable | UNSUPPORTED | CLI did not compile; TypeScript stripping not exercised |
| top_level_await_binding | UNSUPPORTED | wrapper and host binding not exercised |
| typed_binding_rejection | UNSUPPORTED | typed rejection not exercised |
| sync_loop_compute_timeout | UNSUPPORTED | IsolateHandle probe authored but never built/run |
| sync_loop_abort | UNSUPPORTED | explicit abort not exercised |
| dispose_inflight | UNSUPPORTED | disposal, join, teardown, and follow-up rejection not exercised |
| idle_wall_timeout | UNSUPPORTED | never-settling op and wall timeout not exercised |
| slow_binding_not_charged | UNSUPPORTED | delayed binding compute accounting not exercised |
| heap_limit | UNSUPPORTED | isolate heap limit and healthy follow-up not exercised |
| output_limit | UNSUPPORTED | output budgeting not exercised |
| invalid_output | UNSUPPORTED | lossless-JSON validation not exercised |
| non_erasable_typescript | UNSUPPORTED | enum rejection not exercised |
| fresh_realm | UNSUPPORTED | fresh isolate sequence not exercised |

## What worked

- An independent Cargo project resolved `deno_core 0.410.0` without changing the root workspace or root lockfile.
- On this Windows host, the dependency build compiled V8 150.4.0 and the deno_core stack.

## What did not

- No release executable was produced; artifact size is therefore 0.
- No fixture was actually executed, including the return-42 and IsolateHandle loop-termination probes.
- Runtime disposal, heap isolation, host async operations, budgets, and teardown remain unverified.

## Recommendation

Do not select this candidate from this spike. The build evidence reduces Windows V8 distribution risk, but all contract semantics remain unproven until the deno_core 0.410 value API is integrated and the complete fixture runner is built and executed.
