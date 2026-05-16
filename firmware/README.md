# `firmware/` — MCU targets

Nested Cargo workspace for microcontroller targets. The host workspace
at the repo root uses an explicit, globless `members = [...]` list, so
`firmware/` is simply not a member — no `exclude` is needed. Kept
separate because these crates build against the esp-rs Xtensa Rust fork
and target triples like `xtensa-esp32s3-espidf` or
`riscv32imac-unknown-none-elf`, which can't share the host workspace's
build graph.

Driven by [`docs/plans/star-adventurer-gti-embedded.md`](../docs/plans/star-adventurer-gti-embedded.md).

## Layout

```
firmware/
├── spikes/
│   └── usb-cdc-hello/   — Phase 0: USB-CDC host → mount round-trip
└── (future)
    ├── alpaca-embedded/             — no_std Alpaca handler over picoserve
    ├── rp-auth-embedded/            — no_std port of crates/rp-auth
    ├── star-adventurer-gti-esp32s3/ — primary firmware
    └── star-adventurer-gti-rp2350/  — RISC-V validation target
```

Each leaf crate has its own `rust-toolchain.toml` (pinning the
appropriate toolchain) and `.cargo/config.toml` (pinning the target
triple). Do not invoke `cargo` from this directory — `cd` into the
specific crate.

## Tooling install

See the per-crate README. The common one-time setup is
[`spikes/usb-cdc-hello/README.md`](spikes/usb-cdc-hello/README.md).
