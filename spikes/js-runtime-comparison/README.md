# JS Runtime Comparison Spike

## Question

Given the existing Rust `CodeRuntime` seam, when the same TypeScript programs and host bindings run on Windows, which substrate can preserve the upstream worker-thread contract with bounded cancellation and teardown at acceptable build/runtime cost?

## Candidates

| ID | Candidate | Key architectural claim | Highest risk |
|---|---|---|---|
| A | Boa | In-process pure Rust engine | Hard interruption, TypeScript stripping, async host bridge |
| B | `deno_core` | Embedded V8 isolate controlled by Rust | Build size/time, V8 distribution, isolate termination ownership |
| C | isolated Node sidecar | Out-of-process Node 22 worker/process | IPC complexity and process overhead; best semantic parity |

## Shared acceptance contract

Every candidate reads `../fixtures.json` unchanged and writes `results.json` with this shape:

```json
{
  "candidate": "boa|deno_core|node_sidecar",
  "versions": {},
  "build": { "command": "", "success": true, "elapsed_ms": 0, "artifact_bytes": 0 },
  "fixtures": [
    { "id": "fixture id", "status": "PASS|FAIL|UNSUPPORTED", "elapsed_ms": 0, "observed": {} }
  ],
  "teardown": { "clean": true, "details": "" },
  "verdict": "VALIDATED|PARTIAL|INVALIDATED",
  "recommendation": ""
}
```

A candidate passes only if it executes the actual program. A hardcoded fixture result is invalid. Each implementation must expose one runnable command that performs all fixtures and exits nonzero when a supported fixture fails.

## Non-negotiable semantics

- Program body runs inside a strict async-function equivalent: top-level `await` and `return` work.
- Erasable TypeScript syntax is accepted; non-erasable syntax becomes `exception`.
- Host binding arguments/resolutions and final values cross a lossless-JSON boundary.
- A typed binding rejection materializes `ToolCallError` with `toolName`.
- Every run gets a fresh realm.
- Synchronous loops stop under compute timeout and explicit abort, within the fixture deadline.
- Awaiting a never-settling host binding stops at wall timeout.
- Logs plus result/failure are bounded by `max_output_bytes`.
- Teardown leaves no runtime thread/process alive.

## Measurement rules

- Record the exact build/run command, compiler/runtime versions, elapsed time, artifact size, and any downloaded binary payload.
- Run on this Windows host, not by inference from documentation.
- A build failure is evidence and must be recorded verbatim in `results.json`/README.
- `UNSUPPORTED` is acceptable evidence in a spike, but it cannot be counted as PASS.
- Do not modify root `Cargo.toml`, `Cargo.lock`, production crates, or another candidate directory.

## Decision rule

A candidate is eligible for production only if all non-negotiable semantics pass. Among eligible candidates, choose the smallest lifecycle risk first, then build/distribution cost, then steady-state performance. Failed candidate prototypes are deleted after the verdict; the comparison README and selected implementation evidence remain.

## Final verdict

**Selected: `node_sidecar`.** The corrected Node candidate passed all 14 frozen fixtures, including the independent-review regression where a never-settling binding is dispatched without `await` before a synchronous loop. It used worker event-loop utilization for compute accounting, preserved slow-binding fairness, enforced heap/output/wall limits, terminated abort/dispose runs, rejected follow-up after disposal, and shut down with no active workers.

Boa built a smaller native artifact but did not validate TypeScript, async bindings, explicit abort/dispose, fair compute accounting, or heap isolation. `deno_core` proved that the V8 dependency chain can build on Windows, but the bounded runner did not compile and no fixture executed.

The production build therefore uses a managed Node sidecar with one fresh resource-limited worker per run and keeps OS process-tree ownership in Rust `SubprocessRuntime`.
