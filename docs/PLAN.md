# Surtr: rough plan

Status: draft v0.1. Everything here is a plan, not a commitment. Items marked
**(verify)** rely on third-party APIs or crates whose current state has not
been checked.

Surtr (Old Norse fire giant, said roughly "SOOR-tur") is an open source,
cross-platform hardware inspection, monitoring, stress-testing and tuning
tool for enthusiasts and gamers who overclock. It is written in Rust. One
tool replaces the CPU-Z + GPU-Z + HWiNFO + stress/benchmark + tuning-utility
toolbox, and it is scriptable and AI-connectable from day one.

## 1. Goals and non-goals

### Goals
- **Inspect:** accurate CPU, GPU, memory, storage and board information
  (the CPU-Z / GPU-Z side), backed by a community-maintained SKU database.
- **Monitor:** live and historical sensors (clocks, temps, power, voltages,
  fans, usage) with low overhead.
- **Verify:** stress and stability tests that detect *errors*, not just heat.
- **Tune (opt-in, later):** profiles for power limits, per-core offsets, fan
  curves and memory settings, always with an auto-revert safety net.
- **Report:** exports, shareable run reports, and observability integrations
  (Prometheus, OpenTelemetry, Grafana dashboards).
- **Automate:** a local API and an **MCP server** so scripts and AI agents can
  read state and run tests, with write access gated by explicit consent.
- **Run everywhere:** Windows, Linux and macOS on x86-64 and ARM64; Intel,
  AMD and ARM CPUs; NVIDIA, AMD, Intel and Apple GPUs.

### Non-goals (for now)
- Competing with Cinebench/Geekbench scores or running a public leaderboard.
- BIOS flashing, firmware updates, RGB lighting control.
- Anything that needs a custom signed kernel driver in the first releases.
- Fully feature-equal support on every OS. Capability differs by platform and
  the UI must say so honestly (see section 3).

### Principles
1. **Read-only first.** Inspection and monitoring ship before any write path.
2. **Never fake data.** Every value carries its source and may be absent.
3. **The host enforces safety.** Limits and aborts live in Surtr, not in
   plugins or in whatever caller (human, script, AI) asked for the run.
4. **Least privilege.** The GUI/CLI run unprivileged. A small service holds
   elevated access and exposes only narrow, named operations.
5. **Local-first, private by default.** No telemetry. No data leaves the
   machine unless the user configures an exporter or an AI endpoint.
6. **Tuning is separate from inspection** in code, UI and permissions.

## 2. Audience and differentiators

Enthusiasts who overclock or undervolt (Curve Optimizer, power limits, memory
timings), and gamers who want to prove a setup is stable. Differentiators
against existing tools:

- Open source and cross-platform (HWiNFO, CPU-Z, GPU-Z, OCCT are closed).
- Verified stress tests that name the failing core and the clock/voltage at
  the time of failure.
- A scriptable CLI/API and an MCP server (none of the incumbents has these).
- First-class export to Grafana/Prometheus for long-running tests and homelabs.
- Per-core stability map that ties tuning offsets to stress results.

## 3. Platform and hardware support

| Area | Windows x64 / ARM64 | Linux x64 / ARM64 | macOS Apple Silicon / Intel |
|---|---|---|---|
| CPU identity (CPUID / sysctl / DT) | Yes (ARM64: via Windows registry + SMBIOS, no CPUID) | Yes (ARM64: `/proc/cpuinfo`, device tree, sysfs) | Intel: CPUID. Apple Silicon: sysctl, no CPUID |
| Clocks, load, topology | PDH / WMI | `/proc`, `cpufreq` sysfs | sysctl, `host_statistics` |
| Temps, fans, voltages | Driverless subset; full via optional privileged backend | `hwmon` (mostly driverless) | SMC / IOReport (partial, Apple Silicon limited) |
| Power (RAPL etc.) | Intel/AMD via privileged backend | `powercap`/RAPL, `hwmon`, may need root | `powermetrics` needs root; limited |
| GPU | NVML, ADLX, Intel IGCL | NVML, amdgpu sysfs, i915/xe | Metal (few sensors) |
| Memory timings / SPD | Privileged backend (SMBus) | `i2c-dev` + root | Mostly unavailable |
| Stress test | Yes | Yes | Yes |
| Tuning writes | Privileged backend, per vendor | sysfs / MSR with root | Not supported |

