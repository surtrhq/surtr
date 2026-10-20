# Surtr: rough plan

Status: draft v0.3. Everything here is a plan, not a commitment. Items marked
**(verify)** rely on third-party APIs or crates whose current state has not
been checked.

Surtr (Old Norse fire giant, said roughly "SOOR-tur") is an open source,
cross-platform hardware inspection, monitoring, stress-testing and tuning
tool for **gamers, overclocking enthusiasts, AI workstation owners and
everyday users who just want to know their PC is healthy**. It is written in
Rust. One tool replaces the LibreHardwareMonitor + CPU-Z + GPU-Z + HWiNFO +
MSI Afterburner + RivaTuner Statistics Server + CPU/GPU stress-tool toolbox,
adds the GPU-fleet and workload awareness that AI workstations need, and is
scriptable and AI-connectable from day one.

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
- **Stay modular and extensible:** a small core with replaceable parts
  (backends, workloads, exporters, data packs) and a plugin system so the
  community can add hardware support and tests without forking (section 4.1).

### Non-goals (for now)
- Competing with Cinebench/Geekbench scores or running a public leaderboard.
- BIOS flashing, firmware updates, RGB lighting control.
- A frame-rate limiter or an injected in-game overlay in early releases
  (see section 5.8 for the staged approach).
- Replacing datacenter fleet tooling (DCGM, Kubernetes GPU operators). Surtr
  targets workstations and small multi-GPU boxes, and interoperates with
  those stacks through Prometheus/OTLP.
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
7. **Modular by design.** Every vendor-, OS- or feature-specific part sits
   behind a trait or a versioned protocol, so it can be built, disabled,
   replaced or contributed independently of the core.

## 2. Audience and differentiators

Four personas, one tool with a different front door for each (see 2.2):

- **Gamer:** wants an overlay with FPS/frametimes and temperatures, and proof
  that a setup is stable. Cares about anti-cheat compatibility.
- **Enthusiast / overclocker:** overclocks or undervolts (Curve Optimizer,
  power limits, memory timings) and needs deep sensors, verified stress tests
  and safe tuning.
- **AI workstation owner:** runs local LLM inference, training, image/video
  generation or rendering on one or more GPUs. Needs VRAM, power, thermal and
  throttle visibility per GPU and per process, long-run reliability, alerts,
  and Grafana/Prometheus integration. Often headless or remote (Linux, WSL2,
  SSH).
- **Regular user:** wants a plain-language "is my PC healthy?" summary, with
  no jargon unless they ask for it.

Differentiators against existing tools:

- Open source and cross-platform (HWiNFO, CPU-Z, GPU-Z, OCCT are closed).
- Verified stress tests that name the failing core and the clock/voltage at
  the time of failure.
- A scriptable CLI/API and an MCP server (none of the incumbents has these).
- First-class export to Grafana/Prometheus for long-running tests and homelabs.
- Per-core stability map that ties tuning offsets to stress results.
- **AI-workstation awareness:** per-GPU and per-process VRAM, throttle
  reasons, PCIe/NVLink state, and correlation of application metrics (for
  example tokens per second) with hardware sensors on one timeline. Existing
  enthusiast tools don't understand AI workloads, and existing GPU-ops tools
  (nvidia-smi, nvtop, DCGM) aren't aimed at workstation users.
- Free, open equivalents of features that are limited or paid elsewhere
  (command-line logging and reports, unlimited sensor feeds, remote
  monitoring of several machines).

## 2.1 Landscape: what Surtr consolidates

Research notes (details and confidence in Appendix A):

