# Surtr

Surtr is an open-source, cross-platform hardware inspection, monitoring,
stress-testing and tuning tool for gamers, overclockers, AI workstation owners
and anyone who wants to know their PC is healthy. It is written in Rust.

**Status: pre-release, early development (M7 next).** Preview builds are on
the [releases page](https://github.com/surtrhq/surtr/releases). Working today: system information, CPU, memory and GPU sensors
(NVIDIA through NVML, AMD on Linux), a terminal UI, history recording, alerts,
Prometheus metrics, a read-only MCP server for AI agents ([docs/mcp.md](docs/mcp.md))
and a local REST and WebSocket API ([docs/api.md](docs/api.md)).
A verified CPU stress test (`surtr stress cpu`) checks every result, stops at
a temperature limit, records each run and compares runs; tuning comes later.
A stress test can still crash or hang an unstable machine, so save your work
first.

See [the plan](docs/PLAN.md) for goals, architecture and milestones, and
[the mockups](docs/mockups/README.md) for the intended UI. Bug reports and
hardware reports are welcome: see [CONTRIBUTING.md](CONTRIBUTING.md).

## Layout

| Crate | Purpose |
|---|---|
| [`surtr-model`](crates/surtr-model) | Plain typed data (hardware, sensors, capabilities). Every value is optional and tagged with its source. No I/O. |
| [`surtr-backend`](crates/surtr-backend) | The `Provider` trait every hardware backend implements, plus a deterministic mock provider (feature `mock`). |
| [`surtr-backend-native`](crates/surtr-backend-native) | Driverless OS backends (Windows, Linux, macOS): OS, CPU and memory information; CPU load, clocks and memory use on Windows and Linux, plus `hwmon` sensors on Linux. |
| [`surtr-backend-nvidia`](crates/surtr-backend-nvidia) | NVIDIA GPUs through NVML, loaded at run time from the driver install (never shipped; see its NOTICE): identity, sensors, throttle reasons, per-process memory. |
| [`surtr-backend-amd`](crates/surtr-backend-amd) | AMD GPUs on Linux through the `amdgpu` sysfs files: identity (named from `pci.ids`), sensors, throttle reasons from `gpu_metrics` where its table reports them, per-process memory from DRM `fdinfo`. |
| [`surtr-core`](crates/surtr-core) | The `Hub`, which combines the OS backend and GPU backends into one view (and logs provider failures); per-user paths, home-directory redaction and the settings file (`config.toml`). |
| [`surtr-api`](crates/surtr-api) | `surtr serve --api`: a read-only REST and WebSocket API on loopback, behind a bearer token; see [docs/api.md](docs/api.md). Also the JSON answers it shares with the MCP server. |
| [`surtr-mcp`](crates/surtr-mcp) | `surtr mcp`: a read-only MCP server so AI agents can ask about this machine's hardware; see [docs/mcp.md](docs/mcp.md). |
| [`surtr-ipc`](crates/surtr-ipc) | Local IPC between `surtr serve` and front ends, over a socket (Unix) or named pipe (Windows) that only the same user can open. While `serve` runs, other commands use its latest readings (`--embedded` reads the hardware directly). |
| [`surtr-export`](crates/surtr-export) | Prometheus text format for `surtr serve --metrics`; Grafana dashboards in [`dashboards/`](dashboards). |
| [`surtr-store`](crates/surtr-store) | Sensor history in SQLite (compiled in, not shipped separately): retention and bucketed queries. |
| [`surtr-view`](crates/surtr-view) | What every front end shows: view models, value formatting, health levels and theme tokens (dark and light). |
| [`surtr-tui`](crates/surtr-tui) | `surtr tui`: Overview, Sensors (with trends and a `/` filter), GPU, Capabilities and Stress views (choose, confirm, run and stop the CPU test); command palette (`:`); keyboard and mouse; works over SSH. |
| [`surtr-cli`](crates/surtr-cli) | The `surtr` command: `info`, `status`, `tui`, `watch`, `alerts`, `caps`, `setup`, `sensors`, `stress cpu`, `runs`, `gpu procs`, `gpu throttle`, `serve`, `mcp`, `token`, `record`, `history`, `export`, `diag`, `config`, `completions`, with `--json` and `--mock`. Colored on a terminal, plain when piped or with `NO_COLOR`. |

## Building

```sh
cargo test --workspace --all-features
cargo run -p surtr-cli -- info          # this machine
cargo run -p surtr-cli -- --mock --json # the mock provider, as JSON
```

## Using Surtr with AI agents

- **Coding agents with a shell** (Claude Code, Codex, Cursor, ...): install
  the [agent skill](skills/surtr/SKILL.md). For Claude Code, copy
  `skills/surtr` to `~/.claude/skills/surtr` (all projects) or to
  `.claude/skills/surtr` in one project; other agents that support the Agent
  Skills format have their own skills folder. The skill teaches the agent
  which `surtr ... --json` command answers which question, how to read the
  results, and never to run `surtr setup` without asking.
- **Chat clients** (Claude Desktop, Cursor, VS Code and others): use the MCP
  server. `surtr mcp config <client>` prints the exact setup; see
  [docs/mcp.md](docs/mcp.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