Notes:
- **ARM has no CPUID.** Identification and feature flags come from the OS and
  firmware tables. The model layer must not assume x86 fields exist.
- **Apple Silicon** exposes far less than x86 desktops. Label it "basic
  support" and do not chase parity.
- **Windows on ARM** (Snapdragon X etc.) sensor access is limited and may
  depend on OEM interfaces. Treat as best-effort until hardware is available.
- **Vendor differences:** Intel (RAPL, MSR), AMD (Zen SMU, CPPC, Curve
  Optimizer), ARM server parts (vendor sysfs, SCMI) each get their own
  backend module behind the same traits.

Minimum supported versions to decide at M0: Windows 10/11, a recent Linux
kernel (for `hwmon`/amdgpu), macOS 13+.

## 4. Architecture

Cargo workspace. The core never calls OS APIs directly: it talks to backend
traits, and every field is `Option<T>` with a source tag.

```
surtr/
  crates/
    surtr-model/        plain typed data (Cpu, Gpu, Memory, Sensor, Run, ...) + serde, no I/O
    surtr-backend/      Provider trait + backends
      windows/ linux/ macos/        per-OS code
      vendor/{intel,amd,nvidia,apple,arm}/   per-vendor code
    surtr-db/           SKU database (TOML/JSON) + matcher by CPUID / PCI ID / DMI
    surtr-store/        SQLite history, retention, run records
    surtr-core/         polling loop, event bus, safety limits, plugin host
    surtr-plugin-api/   plugin manifest + event schema (serde, no I/O)
    surtr-service/      privileged helper (Windows service / Linux daemon / launchd)
    surtr-api/          local HTTP + WebSocket API, OpenAPI schema
    surtr-mcp/          MCP server (stdio + local streamable HTTP)
    surtr-export/       Prometheus, OpenTelemetry, InfluxDB, CSV/JSON/Parquet, HTML reports
    surtr-cli/          `surtr` command, JSON output
    surtr-gui/          egui app (consumes surtr-model only)
  plugins/
    stress-cpu/  stress-mem/  stress-gpu/  stress-storage/   built-in plugins (separate binaries)
  dashboards/           Grafana dashboard JSON + provisioning examples
  fixtures/             recorded /sys trees, WMI dumps, SMBIOS blobs for tests
  docs/
  prototype/            egui throwaway prototype (moved from hwx-proto)
```

### Process model
```
GUI / CLI / API clients / MCP clients   (unprivileged)
              |
        surtr-core  ---- plugin processes (stress workloads, killable)
              |
        surtr-service   (privileged, narrow named operations only)
              |
   OS drivers / PawnIO (Windows) / hwmon, msr, i2c (Linux) / SMC (macOS)
```

### Privileged access (Windows)
Driverless backends first (CPUID, SMBIOS tables, PDH/WMI, SMART via
`DeviceIoControl`, NVML/ADLX/IGCL). Then an **optional PawnIO backend** for
MSR, SMBus and Super I/O **(verify current license, API and Zen 4/5 module
coverage; PawnIO driver is GPL-2.0 with an IOCTL exception, modules LGPL-2.1,
installer terms unclear)**. Our own signed driver is a last resort. Whatever
the backend, the service exposes high-level operations ("read package power"),
never raw MSR or physical-memory access, and checks the caller.

### Plugins
- **Separate processes**, not dynamic libraries: Rust has no stable ABI, and a
  stress test is expected to crash or hang the machine.