| Tool | What it does | License / platform | What Surtr takes | What Surtr avoids |
|---|---|---|---|---|
| LibreHardwareMonitor | Sensor library (NuGet) plus WinForms GUI: temps, fans, voltages, loads, clocks for Intel/AMD CPUs, NVIDIA/AMD/Intel GPUs, boards, storage, NICs | MPL-2.0, C#, Windows | Sensor-coverage knowledge, library-first design. MPL allows file-level reuse if reused files stay MPL | Windows-only; some sensors need admin and vary by board |
| HWiNFO64 | Deepest sensor and system info, logging, reports, remote monitoring, shared-memory feed for overlays | Closed. Free for non-commercial; Pro adds command-line reports/logging, unlimited shared memory (free is capped at 12 h) and more remote hosts (5 free, 50 Pro) | The bar for sensor depth. Logging, reports and feeds as free, open features | Closed source, feature gating |
| CPU-Z | CPU, board, memory/SPD identification, small benchmark, short stress test | Closed freeware, Windows | Identification views, validation-style export | Its benchmark/stress test isn't a real stability test |
| GPU-Z | GPU details, sensors with performance-limit (throttle reason) indicators, render test, BIOS backup | Closed freeware, Windows | Throttle-reason indicators (key for AI rigs too) | BIOS dump/flash out of early scope |
| MSI Afterburner | GPU tuning: clocks, voltage, power/temp limits, V/F curve editor, OC Scanner (~30 min), fan curves, profiles; bundles RTSS for the overlay | Closed freeware, Windows | Tuning UX: curve editor, fan curves, profiles, scanner concept paired with real verification | Windows-only; scanning without verified error detection |
| RivaTuner Statistics Server | On-screen display, frametime graph, frame limiter, per-app profiles by hooking DirectX/OpenGL/Vulkan | Free, closed, Windows | Frametime view and OSD concept | Injection hooks conflict with some anti-cheat systems; deferred (5.8) |
| CPU stress tools (Prime95, OCCT, y-cruncher, Linpack variants, stress-ng) | Heavy verified compute loads | Mixed; most closed or single-purpose | Verified computation, per-core error attribution | Heat-only loads that don't detect errors |
| GPU stress tools (FurMark, OCCT GPU, gpu-burn) | Graphics and compute burn. gpu-burn: multi-GPU CUDA, memory %, FP64, Tensor cores | gpu-burn BSD-2 (NVIDIA/CUDA only); others closed | Multi-GPU compute loads, Tensor-core loads for AI cards | CUDA-only lock-in |
| nvtop | Terminal GPU monitor, many vendors (AMD, Intel, NVIDIA, limited Apple, others), per-process VRAM, PCIe RX/TX | GPL-3.0, Linux | TUI model and per-process view as a feature target | Copying code (GPL); clean-room only |
| MangoHud | Vulkan layer / OpenGL overlay with FPS, frametime, VRAM, power, logging | MIT, Linux | Linux overlay approach, interoperate or export config | Reimplementing it first |
| DCGM + dcgm-exporter | NVIDIA datacenter GPU health; Prometheus metrics on port 9400 including XID, health status and P2P/NVLink status | NVIDIA, datacenter-oriented | Metric concepts and naming for the AI dashboards, compatibility with existing Prometheus setups | Datacenter-only assumptions; consumer-GPU coverage is unverified |

Observations (our assessment, not verified claims):
1. No single tool covers inspect, monitor, stress, tune and overlay. Users
   juggle four to six tools, mostly Windows-only and mostly closed.
2. The open-source options are fragmented by OS and vendor.
3. AI workstation owners mostly use `nvidia-smi`, nvtop or DCGM with Grafana,
   not enthusiast tools, and none of those tie hardware stress, tuning and
   application metrics together.
4. Consumer stress GUIs rarely include AI-relevant loads (Tensor cores, FP16/
   BF16, sustained VRAM pressure); gpu-burn does, but only as a CLI.
5. Licensing shapes reuse: MPL-2.0 (LibreHardwareMonitor) allows file-level
   reuse, MIT/BSD (MangoHud, gpu-burn) is easy, GPL-3.0 (nvtop) must not be
   copied into Apache/MIT crates.

### Consolidation map

| Capability | Replaces | Surtr module | Milestone |
|---|---|---|---|
| Hardware identification | CPU-Z, GPU-Z, HWiNFO | `surtr-backend` + `surtr-db` | M1, M7 |
| Live sensors and throttle reasons | LibreHardwareMonitor, HWiNFO, GPU-Z, nvtop | `surtr-backend`, GUI/TUI | M2, M3 |
| Per-process GPU usage and VRAM | nvtop, `nvidia-smi` | GPU backends, GUI/TUI | M2 |
| Logging, reports, remote feed | HWiNFO (Pro features) | `surtr-store`, `surtr-export`, `surtr-api` | M4 |
| Dashboards (Grafana) | dcgm-exporter + custom setups | `surtr-export`, `dashboards/` | M4 |
| CPU/memory/GPU/storage stress | Prime95, OCCT, FurMark, gpu-burn | stress plugins | M6, M7 |
| OC controls, curve editor, fan curves | MSI Afterburner | tuning module | M9 |
| Overlay and frametimes | RTSS, MangoHud | overlay module (staged) | M12 |
| Automation and AI agents | none of the above | `surtr-api`, `surtr-mcp` | M5 |

