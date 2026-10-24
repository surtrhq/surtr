# Changelog

All notable changes to Surtr are listed here. Surtr follows
[Semantic Versioning](https://semver.org/); before 1.0, minor versions may
change commands and JSON output.

## [Unreleased]

### Added
- `surtr tui` has a Stress tab: choose the test (mixed, SIMD, cache or
  memory), all cores or one at a time, threads, SMT, length and load
  pattern; see what will run and where it stops, confirm, watch the flames,
  stop with `s`. The same limits and records as `surtr stress cpu`;
  quitting during a run stops it first.
- In the same tab: past runs (`r`), each run's report, and comparing two
  (`c` on one, then `c` on the other); and AI agents' stress requests,
  shown as they arrive, to approve (`a`, then `y`) or deny (`d`) without
  typing the code.
- AMD GPUs on Linux report throttle reasons (`surtr gpu throttle`, the API,
  MCP and `/metrics`), read from the kernel's `gpu_metrics` table. Going by
  the kernel source, that covers Radeon RX 5000 to RX 9000, Instinct MI100
  and MI200, and the Van Gogh and Renoir APUs; not yet checked on real
  hardware (#8). GPUs whose table leaves the status out stay `unsupported`.
- Two new throttle reasons: `current_limit` (TDC or EDC) and `other` (a
  limit the GPU reports that Surtr doesn't classify).
- A Claude Code plugin: `claude plugin marketplace add surtrhq/surtr`, then
  `claude plugin install surtr@surtr`, adds the skill and the MCP server in
  one step. It runs the `surtr` on your `PATH`; install that first (#63).
- Windows releases include an MCP Bundle (`.mcpb`) for Claude Desktop:
  open it to install the MCP server with its binary (#63). Not for macOS
  yet, where Claude Desktop drops the binary's executable bit
  (modelcontextprotocol/mcpb#294).

### Changed
- `software_thermal` now reads "thermal limit (driver or firmware)": on AMD
  the GPU firmware applies temperature limits.
- A sensor that gave no valid value in one read now shows "no reading"
  instead of "not available" (`surtr sensors`, `surtr watch`, the TUI), so
  a momentary gap no longer looks like a missing feature. On Windows this is
  the per-thread load Windows itself marks invalid now and then (#10); Surtr
  still leaves those out rather than guess.

## [0.3.0] - 2026-10-23

The stress tests tuners use: one core at a time for per-core tuning, a
load that switches on and off for idle-to-boost crashes, SIMD for the
highest power draw, and cache- and memory-sized tests for the memory
controller. All verified, all under the same limits; tested on a Ryzen 5
3500U and a hybrid Intel CPU.

### Added
- `surtr stress cpu --per-core [--each 1m] [--siblings]`: one physical core
  at a time, each in turn, with a result, peak clock, throughput and peak
  temperature per core; finds the weak core in per-core tuning (Curve
  Optimizer, per-core undervolts). Hybrid CPUs show P- and E-cores.
- `surtr stress cpu --kernel simd`: fused multiply-add on wide vectors
  (AVX-512, AVX2 or NEON, picked at run time), running the FMA units at
  their peak for the highest power draw; the report names the instruction
  set used.
- `surtr stress cpu --kernel cache` (an FFT that stays in L2, like Prime95
  small FFT) and `--kernel memory` (a RAM-sized FFT plus a memory pattern
  test, about 50 MiB per thread, for the memory controller and fabric, like
  Prime95 large FFT). Every result is still checked.
- `surtr stress cpu --pattern transient`: load switching on and off, all
  threads together, with gaps long enough for deep idle; catches crashes at
  idle-to-boost transitions that steady load never triggers. Works with
  `--per-core`.

### Fixed
- `surtr tui` over SSH: moving the mouse no longer switches tabs. The TUI
  asks for clicks and the wheel only, and rebuilds key and mouse sequences
  that arrive in pieces instead of reading them as keys.

## [0.2.0] - 2026-10-23

Stress testing arrives: a verified CPU stress test with host-enforced
limits, a live flame view, run records with crash evidence, run comparison,
and stress runs that AI agents can request and a person confirms. Verified
on a Ryzen 5 3500U; on Windows it waits for a CPU temperature sensor (M5c),
except for a 60-second `--unmonitored` check.

### Added
- `surtr stress cpu`: a verified CPU stress test. Every thread repeats
  computations with known answers, and any wrong answer is reported with
  its CPU and clock. It runs in a separate worker process, stops at 95 °C
  (or 5 °C below the CPU's own critical limit) and after 30 minutes at
  most (`[stress]` in config.toml), and refuses to run without a CPU
  temperature sensor unless `--unmonitored` (60 seconds at most).
- `surtr stress cpu` shows the run live as one flame per CPU thread: height
  is the thread's throughput (throttling makes it drop), color is the
  temperature against the limit, a thread with a wrong answer turns to ash.
  `--plain`, `--json`, `NO_COLOR` or a non-terminal give a one-line status.
- `surtr runs compare` and the MCP `compare_runs` tool: two runs side by
  side (throughput, temperatures, throttling sag, errors), with warnings
  when the runs aren't comparable.
- Stress threads go one per physical core before any SMT sibling is used;
  `--cpus 0,2,5` picks exact CPUs and `--no-smt` keeps to one thread per
  core. Runs record their CPUs.
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