- Protocol: `plugin --describe` returns a JSON manifest (name, targets,
  parameters, required privileges). The host runs it with parameters. The
  plugin streams JSON lines (progress, per-worker status, errors found). The
  host owns sensors and limits and kills the plugin on a limit or user stop.
- First-party plugins use the same protocol as third-party ones.
- Wrapping external tools (stress-ng, y-cruncher, etc.) is supported as
  user-installed plugins; we do not bundle tools whose licenses forbid it.

### Data and storage
- Live: in-memory ring buffers per sensor, 1 s default poll, configurable.
- History: SQLite (downsampled rollups plus raw for recent window and for
  stress runs). Retention policy in settings.
- Run records: sensor timeline, plugin events, errors, config, hardware
  snapshot, versions. Exportable as a versioned JSON schema.

### Safety model
- Limits (temperature, power, voltage ceilings) enforced by the host for every
  run, from every entry point.
- Tuning writes require: tuning enabled in settings, explicit confirmation,
  and an **auto-revert timer** (default 60 s) unless confirmed.
- Hard floors/ceilings per platform that no caller can override.
- Audit log of every write and every test start, with the caller identity.

## 5. Feature areas

### 5.1 Inspect (CPU-Z / GPU-Z side)
CPU (name, codename, package, process, cores/threads, caches, ISA flags,
microcode), memory (type, speed, timings where available), GPU (adapter,
VRAM, clocks, BIOS where available), storage (SMART/NVMe health), board and
firmware. SKU database in-repo with a contribution workflow and CI that
validates entries.

### 5.2 Monitor
Sensor graph dashboard, per-core tiles with preferred-core ranking, alerts
(threshold, rate-of-change), overlay feed for games (stretch), tray mode.

### 5.3 Stress and benchmark
- CPU: verified FFT/integer/AVX2/AVX-512 (where present), per-core pinning,
  known-answer checks. Report the failing core, clock and voltage at failure.
- Memory: pattern and walking-bit tests in the OS; recommend memtest86+ for
  boot-time confirmation.
- GPU: verified compute (wgpu/Vulkan compute first; CUDA/OpenCL optional) and
  a separate graphics load.
- Storage: write/read with verification.
- Benchmarks as plugins: simple integer, float, memory bandwidth, GPU
  compute. Results are exportable and comparable offline; no online
  leaderboard in scope.

### 5.4 Tune (opt-in, later)
Profiles (Daily, Gaming, Quiet, Max OC), per-core offsets, power limits, fan
curves, boost override, apply-with-auto-revert, apply-and-stress, stability
map per core. Linux and Windows first; macOS read-only.

### 5.5 Reporting and observability
- **Prometheus exporter** (`/metrics`, OpenMetrics). Scrapeable by Grafana
  Agent/Alloy or Prometheus. Low-cardinality labels (host, device, sensor).
- **OpenTelemetry (OTLP) metrics** export to any collector.
- **InfluxDB line protocol** push for Influx/Telegraf setups.
- **Grafana:** ship importable dashboard JSON (`dashboards/`): Overview,
  Per-core, Stress run timeline, Fleet (many hosts). Provisioning examples.
  Event annotations for stress start/stop, errors and profile changes.
- **Files:** CSV, JSON, Parquet export; self-contained HTML run report with
  charts; versioned `.surtr.json` result file.
- **Optional integrations (later):** MQTT / Home Assistant, webhooks, and
  Discord/Slack notification on test failure.

### 5.6 Automation: local API
`surtr-api` serves REST + WebSocket on localhost only (off by default,
token-protected). It is the single backend that the GUI, CLI and MCP server
can share, so permissions and safety are enforced in one place.

## 6. MCP and AI connectivity

### 6.1 MCP server (`surtr-mcp`)
Transports: stdio (launched by an MCP client) and local streamable HTTP
(loopback, bearer token). Rust SDK: the official `rmcp` crate **(verify
maturity and spec version)**.

**Tools** (grouped by permission scope):