## 2.2 Experience modes (one engine, four front doors)

- **Simple mode (regular user):** a health summary with traffic lights and
  plain-language messages ("GPU is running hot; check case airflow"), no
  jargon, no tuning controls.
- **Gaming mode:** overlay presets, frametime view, session summary after a
  game, stability check before a long session.
- **Advanced mode (enthusiast):** full sensor tree, per-core views, stress and
  tuning screens.
- **Workstation mode (AI):** per-GPU dashboard, per-process VRAM, throttle
  reasons, headless daemon plus a terminal UI, alerts, Grafana dashboards,
  application-metric adapters (5.7).

All four use the same model, backends and safety rules; modes only change what
is shown and enabled.

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

**AI workstation GPU stack.** NVIDIA through NVML (also under WSL2 and
Docker), AMD through the amdgpu sysfs interface and ROCm/AMD SMI, Intel
through the i915/xe drivers and Level Zero **(verify current interfaces and
which consumer-card metrics each exposes)**. Linux headless is first class for
this audience: daemon, terminal UI, Prometheus endpoint, no display required.
Apple Silicon gets unified-memory and power basics only.

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
    surtr-plugin-sdk/   helper crate for writing plugins in Rust (manifest, events, params)
    surtr-service/      privileged helper (Windows service / Linux daemon / launchd)
    surtr-api/          local HTTP + WebSocket API, OpenAPI schema
    surtr-mcp/          MCP server (stdio + local streamable HTTP)
    surtr-export/       Prometheus, OpenTelemetry, InfluxDB, CSV/JSON/Parquet, HTML reports
    surtr-cli/          `surtr` command, JSON output
    surtr-tui/          terminal UI for headless and SSH use (nvtop-style, multi-GPU)
    surtr-overlay/      overlay module (external window first; staged, see 5.8)
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

### 4.1 Modularity and plugin system

Surtr is a small core with replaceable parts. The aim is that a contributor
can add support for a new sensor chip, a new stress workload or a new export
target by writing one self-contained module, without touching the core and
without being able to weaken its safety rules.

**Three layers of extension, from most to least trusted:**

1. **Compile-time modules (Cargo features).** Backends, exporters, GUI,
   MCP, AI and the heavy stress dependencies (wgpu etc.) are separate crates
   behind features. This gives slim builds (for example a CLI-only
   Linux build with `--no-default-features`), easier distro packaging, and a
   smaller audit surface for privileged code.
2. **Runtime plugins (out-of-process).** A plugin is an executable that
   speaks a versioned JSON-lines protocol over stdio. Any language works.
   It can be killed, resource-limited and permission-limited by the host.
   This is the main community extension mechanism.
3. **Sandboxed script plugins (WASM, optional, later).** Pure-computation
   modules with no hardware or file access: derived sensors, alert rules,
   report templates, export formatting. Candidate runtime: `wasmtime` with
   the component model **(verify maturity)**.

In-process native plugin loading (`.dll` / `.so` via `dlopen`) is **out of
scope**: Rust has no stable ABI, a crashing plugin would take the host down,
and it would sit inside the privileged trust boundary. Crates like
`abi_stable` were considered and rejected for now.

**Extension points:**

| Extension point | Mechanism | Examples |
|---|---|---|
| Hardware backend (`Provider`) | Built-in crate, or read-only out-of-process provider plugin | New vendor sensors, board-specific embedded-controller support, community Super I/O chip maps |
| Stress / benchmark workload | Out-of-process plugin | Wrappers for external tools, custom verified kernels, game-like loads |
| Exporter / sink | Built-in, or plugin | Prometheus, OTLP, InfluxDB, MQTT, webhooks, Home Assistant |
| Derived sensors and alert rules | Declarative config, later WASM | "VRM delta", effective clock, rate-of-change alerts |
| Report templates | Data templates, later WASM | HTML layouts, forum-post summary |
| SKU and board data packs | Data files (TOML/JSON), versioned | CPU/GPU SKUs, per-board sensor labels and fan mappings |
| GUI panels | Declarative panel schema (title, metrics, chart type), not arbitrary UI code | Custom dashboards |
| MCP tools | Plugins may declare extra tools; the same scopes and limits apply | A board-specific VRM status tool |
| AI backends | Trait implementations or endpoint configuration | Local models, hosted providers |

