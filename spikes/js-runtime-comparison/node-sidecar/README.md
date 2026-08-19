# Isolated Node Sidecar Runtime Spike

## Verdict: VALIDATED

Eligible on the measured semantics: use a persistent Node 22 sidecar with one fresh, resource-limited worker per run and keep host calls on the NDJSON/worker-IPC bridge. Before production, add OS sandboxing and package the pinned Node executable.

## Architecture exercised

`host.py` is the Python host and `run.py` is the runnable entry point. The host starts one real Node sidecar over stdin/stdout NDJSON; the sidecar creates a fresh `worker_threads.Worker` for every run. Binding calls travel worker IPC → sidecar NDJSON → Python and resolutions/rejections return along the reverse path. The sidecar terminates each worker after completion and forcibly terminates synchronous loops on compute timeout or host abort.

TypeScript is transformed only by Node's native `module.stripTypeScriptTypes(..., { mode: "strip" })`; no evaluator or fixture result exists in the Python test host.

## Exact commands and measurements

```text
cd D:\deepwork\deepseek-harness-rs\spikes\js-runtime-comparison\node-sidecar
node --no-warnings --check sidecar.mjs && node --no-warnings --check worker.mjs
python run.py
```

- Measured UTC: `2026-08-18T22:01:54.576950+00:00`
- Build check: `193 ms`, success `true`
- Full fixture run and teardown: `2898 ms`
- Runtime scripts: `43654 bytes`
- Installed Node executable: `86997320 bytes`
- Downloaded binary payload: `0 bytes`
- Node: `22.23.2`; V8: `12.4.254.21-node.56`; Python: `3.11.15`

Script byte counts:

- `run.py`: `80`
- `host.py`: `28092`
- `sidecar.mjs`: `8726`
- `worker.mjs`: `6756`

## Fixture observations

