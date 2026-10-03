# Icon golden provenance

The four icon matrix tests use Flutter's default `LocalFileComparator` with an exact pixel comparison. `Abi.macosArm64` uses `macos-arm64/`; the other ABIs use the PNGs in this directory.

The macOS ARM64 PNGs were copied byte for byte from the reviewed `failures/*_testImage.png` outputs of the original four tests:

- Flutter SDK: `3.47.5`, commit `6a19cca56475dbfba1478ee68d7bd0c2ef891da1`.
- Application source: `833f9ce8030ea55d154e8aac60d3794aefb53c8d`.
- Workflow: [run 37154602872, ARM golden job 111295306908](https://github.com/qiu7824/deepseek-harness-rs/actions/runs/37154602872/job/111295306908).
- Evidence branch: `diagnostics/flutter-goldens-37154602872-macos-aarch64`.
- Parentless evidence commit: `a8a81eac89be06730fceab78c04d43c8570f67ab`; its manifest records every PNG's byte count and SHA-256.

| macOS ARM64 baseline | SHA-256 |
| --- | --- |
| `macos-arm64/icons_light_1.0x.png` | `8bcd440b31259042f9ead78ccb677c13307223e8d97766ba93da031e2eae4920` |
| `macos-arm64/icons_light_1.5x.png` | `c996b3f71c757b0aac9a0076fcd7deff98d8334c035130d363ad9700c078fff1` |
| `macos-arm64/icons_dark_1.0x.png` | `8da701d91d3ca65e90a76729b3df8f3510c8df51edb3fc3f919a64c116d9bb39` |
| `macos-arm64/icons_dark_1.5x.png` | `49bff9baf7b88a06999ea56d59d6c6d1142ed115fb2c9e9a793e0ec5956884e4` |