**Plugin contract:**
- **Manifest** (`plugin.toml`, and returned by `plugin --describe`): id, name,
  version, kind, supported platforms/architectures, required capabilities,
  declared parameters and sensors, supported plugin API range, license,
  homepage.
- **Versioned protocol:** semantic versioning with a handshake that negotiates
  the version. Unknown fields are ignored. Changes within a major version are
  additive only. The API is experimental (v0) until the first stress release,
  then v1 with a documented compatibility policy.
- **Capabilities and permissions:** a plugin declares what it needs (read
  sensors, generate load, write files, use the network, needs admin). The
  user grants it; the host enforces it. Default is deny. Plugins never get
  raw privileged access, only the named operations the privileged service
  already offers.
- **Isolation:** separate process, timeouts, output-size caps, memory/CPU
  limits where the OS allows (job objects on Windows, cgroups/rlimits on
  Linux; macOS limits are weaker, so plugins there are more trusted-by-user).
- **Tuning plugins are second-class:** plugins that can write hardware
  settings are disabled by default, need explicit user trust, and still go
  through the same safety limits, confirmation and auto-revert path as
  built-in tuning. A plugin cannot raise its own limits.
- **Install and trust:** plugins live in a per-user directory
  (`surtr plugin list | install | enable | disable | remove`). Installs verify
  a checksum and, where available, a signature (minisign/sigstore style).
  Unsigned plugins show a clear warning. No automatic updates at first. A
  curated index (a git repo of manifests) is a later option; a full "store"
  is a non-goal.
- **SDK and tooling:** a `surtr-plugin-sdk` Rust crate, a published JSON
  Schema for the manifest and events, example plugins in Rust and Python, and
  `surtr plugin test`, which runs a plugin against a mock host with simulated
  limits and failures.
- **Performance boundary:** high-frequency or latency-sensitive sensor reads
  stay in built-in backends. Provider plugins are polled at a coarse interval.

**Core design rules that keep it modular:**
- Dependency direction: `surtr-model` is used by backend traits, which are used
  by `surtr-core`, which is used by API, MCP, GUI and CLI. Nothing depends on
  the GUI, and backends do not depend on each other.
- Platform `cfg` lives only in backend crates, never in core logic.
- Every module reports what it can and cannot do (`get_capabilities`), so the
  GUI, API and MCP adapt instead of assuming.
- Every heavy dependency is behind a feature flag.
- Every crate has a mock or fixture-based test and a documented, semver-ed
  public API.

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

### 5.7 AI workstation monitoring

Goal: an AI workstation owner can answer "is my machine healthy and fast
while the model runs, and if not, why?" without stitching tools together.

- **Multi-GPU dashboard:** per GPU: VRAM used/total, utilization, core and
  memory clocks, power draw versus limit, temperatures (core, hotspot, memory
  junction where available), fan speed, PCIe link generation/width and
  throughput, NVLink/peer status where present, ECC counters on cards that
  have them.
- **Throttle reasons:** why a GPU is below its potential clock (power,
  thermal, voltage, board limits), shown live and recorded in history. This
  is the single most useful signal for "why did my tokens/s drop?"
- **Per-process view:** which process holds how much VRAM and uses how much
  GPU (nvtop-style), with container and WSL2 awareness and friendly names for
  common AI runtimes.
- **Workload adapters (plugins):** scrape or receive application metrics
  (inference servers that expose Prometheus metrics, training-loop logs,
  custom scripts via a tiny HTTP/stdin push API) and show them on the same
  timeline as sensors. Example: tokens/s falling at the moment of a thermal
  throttle. First adapters to evaluate **(verify which exporters exist and
  their metric names)**: vLLM, llama.cpp server, Ollama, ComfyUI.
- **Reliability tooling:** long soak runs with Tensor-core/FP16/BF16 and
  sustained-VRAM loads, a VRAM integrity test (pattern checks for GPU memory),
  PCIe error and driver-error visibility (Linux kernel log, Windows events),
  alerts on throttling, temperature, power-cap hits and driver resets.