| Scope | Tool | Purpose |
|---|---|---|
| `read` | `get_system_info` | CPU/GPU/memory/storage/board snapshot |
| `read` | `get_sensors` | current values, filterable by device/sensor |
| `read` | `query_history` | time range, downsampling, aggregation |
| `read` | `list_stress_plugins` | available workloads and their parameters |
| `read` | `get_run`, `list_runs`, `compare_runs` | run records and diffs |
| `read` | `get_capabilities` | what this machine/OS can and cannot do |
| `run` | `start_stress_test` | start a verified run with parameters and limits |
| `run` | `stop_stress_test`, `get_run_status` | control and poll |
| `tune` | `preview_profile` | dry-run: shows what would change |
| `tune` | `apply_profile` | requires human confirmation (see below) |

**Resources:** `surtr://system`, `surtr://sensors/live`, `surtr://runs/{id}`,
`surtr://profiles/{name}`, `surtr://capabilities`. **Prompts:** "diagnose
instability", "plan an undervolt session", "summarize this run".

**Security rules (non-negotiable):**
- Default scope is `read`. `run` and `tune` are separate opt-ins in settings
  and can be limited to a time window.
- Host-enforced safety limits apply to every call; a caller cannot raise them
  beyond configured ceilings.
- Writes (`apply_profile`) require out-of-band human confirmation (GUI prompt
  or MCP elicitation); an AI client cannot approve its own change.
- Everything returned is data, never instructions; tool descriptions are
  static; sensor and plugin text is escaped.
- Loopback only, bearer token, per-client scopes, rate limits, audit log.
- Never expose identifiers (serial numbers, MAC addresses, hostnames) unless
  the user opts in; redaction option for shared reports.

