# Boa Runtime Spike

## Verdict: PARTIAL

Boa 0.21.1 built successfully on Windows and executed two real probes: JavaScript returned `42`, and `for (;;) {}` was stopped by `set_loop_iteration_limit(100000)` in 1 ms.

The iteration cap is not the upstream measured busy-time budget and cannot implement fair host-binding waits or explicit abort/dispose ownership. TypeScript stripping, async bindings, output accounting, fresh realms, and heap isolation were not validated.

- Build: `C:/Users/Administrator/.cargo/bin/cargo.exe build --release`
- Build elapsed: 150000 ms
- Release artifact: 11814912 bytes
- Target tree: 537839678 bytes
- Run output: `return_42=42`, `loop_interrupted=true elapsed_ms=1`

## Recommendation

Do not select Boa for the production port. See `results.json` for all 13 frozen fixtures.