- **Efficiency and power management:** persistent power-limit and clock
  profiles per GPU (common practice on AI rigs), energy use per run and per
  day, an optional electricity-cost estimate, idle-power view. Writes follow
  the normal tuning safety model (5.4, section 4 safety).
- **Headless and remote:** a daemon with systemd/launchd/Windows service
  units, `surtr tui` for SSH sessions, and a simple multi-host view through
  Prometheus/Grafana (a "workstation fleet" dashboard) for people with more
  than one box.
- **Dashboards:** ship Grafana JSON for "AI workstation overview",
  "per-GPU detail", and "run timeline" with annotations for model
  load/unload and test runs.
- **Beyond GPUs (later):** NPUs and other accelerators through provider
  plugins; CPU-side signals that matter for AI (memory bandwidth, NUMA,
  swap, dataset-storage throughput).

### 5.8 Overlay and frametimes (staged)

The RTSS/MangoHud role, built in stages so the risky part comes last:

1. **Frametime capture without injection (Windows).** Use Event Tracing for
   Windows (ETW) present events, the approach Intel's PresentMon uses
   **(verify PresentMon's current license and whether to depend on it or
   reimplement)**. Gives FPS and frametime statistics with no hooks, so no
   anti-cheat conflict.
2. **External overlay window.** A transparent, always-on-top, click-through
   window for borderless/windowed games. No injection. Does not work over
   exclusive fullscreen; say so in the UI.
3. **Linux:** interoperate with MangoHud (export a MangoHud config and read
   its logs) rather than reimplement, then consider our own Vulkan implicit
   layer later.
4. **Hook-based overlay and frame limiter (Windows), optional and separate.**
   DirectX/Vulkan/OpenGL injection is where anti-cheat conflicts and
   per-API maintenance live. Ship it, if at all, as an opt-in plugin with
   clear warnings and a list of known-incompatible games.
5. **Sensor feed for third-party overlays:** a documented shared-memory or
   local-socket feed (free, no time cap) so existing overlay tools can show
   Surtr data.

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
| `read` | `get_gpu_processes` | per-process VRAM and utilization per GPU |
| `read` | `get_throttle_reasons` | why each GPU/CPU is below its potential clock, now and over a time range |
| `read` | `correlate_metrics` | align hardware sensors with application metrics (for example tokens/s) over a range |
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
| M2 | Sensors | GPU (NVML/amdgpu/ADLX) including multi-GPU, per-process VRAM and throttle reasons; Linux `hwmon`, Windows driverless sensors, SQLite history |
| M3 | GUI v1 + TUI | egui Overview, CPU, Sensors screens on all three OS; `surtr tui` multi-GPU view for headless/SSH use |
| M4 | Export | Prometheus `/metrics`, CSV/JSON export, Grafana dashboards: Overview and AI workstation (per-GPU, throttle reasons); daemon/service units |
| M5 | Local API + MCP (read-only) | `surtr-mcp` with `read` tools, auth, audit log; example agent session documented |
| M6 | Stress v1 | Plugin protocol v0 documented, host limits, verified CPU plugin built on it, run records + HTML report; MCP `run` scope |
| M7 | Stress v2 + SKU DB | Memory, storage, GPU plugins; SKU database and contribution workflow |
| M8 | Privileged backends | Service split, PawnIO (Windows) and root helpers (Linux), SMBus/SPD, AMD/Intel power |
| M9 | Tuning (opt-in) | Profiles, auto-revert, apply-and-stress, stability map; MCP `tune` scope with human confirmation |
| M10 | AI features + OTLP/Influx | Local-LLM summaries and triage, OpenTelemetry/Influx export, fleet dashboard |
| M11 | Plugin ecosystem | Plugin API v1 with compatibility policy, `surtr plugin` install/trust commands, SDK + examples + `surtr plugin test`, provider and exporter plugins, optional WASM script plugins, curated manifest index |
| M12 | Workload adapters + overlay | Application-metric adapters (inference servers, training logs) correlated with sensors; ETW frametime capture and external overlay window; MangoHud interop on Linux; third-party sensor feed |

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
- **Plugin trust and safety:** a third-party plugin can generate heavy load or
  (if allowed) change hardware settings. Mitigation: out-of-process, default
  deny, capability grants, host-enforced limits, signatures and warnings,
  tuning plugins off by default.
