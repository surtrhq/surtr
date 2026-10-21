# Surtr

Surtr is an open-source, cross-platform hardware inspection, monitoring,
stress-testing and tuning tool for gamers, overclockers, AI workstation owners
and anyone who wants to know their PC is healthy. It is written in Rust.

**Status: early development (milestone M2, sensors).** OS, CPU and memory
information and basic CPU and memory sensors work so far. See [the plan](docs/PLAN.md) for goals, architecture and
milestones, and [the mockups](docs/mockups/README.md) for the intended UI.

## Layout

| Crate | Purpose |
|---|---|
| [`surtr-model`](crates/surtr-model) | Plain typed data (hardware, sensors, capabilities). Every value is optional and tagged with its source. No I/O. |
| [`surtr-backend`](crates/surtr-backend) | The `Provider` trait every hardware backend implements, plus a deterministic mock provider (feature `mock`). |
| [`surtr-backend-native`](crates/surtr-backend-native) | Driverless OS backends (Windows, Linux, macOS): OS, CPU and memory information; CPU load, clocks and memory use on Windows and Linux, plus `hwmon` sensors on Linux. |
| [`surtr-cli`](crates/surtr-cli) | The `surtr` command: `surtr info`, `surtr caps`, `surtr sensors`, `--json`, `--mock`. |

## Building

```sh
cargo test --workspace --all-features
cargo run -p surtr-cli -- info          # this machine
cargo run -p surtr-cli -- --mock --json # the mock provider, as JSON
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