| Fixture | Status | Elapsed (ms) | Actual observation |
|---|---:|---:|---|
| `typescript_erasable` | PASS | 106 | `{"value":42,"logs":[],"error":null,"serialized_payload_bytes":35,"termination":"completed","worker_thread_id":1,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":102,"host_elapsed_ms":106,"limits":{"compute_ms":300,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[],"abort_signal":{"requested":false},"assertions":["value matched","logs matched","error matched"]}` |
| `top_level_await_binding` | PASS | 114 | `{"value":{"nested":[1,{"ok":true}]},"logs":["echoed true"],"error":null,"serialized_payload_bytes":72,"termination":"completed","worker_thread_id":2,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":112,"host_elapsed_ms":114,"limits":{"compute_ms":300,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[{"name":"tools.echo","args":[{"nested":[1,{"ok":true}]}],"resolution":{"nested":[1,{"ok":true}]}}],"abort_signal":{"requested":false},"assertions":["value matched","logs matched","error matched"]}` |
| `typed_binding_rejection` | PASS | 100 | `{"value":{"typed":true,"name":"ToolCallError","toolName":"fail","message":"fixture failure"},"logs":[],"error":null,"serialized_payload_bytes":116,"termination":"completed","worker_thread_id":3,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":99,"host_elapsed_ms":100,"limits":{"compute_ms":300,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[{"name":"tools.fail","args":[{}],"rejection":{"kind":"tool-call","toolName":"fail","message":"fixture failure"}}],"abort_signal":{"requested":false},"assertions":["value matched","logs matched","error matched"]}` |
| `sync_loop_compute_timeout` | PASS | 231 | `{"value":null,"logs":[],"error":{"kind":"timeout","reason":"compute","message":"compute timeout exceeded"},"serialized_payload_bytes":107,"termination":"timeout:compute","worker_thread_id":4,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":229,"host_elapsed_ms":231,"limits":{"compute_ms":150,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[],"abort_signal":{"requested":false},"assertions":["error kind 'timeout' matched","elapsed 231 ms <= 3000 ms"]}` |
| `binding_dispatch_does_not_pause_busy_compute` | PASS | 218 | `{"value":null,"logs":[],"error":{"kind":"timeout","reason":"compute","message":"compute timeout exceeded"},"serialized_payload_bytes":107,"termination":"timeout:compute","worker_thread_id":5,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":217,"host_elapsed_ms":218,"limits":{"compute_ms":150,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[{"name":"tools.never","args":[{}],"resolution":"left pending for wall-timeout fixture"}],"abort_signal":{"requested":false},"assertions":["error kind 'timeout' matched","timeout reason 'compute' matched","elapsed 218 ms <= 3000 ms"]}` |
| `sync_loop_abort` | PASS | 160 | `{"value":null,"logs":[],"error":{"kind":"abort","message":"execution aborted by host"},"serialized_payload_bytes":87,"termination":"abort","worker_thread_id":6,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":159,"host_elapsed_ms":160,"limits":{"compute_ms":30000,"wall_ms":30000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[],"abort_signal":{"requested":true,"sent_after_ms":151},"assertions":["error kind 'abort' matched","elapsed 160 ms <= 3000 ms"]}` |
| `dispose_inflight` | PASS | 160 | `{"value":null,"logs":[],"error":{"kind":"abort","reason":"dispose","message":"execution aborted by host"},"serialized_payload_bytes":106,"termination":"abort:dispose","worker_thread_id":1,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":157,"host_elapsed_ms":160,"limits":{"compute_ms":30000,"wall_ms":30000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[],"abort_signal":{"requested":false,"dispose_requested":true,"dispose_sent_after_ms":151},"assertions":["error kind 'abort' matched","elapsed 160 ms <= 3000 ms"],"rejects_followup_run":true,"followup_rejection":"runtime is disposed","dispose_active_workers":0,"teardown_clean":true,"sidecar_shutdown":{"type":"shutdown","active_workers":0,"exit_code":0,"stderr":[]}}` |
| `idle_wall_timeout` | PASS | 254 | `{"value":null,"logs":[],"error":{"kind":"timeout","reason":"wall","message":"wall timeout exceeded"},"serialized_payload_bytes":101,"termination":"timeout:wall","worker_thread_id":7,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":253,"host_elapsed_ms":254,"limits":{"compute_ms":30000,"wall_ms":250,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[{"name":"tools.never","args":[{}],"resolution":"left pending for wall-timeout fixture"}],"abort_signal":{"requested":false},"assertions":["error kind 'timeout' matched","elapsed 254 ms <= 3000 ms"]}` |
| `slow_binding_not_charged` | PASS | 593 | `{"value":"slow-done","logs":[],"error":null,"serialized_payload_bytes":44,"termination":"completed","worker_thread_id":8,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":592,"host_elapsed_ms":593,"limits":{"compute_ms":150,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[{"name":"tools.slow","args":[{}],"delay_ms":500,"resolution":"slow-done"}],"abort_signal":{"requested":false},"assertions":["value matched","logs matched","error matched"]}` |
| `heap_limit` | PASS | 119 | `{"value":null,"logs":[],"error":{"kind":"worker-exit","reason":"Worker terminated due to reaching memory limit: JS heap out of memory","message":"Worker terminated due to reaching memory limit: JS heap out of memory"},"serialized_payload_bytes":218,"termination":"worker-exit:Worker terminated due to reaching memory limit: JS heap out of memory","worker_thread_id":9,"worker_exit_code":null,"termination_error":null,"sidecar_elapsed_ms":118,"host_elapsed_ms":119,"limits":{"compute_ms":30000,"wall_ms":30000,"max_output_bytes":65536,"max_old_generation_mb":32},"binding_calls":[],"abort_signal":{"requested":false},"assertions":["error kind 'worker-exit' matched","elapsed 119 ms <= 15000 ms"],"host_survives_followup":true,"followup":{"value":"healthy-after-oom","logs":[],"error":null,"serialized_payload_bytes":52,"termination":"completed","worker_thread_id":10,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":101,"host_elapsed_ms":102,"limits":{"compute_ms":300,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[],"abort_signal":{"requested":false}}}` |
| `output_limit` | PASS | 113 | `{"value":null,"logs":[],"error":{"kind":"output-limit","message":"max_output_bytes exceeded","limit":256},"serialized_payload_bytes":106,"termination":"output-limit","worker_thread_id":11,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":112,"host_elapsed_ms":113,"limits":{"compute_ms":300,"wall_ms":2000,"max_output_bytes":256,"max_old_generation_mb":64},"binding_calls":[],"abort_signal":{"requested":false},"assertions":["error kind 'output-limit' matched","payload 106 bytes <= 256 bytes"]}` |
| `invalid_output` | PASS | 103 | `{"value":null,"logs":[],"error":{"kind":"invalid-output","message":"return value.callable has non-JSON type function"},"serialized_payload_bytes":119,"termination":"completed","worker_thread_id":12,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":102,"host_elapsed_ms":103,"limits":{"compute_ms":300,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[],"abort_signal":{"requested":false},"assertions":["error kind 'invalid-output' matched"]}` |
| `non_erasable_typescript` | PASS | 117 | `{"value":null,"logs":[],"error":{"kind":"exception","name":"SyntaxError","message":"TypeScript enum is not supported in strip-only mode"},"serialized_payload_bytes":138,"termination":"completed","worker_thread_id":13,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":116,"host_elapsed_ms":117,"limits":{"compute_ms":300,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[],"abort_signal":{"requested":false},"assertions":["error kind 'exception' matched"]}` |
| `fresh_realm` | PASS | 247 | `{"fresh_per_run":true,"runs":[{"value":1,"logs":[],"error":null,"serialized_payload_bytes":34,"termination":"completed","worker_thread_id":14,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":137,"host_elapsed_ms":138,"limits":{"compute_ms":300,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[],"abort_signal":{"requested":false}},{"value":"undefined","logs":[],"error":null,"serialized_payload_bytes":44,"termination":"completed","worker_thread_id":15,"worker_exit_code":1,"termination_error":null,"sidecar_elapsed_ms":108,"host_elapsed_ms":109,"limits":{"compute_ms":300,"wall_ms":2000,"max_output_bytes":65536,"max_old_generation_mb":64},"binding_calls":[],"abort_signal":{"requested":false}}],"assertions":["sequence[0] value matched","sequence[1] value matched","each sequence run used a distinct worker thread"]}` |

## Teardown

- Clean: `true`
- shutdown reported active_workers=0; sidecar exit_code=0; PID 6736 alive after wait=False; stderr=[]

## What worked

- Erasable TypeScript, strict async-function top-level `await`/`return`, lossless JSON binding traffic, and typed `ToolCallError` all executed inside real Node workers.
- Fresh workers prevented global leakage between sequence runs.
- Worker termination bounded synchronous loops for both compute timeout and an explicit abort sent by the Python host over NDJSON.
- Compute accounting pauses while a host binding is pending, so the never-resolving binding reaches the independent wall deadline.
- Output is JSON-validated and byte-metered before crossing the final result boundary; non-JSON values and oversized logs become typed failures.

## What didn't / constraints

- This validates lifecycle and semantic parity, not hostile-code sandboxing; a production sidecar still needs an OS-level containment policy.
- A persistent sidecar amortizes process startup, but every invocation still pays fresh worker startup/termination cost shown above.

## Recommendation for the real build

Eligible on the measured semantics: use a persistent Node 22 sidecar with one fresh, resource-limited worker per run and keep host calls on the NDJSON/worker-IPC bridge. Before production, add OS sandboxing and package the pinned Node executable.