- **Plugin API churn:** freezing the protocol too early locks in mistakes,
  too late discourages contributors. Mitigation: experimental v0 through the
  first stress release, first-party plugins on the same protocol, then v1.
- **Modularity overhead:** many crates and feature combinations raise build
  and CI cost. Mitigation: test a small set of supported feature profiles
  (full, slim CLI, no-AI) instead of every combination.
- **Breadth of audience.** Gamers, overclockers, AI users and regular users
  want different things. Mitigation: one engine with experience modes (2.2),
  and a clear priority order per milestone rather than building for everyone
  at once.
- **Anti-cheat and overlays.** Injected overlays can trigger anti-cheat.
  Mitigation: no injection until late, ETW capture and external window first,
  documented compatibility.
- **Consumer versus datacenter GPU metrics.** Some counters (ECC, NVLink
  error detail, XID events) are datacenter-oriented. Mitigation: report what
  each GPU exposes through `get_capabilities` instead of assuming.
- **GPL contamination.** Reference tools such as nvtop are GPL-3.0. Mitigation:
  reimplement from public interfaces (NVML, sysfs), never copy code.
- **AI-metrics adapters depend on third-party exporters** that change often.
  Mitigation: adapters are plugins with their own versions.
- Open questions: minimum OS versions; whether WASM script plugins are worth
  their complexity compared with declarative rules; native Rust GPU stress approach
  (wgpu vs vendor APIs); whether to build a Windows service or a scheduled
  elevated helper first; whether to support remote/fleet agents in v1.

## 12. Next steps

1. Pick the license and add LICENSE, README, SECURITY.md, CODE_OF_CONDUCT.
2. Create the workspace and `surtr-model` + `Provider` trait (M0).
3. Move the egui prototype into `prototype/` and keep it as a UI reference.
4. Set up CI on the 3-OS matrix.
5. Verify the crates and services flagged **(verify)**: `rmcp`, PawnIO
   terms and module coverage, vendor GPU APIs and their Rust bindings,
   ROCm/AMD SMI and Intel Level Zero coverage, PresentMon licensing, and the
   exporters of the AI runtimes listed in 5.7.

## Appendix A: landscape research notes and confidence

Gathered 2026-10-20. Several vendor pages blocked automated fetching, so
some rows rest on secondary sources. Treat this as a starting point.

Read directly from the project's own page:
- LibreHardwareMonitor (GitHub): MPL-2.0, C#, Windows Forms GUI plus NuGet
  library, hardware list, admin-required sensors, board-dependent accuracy.
  It did not state how it reads hardware.
- nvtop (GitHub): GPL-3.0, vendor list, per-process memory, PCIe RX/TX, Linux.
- MangoHud (GitHub): MIT, Linux, Vulkan layer and OpenGL hooking, metrics
  vary by driver/kernel.
- gpu-burn (GitHub): BSD-2-Clause, CUDA/NVIDIA, multi-GPU, options for memory
  share, double precision, Tensor cores. The page did not describe how it
  verifies errors.
- OCCT (landing page only): described as a free all-in-one stability, stress,
  benchmark and monitoring tool; test types and licensing terms not confirmed.

From secondary sources (official pages were unreachable):
- HWiNFO: free for non-commercial use (64-bit and ARM64), Pro adds
  command-line reports/logging and lifts shared-memory (12 h) and remote
  (5 vs 50) limits. Some sites misstate the commercial terms; check the
  vendor's license page before relying on this.
- CPU-Z: identification, a simple benchmark and stress test (sources disagree
  on how capable the stress test is). GPU-Z: details, sensors with
  performance-limit indicators, render test, BIOS backup.
- MSI Afterburner: clocks, voltage, power/temp limits, curve editor, OC
  Scanner (about 30 minutes), fan curves, bundled RTSS. AMD support not
  confirmed.
- RTSS: free, DirectX/OpenGL/Vulkan, frametime graph, frame limiter,
  per-app profiles; anti-cheat can restrict it.
- DCGM/dcgm-exporter: Prometheus on port 9400, XID, health and P2P metrics;
  exporter scrape interval can miss short events. Consumer-GPU coverage and
  NVLink error counters were not confirmed.

Not researched: nvitop, y-cruncher, Prime95, FurMark, Unigine and 3DMark
details, PresentMon, AMD SMI/ROCm and Intel Level Zero specifics.
