# Changelog

All notable changes to Surtr are listed here. Surtr follows
[Semantic Versioning](https://semver.org/); before 1.0, minor versions may
change commands and JSON output.

## [Unreleased]

### Added
- `surtr stress cpu`: a verified CPU stress test. Every thread repeats
  computations with known answers, and any wrong answer is reported with
  its CPU and clock. It runs in a separate worker process, stops at 95 °C
  (or 5 °C below the CPU's own critical limit) and after 30 minutes at
  most (`[stress]` in config.toml), and refuses to run without a CPU
  temperature sensor unless `--unmonitored` (60 seconds at most).
- `surtr runs`, `surtr runs show`, `surtr runs report`: every stress run is
  recorded, and a run cut short by a reset or freeze is reported as
  interrupted with what it recorded up to then; HTML reports with charts.
- Sensors carry the limits the hardware reports; `status` and alerts use
  them.
- MCP: `list_runs` and `get_run` read recorded stress runs. With
  `[mcp] allow_stress = true`, agents can request a stress test
  (`start_stress_test`), check it and stop it; every run still starts only
  when a person types `surtr runs confirm <code>` in a terminal.

### Fixed
- Linux: impossible placeholder temperatures (-273.2 °C) are no longer shown.
- Linux: no false warning for network adapters that run hot by design.
- Linux: with acpi-cpufreq, the listed maximum clock is shown as the base
  clock.
- The in-kernel amdgpu driver shows its kernel release as its version.
- `surtr tui` no longer quits when an arrow key arrives in pieces over SSH;
  Esc no longer quits (use q or Ctrl+C).

## [0.1.0] - 2026-10-22

The first preview release: hardware inspection and monitoring from the
command line, a terminal UI, history, alerts, Prometheus metrics, a local
API and an MCP server for AI agents. Stress testing and tuning are not in
this release.

Linux is the best-supported platform. Windows works without a driver but
reads fewer sensors (no CPU temperature yet). macOS shows system information,
CPU load and memory use only.

### Inspect and monitor
- `surtr` / `surtr info`: OS, CPU, memory and GPUs.
- `surtr sensors`, `surtr watch`: CPU load, clocks and memory use on Linux and
  Windows; Linux `hwmon` sensors (temperatures, fans, voltages); NVIDIA GPUs
  through NVML (loaded from the installed driver); AMD GPUs on Linux through
  `amdgpu`.
- `surtr gpu procs`, `surtr gpu throttle`: processes using each GPU, and why a
  GPU runs below its potential clock.
- `surtr status`: a health summary with plain-language findings; exit code 0
  ok, 2 warning, 3 critical.
- `surtr caps`: what Surtr can and cannot read here, with the reason and a
  fix; `surtr setup` runs those fixes one confirmed step at a time.
- `surtr tui`: interactive terminal UI (overview, sensors with trends, GPUs,
  capabilities); works over SSH.
- Every command prints JSON with `--json`.

### Record, alert and export
- `surtr serve`: the background daemon. It records history to SQLite, raises
  alerts (with desktop notifications), and lets other commands use its
  latest readings over an owner-only local socket or pipe.
- `surtr record`, `surtr history`, `surtr export` (CSV or JSON).
- `surtr serve --metrics`: Prometheus metrics, with Grafana dashboards in
  `dashboards/`.

### Automate
- `surtr mcp`: a read-only MCP server for AI agents (Claude Code, Claude
  Desktop, Cursor, VS Code, ...); `surtr mcp config <client>` prints the
  setup. See `docs/mcp.md`.
- `surtr serve --mcp` and `--api`: MCP over HTTP and a REST + WebSocket API,
  loopback only, behind a bearer token (`surtr token`). See `docs/api.md`.
- An agent skill for coding agents in `skills/surtr/`.

### Support
- `surtr diag`: a redacted diagnostic bundle for bug reports; nothing is
  uploaded.
- `surtr config`: the settings file; `surtr completions`: shell completion.
