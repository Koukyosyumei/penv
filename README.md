# penv

penv is a Rust command-line firmware simulator designed for speed, low memory
use, and compatibility with Renode. It supports selected Arm and RISC-V
platforms and can load supported Renode platform descriptions (`.repl`) and
monitor scripts (`.resc`). Compatibility varies by platform and peripheral.

## Quick start

Build with a recent stable Rust toolchain, then run a firmware image:

```sh
cargo build --release
target/release/penv run --platform board.repl firmware.elf
target/release/penv run --help
```

To run a Renode script, point penv at the Renode checkout and the directory
containing any firmware assets referenced by the script:

```sh
target/release/penv run --script board.resc \
  --platform-root ../renode --asset-dir ./assets
```

## Validation

Run `cargo test` for the Rust test suite. The upstream Robot runner executes
Renode test cases against penv and records results by case ID. See the
[upstream test workflow](docs/renode-upstream-ci.md) and
[compatibility case matrix](docs/release-case-matrix.md).

The previous README's detailed commands and validation notes are available in
[the repository history](https://github.com/Koukyosyumei/penv/blob/3e01c2efef507f42c40ff3a3b6d0b9c820daeea9/README.md).