### 6.2 AI features
All off by default; each needs an explicit opt-in and an endpoint choice.
- **Run summaries and explanations:** turn a run record into plain language
  ("core 7 failed Large FFT at 07:12 at 5.18 GHz / 1.284 V; likely needs +5
  curve offset").
- **Instability triage:** correlate errors with throttling, clock stretching,
  power-limit hits and OS events (WHEA on Windows, MCE on Linux).
- **Guided tuning assistant:** proposes small stepwise plans, each followed by
  a stress run, never applying anything without confirmation.
- **Model backends:** local first (Ollama / llama.cpp-compatible endpoints),
  bring-your-own-key for hosted providers. Only the redacted run record is
  sent, and the user can preview exactly what would be sent.
- **Agent use via MCP:** the same tools above let external agents (Claude,
  etc.) do the analysis without Surtr embedding any model.

## 7. UI

egui for the first GUI (fast, good for live graphs, pure Rust); the GUI only
consumes `surtr-model` so the toolkit can be swapped (Slint, Tauri, Qt) without
touching backends. Screens: Overview, CPU, GPU, Memory, Storage, Sensors,
Stress Test, Tuning, Benchmarks, Reports, Settings (API/MCP/exporters/AI).
Dark theme, one accent color, dense and numeric. Mockups and a working egui
Overview prototype exist from the exploration phase and move into
`prototype/`.

## 8. Packaging, CI, testing

- **Targets:** `x86_64`/`aarch64` for Windows MSVC, Linux (glibc; musl for the
  CLI), macOS. Universal macOS binary or two builds.
- **Distribution:** winget + MSI/zip, Homebrew cask, AppImage/Flatpak/deb/rpm,
  `cargo install` for the CLI. Release automation with `cargo-dist`.
  macOS notarization and Windows code signing need certificates (budget).
- **CI:** GitHub Actions matrix: windows-latest, ubuntu-latest (x64 + ARM64
  runner), macos (Apple Silicon + Intel). `cargo fmt`, `clippy`, tests,
  `cargo deny` (licenses/advisories), `cargo audit`.
- **Testing without the hardware:** mock `Provider` plus recorded fixtures
  (saved `/sys` trees, WMI/SMBIOS dumps). Hardware-in-the-loop on a few real
  machines later (Intel, AMD, ARM Linux, Apple Silicon).
- **Fuzz** parsers for SMBIOS/DMI and the plugin protocol.

## 9. Licensing and governance

Decide before first public release:
- **Recommended:** Apache-2.0 OR MIT for crates, so others can embed the
  library. Check compatibility with the PawnIO exception and LGPL modules if
  that backend is used (talk to the PawnIO maintainer about bundling).
- SKU database and dashboards: CC0 or the same license as the code.
- Contribution flow: DCO sign-off, templates for SKU entries and sensor
  mappings, CODE_OF_CONDUCT, SECURITY.md (privileged code needs a clear
  vulnerability-reporting path).
- Trademark: avoid vendor names in the product name and branding; do not
  imply vendor affiliation. Run a proper trademark search for "Surtr" before
  a public launch (a web search found no conflict; not a clearance).

## 10. Milestones

| # | Milestone | Exit criteria |
|---|---|---|
| M0 | Foundations | Workspace, `surtr-model`, `Provider` trait, mock provider, CI matrix on 3 OS, license chosen |
| M1 | Inspect (driverless) | `surtr --json` prints CPU/memory/OS on Windows + Linux + macOS; ARM64 builds run |
| M2 | Sensors | GPU (NVML/amdgpu/ADLX), Linux `hwmon`, Windows driverless sensors, SQLite history |
| M3 | GUI v1 | egui Overview, CPU, Sensors screens on all three OS |
| M4 | Export | Prometheus `/metrics`, CSV/JSON export, first Grafana dashboard |
| M5 | Local API + MCP (read-only) | `surtr-mcp` with `read` tools, auth, audit log; example agent session documented |
| M6 | Stress v1 | Plugin protocol, host limits, verified CPU plugin, run records + HTML report; MCP `run` scope |
| M7 | Stress v2 + SKU DB | Memory, storage, GPU plugins; SKU database and contribution workflow |
| M8 | Privileged backends | Service split, PawnIO (Windows) and root helpers (Linux), SMBus/SPD, AMD/Intel power |
| M9 | Tuning (opt-in) | Profiles, auto-revert, apply-and-stress, stability map; MCP `tune` scope with human confirmation |
| M10 | AI features + OTLP/Influx | Local-LLM summaries and triage, OpenTelemetry/Influx export, fleet dashboard |

M0 to M6 is the first public-worthy milestone: a cross-platform inspector and
monitor with verified CPU stress testing, Grafana export, and an MCP server.

## 11. Risks and open questions

- **Scope creep.** Mitigation: milestones above, read-only first, plugins.
- **Privileged code and driver trust** (signing, HVCI, blocklists, security
  advisories). Mitigation: narrow service API, PawnIO, security policy.
- **SKU database is a long, manual job.** Mitigation: contributor-friendly
  format, start with the most common CPUs/GPUs.
- **Apple Silicon and Windows on ARM sensor access** may remain thin.
- **GPU API fragmentation** (NVML/ADLX/IGCL licensing and Rust bindings).
- **AI/MCP safety:** an agent starting stress tests or tuning a machine is a
  real hazard. Mitigation: scopes, host-enforced limits, human confirmation,
  audit log, writes off by default.
- **Hardware access for testing** across Intel, AMD, ARM, Apple. Need a plan
  for test machines or community testers.
- Open questions: minimum OS versions; native Rust GPU stress approach
  (wgpu vs vendor APIs); whether to build a Windows service or a scheduled
  elevated helper first; whether to support remote/fleet agents in v1.

## 12. Next steps

1. Pick the license and add LICENSE, README, SECURITY.md, CODE_OF_CONDUCT.
2. Create the workspace and `surtr-model` + `Provider` trait (M0).
3. Move the egui prototype into `prototype/` and keep it as a UI reference.
4. Set up CI on the 3-OS matrix.
5. Verify the crates and services flagged **(verify)**: `rmcp`, PawnIO
   terms and module coverage, vendor GPU APIs and their Rust bindings.
