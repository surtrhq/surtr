# Surtr: rough plan

Status: draft v0.5. Everything here is a plan, not a commitment. Items marked
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
- **CLI parity with the GUI:** anything the GUI can show or run, the CLI can
  show (as styled text, live terminal charts or JSON) or run, as far as the
  medium allows. Exceptions are listed explicitly in section 7.2.
- **Modern, slick interfaces everywhere:** the GUI, the TUI and the CLI's
  human output are all held to one visual and interaction quality bar (section
  7.1), not just the GUI.

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
8. **Front ends are thin.** Every capability is a core/API operation first;
   the GUI, TUI, CLI, API and MCP only present and invoke it. A feature that
   exists only inside the GUI is a bug in the design, because it can't be
   scripted, run over SSH or reached by an agent.

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
| Logging, reports, remote feed | HWiNFO (Pro features) | `surtr-store`, `surtr-export`, `surtr-api` | M4, M5 |
| Dashboards (Grafana) | dcgm-exporter + custom setups | `surtr-export`, `dashboards/` | M4 |
| CPU/memory/GPU/storage stress | Prime95, OCCT, FurMark, gpu-burn | `surtr-stress` (built in); extra tests as plugins | M6, M7 |
| OC controls, curve editor, fan curves | MSI Afterburner | tuning module | M9 |
| Overlay and frametimes | RTSS, MangoHud | overlay module (staged) | M12 |
| Automation and AI agents | none of the above | `surtr-api`, `surtr-mcp` | M5 |

## 2.2 Experience modes (one engine, four front doors)

- **Simple mode (regular user):** a health summary with traffic lights and
  plain-language messages ("GPU is running hot; check case airflow"), no
  jargon, no tuning controls. Traffic-light thresholds come from limits the
  vendor or OS reports (for example Tjmax), with conservative generic defaults
  until the SKU database (M7) and privileged reads (M5c) exist.
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
    surtr-backend/      Provider trait + capability types (no OS or vendor code)
    surtr-backend-{windows,linux,macos}/            per-OS backends, separate crates
    surtr-backend-{intel,amd,nvidia,apple,arm}/     per-vendor backends, separate crates
                        (each vendor binding crate carries its own LICENSE/NOTICE)
    surtr-db/           SKU database (TOML/JSON) + matcher by CPUID / PCI ID / DMI
    surtr-store/        SQLite history, retention, run records
    surtr-core/         polling loop, event bus, safety limits, plugin host, stress runner
    surtr-stress/       built-in stress tests and benchmarks (`Workload` trait, verified
                        kernels, per-core pinning); heavy deps (wgpu) behind features
    surtr-plugin-api/   plugin manifest + event schema (serde, no I/O); also the run-event
                        schema that built-in tests emit, so both look the same to the host
    surtr-plugin-sdk/   helper crate for writing plugins in Rust (manifest, events, params)
    surtr-service/      privileged helper (Windows service / Linux daemon / launchd)
    surtr-api/          local HTTP + WebSocket API, OpenAPI schema
    surtr-mcp/          MCP server (stdio + local streamable HTTP)
    surtr-export/       Prometheus, OpenTelemetry, InfluxDB, CSV/JSON/Parquet, HTML reports
    surtr-view/         front-end-neutral view models (screens, tables, chart series,
                        health summary, plain-language messages) + shared theme tokens;
                        used by GUI, TUI and CLI so they cannot drift apart (see 7)
    surtr-cli/          `surtr` command: styled human output, JSON/NDJSON, full GUI parity
    surtr-tui/          terminal UI for headless and SSH use (nvtop-style, multi-GPU)
    surtr-overlay/      overlay module (external window first; staged, see 5.8)
    surtr-gui/          GUI (toolkit decided at M5b; consumes surtr-view and the API client only)
  plugins/
    examples/           example third-party-style plugins (Rust, Python) for the SDK
  dashboards/           Grafana dashboard JSON + provisioning examples
  fixtures/             recorded /sys trees, WMI dumps, SMBIOS blobs for tests
  docs/
  prototype/            egui throwaway prototype (moved from hwx-proto)
```

This is the target shape, not the starting point. A solo developer starts M0
with about six crates (model, backend traits, core, store, stress, cli+tui) and
splits further only when a boundary is proven (a different dependency set,
license, privilege level or owner).

### Process model
```
GUI / TUI / CLI / MCP clients           (unprivileged, per user)
              |   local IPC (remote access = SSH-forwarded loopback port)
        surtr serve     per-user daemon, unprivileged: core, polling, store,
              |         API, MCP, exporters, alerts, tray
              |  ---- worker processes: built-in stress tests (`surtr worker`)
              |       and plugin processes, both killable by the host
              |
        surtr-service   privileged helper: no UI, no network, no plugins;
              |         narrow named operations, hard ceilings, revert state;
              |         accepts only the daemon of the installing user
              |
   OS drivers / PawnIO (Windows) / hwmon, msr, i2c (Linux) / SMC (macOS)
```
Three kinds of process: clients, the per-user daemon, and the privileged
helper. The tray, alerts and API live in the daemon, never in the helper (a
Windows service runs in session 0 and cannot show a tray, and UI in a
privileged process widens the surface). Front ends are clients of the daemon.
The CLI's "embedded mode" runs the same daemon code in-process with the same
limits and refuses to start if another instance holds the hardware lock.

### Privileged access (Windows)
Driverless backends first (CPUID, SMBIOS tables, PDH/WMI, SMART via
`DeviceIoControl`, NVML/ADLX/IGCL). Then an **optional PawnIO backend** for
MSR, SMBus and Super I/O. Our own signed driver is a last resort. Whatever
the backend, the service exposes high-level operations ("read package power"),
never raw MSR or physical-memory access, and checks the caller. The backend
sits behind a trait so another driver can replace PawnIO.

**Service authorization and IPC.** The service accepts connections only from
the daemon running as the user who installed it (Windows named pipe with an ACL
for that user's SID; Linux Unix socket with a peer-credential check; macOS XPC
with code-signature check) **(verify each)**. Plugins and workers have no
handle to it and can reach it only through the daemon. The IPC has a version
handshake between clients, daemon and service; the service holds hard
ceilings and revert state so a client that bypasses the core still cannot
exceed them (see Safety model). The local API token lives in the OS keychain or
a user-only file. **Hardware access lock:** only one Surtr instance touches
SMBus, Super I/O and PCI at a time, and we look for the global mutexes other
tools (HWiNFO, LibreHardwareMonitor) use for the same buses so we do not
collide with them **(verify names and behavior)**.

**What works without any driver (Windows).** CPU identity (CPUID), topology and
caches; load and OS-reported clocks (not the true effective clock, which needs
APERF/MPERF); SMBIOS memory modules, board and BIOS; storage SMART and NVMe
health including temperature; all GPU telemetry and GPU tuning through NVML,
ADLX and IGCL (admin, not a driver); ETW frametimes; power-plan and boost
settings; the whole stress suite; possibly Intel package power through Windows
Energy Meter counters **(verify, including AMD)**.

**What needs the privileged backend.** AMD CPU temperature, voltage and power
(SMN/SMU); Intel per-core temperature, VID and effective clock (MSR);
motherboard fans, voltages and fan control (Super I/O); SPD, XMP/EXPO and
memory timings (SMBus); CPU offsets and any MSR write. Memory timings are
normally BIOS settings and stay out of scope. Without the backend the UI shows
these as "needs privileged mode" through `get_capabilities`, never as blanks.

**PawnIO (researched 2026-10-20; terms must be re-read before M8).**
- *What it is:* a kernel driver that loads small Pawn "modules" doing the
  hardware access, so callers get named operations instead of raw register
  access. The official `PawnIO.Modules` repo includes Intel/AMD/Zhaoxin MSR,
  AMD family 0F/10/17 and RyzenSMU, SMBus (I801, PIIX4, NCT6793, Skylake IMC),
  `LpcIO` (Super I/O), ACPI/EC, PCH thermal and others. Both repos were active
  in 2026, and LibreHardwareMonitor uses it in place of WinRing0.
- *Licenses:* driver GPL-2.0-or-later with a special exception for LGPL
  programs and for independent modules that talk to it solely through the
  device IOCTL interface; `PawnIOLib` (the wrapper) LGPL-2.1-or-later; official
  modules LGPL-2.1. Modules loaded into the driver must be compatible with the
  driver's license (LGPL-2.1 recommended by the author).
- *Signing:* the author publishes an "Official" signed build and an
  "Unrestricted" unsigned build "without signature checking". Our reading
  (unconfirmed) is that the official build only loads modules the author has
  signed. **(verify; ask the author)**
- *Distribution:* pawnio.eu states no redistribution policy for other apps and
  provides the software "as is".
- *Decision:* **guide, don't bundle.** Surtr detects PawnIO, explains why it is
  optional, links to the official download, and shows a clear "not installed"
  state. Because the user installs the driver, we do not distribute it and its
  GPL terms do not attach to Surtr's own crates. Surtr stays Apache-2.0 OR MIT.
- *No third-party binaries ship with Surtr (project policy).* We bundle no
  driver, no PawnIO modules, no `PawnIOLib` DLL and no vendor libraries. Modules
  come from the user's PawnIO install or from the official release the user
  downloads; the IOCTL client is our own small Rust code (or a library the
  user's install provides) **(verify what the PawnIO installer places on the
  machine)**. Open question: whether an in-app "fetch the official modules"
  helper (explicit user action, checksum-verified, from the official source)
  is acceptable under this policy; the default is that the user installs them.
- *Obligations that remain:* our own modules, if any, are LGPL-2.1; we do not
  imply affiliation. Because nothing is shipped, LGPL notice and source-offer
  duties for third-party modules don't fall on our releases. New hardware
  support goes upstream into `PawnIO.Modules` so it gets signed.
- *Before M8:* email the author about bundling, module signing and the
  installer's terms; check HVCI and the vulnerable-driver blocklist status;
  check whether a winget package exists; decide the version-range policy.
- *Fallbacks, in order:* (1) a narrow allowlist driver of our own, exposing
  only named operations (no generic register access); (2) a fork of PawnIO
  (GPL-2.0-or-later, so allowed, with our own signing and update burden).
  Either needs a Microsoft attestation-signing setup and certificate budget
  **(verify current requirements)**, fuzzing, a vulnerability-response path and
  test hardware.
- *Not an option:* WSL or Linux drivers. WSL2 is a virtual machine and cannot
  see the host's MSRs, SMBus or Super I/O; WSL1 has no real kernel. WSL2 does
  expose GPUs (NVML works there) but adds nothing over the native path. A
  bootable Linux diagnostic image is a possible stretch feature, not a
  Windows driver substitute.

### Vendor SDKs and how we may use them
Checked 2026-10-20 from repository metadata and secondary sources; read each
license before relying on it.

| Vendor | SDK / interface | Covers | License notes |
|---|---|---|---|
| NVIDIA | **NVML** (driver-installed `nvml.dll` / `libnvidia-ml.so`) | Clocks, power, temperature, fans, VRAM, throttle reasons, per-process usage, power limits | Library comes with the driver. Header terms need reading (CUDA EULA covers NVIDIA headers; Chromium vendors a copy under a separate "NVML license") **(verify)**. `nvml-wrapper` is Apache-2.0 and loads the library at run time |
| NVIDIA | NVAPI, DCGM | Windows extras; datacenter health, XID | Read terms individually **(verify)**. `dcgm-exporter` is Apache-2.0 |
| AMD GPU | **ADLX** (Windows) | Telemetry and tuning | No open-source license detected in the repo, only an "ADLX SDK License Agreement" PDF **(verify terms)** |
| AMD GPU | **AMD SMI** (Linux/ROCm); `amdgpu` sysfs/hwmon | Telemetry | AMD SMI is MIT; Windows coverage limited **(verify)**. sysfs is kernel ABI |
| AMD CPU | Ryzen Master Monitoring SDK | Ryzen telemetry | Restrictive license (per a third-party binding project, files can't be redistributed); needs Ryzen Master's driver installed; has had security bulletins **(verify)**. Not a basis for the AMD CPU path; use PawnIO or Linux |
| AMD CPU | uProf | Profiling, power | Closed **(verify)** |
| Intel GPU | **IGCL** | Telemetry and some controls on Arc and integrated GPUs | Intel Software License: redistribution allowed with notices, "solely for use on Intel platforms"; not OSI-approved. Fine for an optional Intel backend |
| Intel GPU | **Level Zero / Sysman** | Power, clocks, temperature, engines | MIT; mainly Linux **(verify Windows)** |
| Intel CPU | **PCM** | Power, bandwidth, counters | BSD-3-Clause; Windows uses its own driver, so it carries the same driver question **(verify)**. Useful as a reference for register meanings, with attribution |
| Intel CPU | Intel Power Gadget | Package power | Discontinued as far as we recall **(verify)** |
| Apple | none public for sensors | Metal; private IOReport/SMC; root-only `powermetrics` | Basic support only |
| OS | PDH, ETW, WMI; `hwmon`, `powercap` | Load, frametimes, parts of power | Free |

Rules for using vendor SDKs:
- **No third-party executable code, drivers or vendor libraries are bundled with
  Surtr** (fonts and data under open licenses are allowed; runtimes such as
  .NET or a web view count as third-party code, see 7.1). Load the vendor library
  **at run time** from the user's driver install; never redistribute vendor
  binaries. (Surtr's own first-party binaries, such as the stress workers and
  the service, are built and shipped by us.)
- Each vendor is its own backend crate behind a Cargo feature and the
  `Provider` trait.
- For bindings, hand-write a small FFI layer from public documentation where a
  header's license is unclear. Output generated from a proprietary header is
  a derivative of it, so each vendor binding crate has its own LICENSE/NOTICE
  and a `cargo deny` exception, and the Apache/MIT crates never contain it.
  ADLX and IGCL are optional, not exit-gating, until their terms are read;
  `amdgpu` sysfs is the AMD baseline.
- Copy code only from permissive sources (Intel PCM is BSD-3). MPL-2.0 code
  (LibreHardwareMonitor) is weak copyleft: files derived from it stay MPL and
  live in a separately licensed file or crate that `cargo deny` allows, or we
  use it as a clean-room reference only. Never copy GPL code (Linux drivers, nvtop); study register layouts and
  write our own.
- Vendor backends cannot be tested in CI without the hardware, so each needs a
  mock or recorded fixtures.

### Plugins
- **Separate processes**, not dynamic libraries: Rust has no stable ABI, and a
  stress test is expected to crash or hang the machine.
- Protocol: `plugin --describe` returns a JSON manifest (name, targets,
  parameters, required privileges). The host runs it with parameters. The
  plugin streams JSON lines (progress, per-worker status, errors found). The
  host owns sensors and limits and kills the plugin on a limit or user stop.
- **Built-in stress tests are not plugins.** The core stress tests (CPU,
  memory, storage, GPU compute) are implemented in `surtr-stress`, compiled
  into Surtr, and need no manifest, install or trust step. They are still
  **isolated**: by default the host runs each one in a separate worker process
  (the `surtr` binary re-executed with a hidden `surtr worker` command), so a
  crash, hang or out-of-memory kill never takes down the host, and the host can
  kill the worker on a limit or user stop. Workers report through the same
  run-event schema as plugins (`surtr-plugin-api`), so run records, the API,
  MCP, CLI, TUI and GUI treat built-in and plugin tests identically, and the
  host's limits and audit rules apply equally.
- **Isolation rule.** Each `Workload` declares its isolation. `Required`:
  every stress and verification test; the host always uses a worker and there
  is no user toggle. `Optional`: short, low-risk benchmarks (seconds, no
  instability intent), which the host may run in-process, as may unit tests
  and calibration steps. Reasons: a crash or hang during a stress run is a
  result the host must survive and record (failing core, clock, voltage); the
  host must be able to kill a run reliably on a limit; heavy load threads
  would compete with sensor polling, the API and the UI; and a memory test can
  take the process down. Speed is not a reason, since stress runs last minutes
  and worker start-up costs milliseconds.
  Third-party and user-installed tests are plugins (above). First-party tests
  can be promoted to a stable plugin later if the protocol proves itself, but
  nothing requires it.
- **Why some are built in:** tight integration (per-core pinning, known-answer
  kernels, host-owned sensors and limits), heavy dependencies (wgpu) behind
  Cargo features, no extra install for the common tests, and faster iteration
  while the plugin protocol is still experimental (v0). Costs: a bigger core
  to maintain and audit, and no external proof that the plugin API is enough
  for real workloads, so the example plugins and `surtr plugin test` must cover
  that gap deliberately.
- Wrapping external tools (stress-ng, y-cruncher, etc.) is supported as
  user-installed plugins; we bundle no third-party binaries at all (project
  policy, see section 4), so the user installs those tools themselves.

### Data and storage
- Live: in-memory ring buffers per sensor, 1 s default poll, configurable.
- History: SQLite (downsampled rollups plus raw for recent window and for
  stress runs). Retention policy in settings.
- Run records: sensor timeline, plugin events, errors, config, hardware
  snapshot, versions. Exportable as a versioned JSON schema.

### Safety model
- Limits (temperature, power, voltage ceilings) are enforced in core library
  code in every mode (daemon, embedded CLI, MCP over stdio), for every run,
  from every entry point. A run whose limit sensors are missing does not start
  at full strength: it is refused or capped (see M6).
- The privileged service additionally enforces **hard floors/ceilings** per
  platform, checks the caller and holds all revert state, so a client that
  bypasses the core cannot exceed them. No caller can override them.
- Tuning writes require: tuning enabled in settings, explicit confirmation,
  and an **auto-revert timer** (default 60 s) unless confirmed. The timer runs
  in the service, not in a client.
- **Pending-write journal (service).** Every tuning write is journaled before it
  is applied and marked confirmed or reverted. Unconfirmed settings are never
  reapplied at boot. After an unclean shutdown or reset the service starts at
  stock settings, and persistent profiles (5.7) count as tuning writes: they
  need the same confirmation and are not reapplied until the user confirms
  again. Repeated crashes or resets after a write fall back to stock.
- **Run journal.** An unstable overclock often ends in a hard reset (WHEA/MCE,
  power loss), which worker isolation cannot survive. A write-ahead journal in
  the store, flushed before each step, lets the next start record "machine
  reset during step N" with the last known clock, voltage and temperature as a
  test failure result.
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
| Hardware backend (`Provider`) | Built-in backend crate, or a read-only out-of-process provider plugin that uses only named operations the service already offers | Sensors reachable through existing operations or vendor APIs. New chip support (Super I/O, embedded controller, SMBus) needs raw access, so it lands as a built-in backend or an upstream PawnIO module, not as a plugin. Board labels and fan mappings are data packs |
| Stress / benchmark workload | Built-in (`surtr-stress`, worker process) for the core tests; out-of-process plugin for everything else | Plugins: wrappers for external tools, custom verified kernels, game-like loads |
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
  additive only. The API is experimental (v0) until M11, when it becomes
  v1 with a documented compatibility policy. The minimum trust gate (explicit
  enable, capability prompt, process isolation) ships earlier, at M7, when the
  first plugin runs a real workload.
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
(threshold, rate-of-change), overlay feed for games (stretch), and a tray
presence (5.2.1).

#### 5.2.1 Tray / menu-bar presence
The tray icon belongs to the **per-user daemon** (`surtr serve`, never the
privileged `surtr-service`, which has no UI), not
to the GUI. Monitoring, logging, alerts and exporters keep running with no GUI
process and no webview; the GUI or TUI is launched on demand.

- **Icon and tooltip:** a status dot (green, amber, red) from the Simple-mode
  health summary; the tooltip shows CPU and GPU temperature and load. Status
  is never color alone (shape or label too).
- **Menu:** open GUI, open TUI in a terminal, pause alerts, start/stop a stress
  test (with the usual limits and confirmation), switch experience mode, quit.
- **Native notifications** for throttling, temperature or power-cap hits,
  driver resets and test failures, with per-alert enable and a quiet period.
- **Close to tray:** closing the GUI window hides it and stops all rendering
  (target about 0% CPU) while the daemon keeps running. Quit is explicit.
- **Start with the OS:** optional, off by default.
- **Live readout in the icon** (a temperature number): stretch goal.
- **Platforms:** Windows notification area is well supported. macOS uses the
  menu bar. Linux depends on the desktop (StatusNotifier/AppIndicator; GNOME
  needs an extension), so it is best-effort there and everything stays
  reachable from the CLI and TUI **(verify current Rust tray crates and Linux
  behavior)**.
- **No tray in headless mode.** `surtr status` and `surtr tui` show the same
  health summary and alerts.

### 5.3 Stress and benchmark
The tests below are **built in** (`surtr-stress`, run in isolated worker
processes, see "Plugins" in section 4). Plugins add further tests and wrap
external tools.
- CPU: verified FFT/integer/AVX2/AVX-512 (where present), per-core pinning,
  known-answer checks (a scalar baseline kernel on every platform, SIMD
  variants including NEON on ARM64). Report the failing core, clock and
  voltage at failure where the platform exposes them. Needs temperature or
  power limit sensors to run at full strength (see M6).
- Memory: pattern and walking-bit tests in the OS; recommend memtest86+ for
  boot-time confirmation.
- GPU: verified compute (wgpu/Vulkan compute first; CUDA/OpenCL optional) and
  a separate graphics load.
- Storage: write/read with verification. Safety rules: test files only in a
  user-chosen directory, a free-space check and an SSD-wear warning before
  start, no raw-device writes, and cleanup of test files on abort or crash.
- Benchmarks: simple integer, float, memory bandwidth, GPU compute (built in,
  with room for plugin benchmarks). Results are exportable and comparable offline; no online
  leaderboard in scope.

### 5.4 Tune (opt-in, later)
Profiles (Daily, Gaming, Quiet, Max OC), per-core offsets, power limits, fan
curves, boost override, apply-with-auto-revert, apply-and-stress, stability
map per core. Linux and Windows first; macOS read-only.

### 5.5 Reporting and observability
- **Prometheus exporter** (`/metrics`, OpenMetrics). Scrapeable by Grafana
  Agent/Alloy or Prometheus. Low-cardinality labels (device, sensor; `host` is a user-set alias, off by
  default, to respect the no-identifiers rule in 6.1).
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
The daemon exposes a local IPC endpoint (user-only permissions) that the GUI,
TUI, CLI and MCP use, and `surtr-api` serves an optional REST + WebSocket API
on loopback only (off by default, token-protected, with Origin/Host header
checks against DNS rebinding and CSRF). Remote access is an SSH-forwarded
loopback port; there is no non-loopback bind in v1 except an explicit opt-in
for `/metrics` with authentication and TLS guidance. Permissions and safety
are enforced in core library code used in every mode, not only at the API.

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
| `read` | `list_stress_tests` | available built-in and plugin workloads and their parameters |
| `read` | `get_run`, `list_runs`, `compare_runs` | run records and diffs |
| `read` | `get_capabilities` | what this machine/OS can and cannot do |
| `read` | `get_gpu_processes` | per-process VRAM and utilization per GPU |
| `read` | `get_throttle_reasons` | why each GPU/CPU is below its potential clock, now and over a time range |
| `read` | `correlate_metrics` | align hardware sensors with application metrics (for example tokens/s) over a range |
| `run` | `start_stress_test` | start a verified run with parameters and limits |
| `run` | `stop_stress_test`, `get_run_status` | control and poll |
| `tune` | `preview_profile` | dry-run: shows what would change |
| `tune` | `apply_profile` | requires human confirmation (see below) |

Tools appear only when their dependencies exist: the run tools and
`get_run`/`list_runs`/`compare_runs` from M6, `correlate_metrics` from M12.

**Resources:** `surtr://system`, `surtr://sensors/live`, `surtr://runs/{id}`,
`surtr://profiles/{name}`, `surtr://capabilities`. **Prompts:** "diagnose
instability", "plan an undervolt session", "summarize this run".

**Security rules (non-negotiable):**
- Default scope is `read`. `run` and `tune` are separate opt-ins in settings
  and can be limited to a time window.
- Host-enforced safety limits apply to every call; a caller cannot raise them
  beyond configured ceilings.
- Writes (`apply_profile`) require human confirmation through a Surtr-owned
  channel only: the GUI or TUI prompt, a tray/OS notification action, or
  `surtr tune confirm <id>` typed in a separate terminal. MCP elicitation is
  not a confirmation channel (the AI client renders it and could auto-answer
  it); it may only tell the user where to confirm. `start_stress_test` (scope
  `run`) needs per-run confirmation through the same channels, or a
  user-pre-approved test-and-limit template, because a stress run can crash the
  machine and lose unsaved work.
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

The GUI toolkit is decided at the M5b gate (7.1); egui is used only for
prototypes. The GUI consumes only `surtr-view` and the API client, so the
toolkit can be swapped without touching backends. Screens: Overview, CPU, GPU, Memory,
Storage, Sensors, Stress Test, Tuning, Benchmarks, Reports, Settings
(API/MCP/exporters/AI). Mockups (`docs/mockups/`, including the TUI board) and
a working egui Overview prototype exist from the exploration phase; the
prototype moves into `prototype/`.

### 7.1 Quality bar: modern and slick, GUI and TUI alike

"Slick" is a requirement with testable criteria, not a mood:

- **One design language.** Shared design tokens (colors, spacing, type scale,
  status colors, accent) defined once in `surtr-view` and rendered by each
  front end: pixels in the GUI, truecolor/256/16-color in the TUI.
- **Dark first, light and high-contrast available.** Status never relies on
  color alone (icon or label too). Honors the OS theme and `NO_COLOR`.
- **Motion with purpose.** GUI: 60 fps, interpolated graphs so a 1 s poll
  looks continuous, short eased transitions, no layout jumps when values
  change width (tabular numerals). TUI: flicker-free diff rendering, smooth
  sparklines/braille charts, no full redraws.
- **Dense but calm.** Numbers lead, chrome recedes. Consistent iconography,
  a bundled UI font, HiDPI-correct, resizable and responsive from a compact
  widget to a full dashboard (TUI: adapts from 80x24 up to ultrawide).
- **Fast and light.** Instant startup, low idle CPU/RAM (a monitor must not
  disturb the thing it measures). Numeric budgets (idle CPU, resident memory, cold start) are set at M3
  from measurement and then enforced on reference hardware.
- **Keyboard-first and discoverable.** Command palette in the GUI and TUI,
  visible key hints, mouse support in both, full keyboard navigation and screen
  reader support where the toolkit allows (AccessKit for egui **(verify)**).
- **Good states.** Designed empty, loading, error and "not supported on this
  hardware/OS" states; no blank panels, no raw error dumps.
- **Mode-aware.** Simple, Gaming, Advanced and Workstation modes (2.2) are
  different layouts of the same components, in the GUI and the TUI.
- **GUI toolkit: deferred on purpose.** The TUI and CLI come first (M3) and
  prove the engine, `surtr-view` and the design tokens without any GUI
  commitment. The GUI toolkit is decided at M5b, when the API, view models and
  a mature TUI exist, and the GUI is just another client of them. Candidates:
  Tauri/web, Avalonia (C#), Slint, iced; egui for prototypes only
  **(open decision)**. Policy check: Tauri/web and Avalonia depend on runtimes
  (WebView2/WebKitGTK, .NET) that Surtr must not bundle, so they qualify only
  if the runtime is a system component or user-installed; Avalonia would also
  make the GUI the one non-Rust part (the core stays Rust). Judged on these criteria, 60 fps live charts,
  accessibility, binary size, theming, and a hard footprint budget (idle CPU
  and RAM with the window open and hidden). The daemon runs without the UI, so
  monitoring cost never depends on the GUI choice.
- **Rules that keep the decision open:** no screen logic outside
  `surtr-view`; no UI toolkit type in `surtr-core`, `surtr-api` or
  `surtr-model`; the GUI talks only to view models and the API; the egui
  prototype stays a throwaway reference.

### 7.2 CLI and TUI: parity with the GUI

Rule: for every GUI screen there is a CLI command and, where interactive use
makes sense, a TUI view, all fed by the same `surtr-view` models. Each command
has three output forms: **styled human output** (default on a terminal),
**`--json`** (one document) and **`--ndjson`** (a stream for live data).
Colors, tables and charts are dropped automatically when piped or when
`NO_COLOR` is set. Exit codes are meaningful (for example stress: pass, errors
found, aborted by a safety limit, internal error).

| GUI screen / feature | CLI | TUI |
|---|---|---|
| Overview, Simple-mode health | `surtr status` / `surtr doctor` (traffic lights, plain-language findings) | Overview view |
| CPU, GPU, Memory, Storage, Board | `surtr info [cpu\|gpu\|mem\|storage\|board]` | per-device views |
| Sensors, live graphs | `surtr sensors`, `surtr watch` (live in-terminal charts), `--ndjson` | Sensors view, braille charts |
| GPU processes, throttle reasons | `surtr gpu procs`, `surtr gpu throttle` | multi-GPU view (nvtop-style) |
| Stress test | `surtr stress run\|stop\|status` (live progress, per-core results) | Stress view |
| Runs and reports | `surtr runs list\|show\|compare`, `surtr report` (HTML/JSON/CSV) | Runs view |
| Tuning, profiles, fan curves | `surtr tune preview\|apply\|revert`, `surtr profile ...` | profile view; curves as tables/coarse edit |
| Tray menu and notifications (daemon-owned, 5.2.1) | `surtr alerts`, `surtr status`, `surtr serve --tray` | alerts panel |
| Alerts, exporters, API/MCP, AI settings | `surtr config`, `surtr serve` (daemon, API, MCP, exporters), `surtr alerts` | Settings view |
| Plugins | `surtr plugin list\|install\|enable\|disable\|test` | Plugins view |
| Capabilities (what this machine supports) | `surtr caps` | shown inline as unsupported states |
| Game overlay and frametimes | `surtr frametimes` (capture/stats/log); overlay is GUI-only | frametime view |

Also: shell completions, a man page and `--help` that is genuinely helpful
(examples included), consistent flags and noun-verb structure.

**Deliberate exceptions (documented, not silent gaps):** the transparent
overlay window; pixel-level curve drag-editing (the CLI/TUI edit curves as
points or load them from a file); anything that needs a pointing device. Each
exception must still have a scriptable equivalent (for example, applying a
curve from a file).

**How the CLI runs.** It can work in **embedded mode** (no separate daemon
process: the same daemon code runs in-process with the same limits, and
refuses to start if another instance holds the hardware lock) or **client mode** (talks to a running
`surtr serve` daemon, locally, or over SSH through a forwarded loopback port, with `--host`; no
non-loopback listener in v1). Same commands in
both. Safety rules are identical to the GUI: the host enforces limits; `tune
apply` needs explicit confirmation and keeps the auto-revert timer in the
service, not in the CLI process, so a dropped SSH session still reverts.

**TUI stack.** A Rust TUI library such as `ratatui` with `crossterm`
**(verify current state and Windows terminal behavior)**, Unicode and truecolor
with graceful fallback, mouse and keyboard, works over SSH and inside tmux.
Test on Windows Terminal, the legacy console, and common Linux/macOS terminals.

**The TUI is the primary interface until the GUI exists (M3 to M5b)**, so it is
built to the full 7.1 quality bar, not as a stopgap: a polished theme,
high-resolution braille/block charts, smooth diff-based rendering, an
animated-but-calm live view, a command palette, a mouse-friendly layout, and
the four experience modes. Simple mode and Gaming mode can't rely on it long
term (regular users and gamers expect a window), so M5b is the point where the
GUI becomes required. Until then, the TUI and CLI ship to the Workstation and
Enthusiast audiences first.

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
- **Hardware verification.** CI runners have no GPUs and few sensors, so
  hardware milestones (M2, M5c, M8, M9) need "verified on named hardware" or a
  community-tester sign-off in their exit criteria, and those checks are not
  CI gates. CI covers mocks, fixtures and build/lint/license checks.
- **Release mechanics (first-release exit criteria).** An IPC version handshake
  between clients, daemon and privileged service; a service install,
  update and rollback policy; Windows code signing, SBOM and build provenance
  (`cargo-dist` may help **(verify)**), with the signing budget decided before
  the first release (check free signing programs for open source **(verify)**).
  No auto-update in v1: at most a version check, off by default.
- **Diagnostics.** `surtr diag` produces a redacted bundle for bug reports
  (versions, capabilities, logs, no identifiers unless the user opts in).

## 9. Licensing and governance

Decide before first public release:
- **Recommended:** Apache-2.0 OR MIT for crates, so others can embed the
  library. PawnIO is user-installed and not bundled, so its GPL terms do not
  attach to our crates; our own modules, if any, are LGPL-2.1 (see "Privileged
  access" in section 4). Policy: no third-party binaries (drivers, modules,
  vendor libraries) are bundled with Surtr; vendor SDKs are loaded at run time
  from the user's install. Talk to the PawnIO author before M8.
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
| M1 | Inspect (driverless) | `surtr --json` prints CPU/memory/OS on Windows + Linux + macOS; ARM64 builds run; `get_capabilities` reports what each backend can and cannot read |
| M2 | Sensors | GPU (NVML and amdgpu sysfs; ADLX and IGCL optional, not exit-gating until their terms are read) including multi-GPU, per-process VRAM and throttle reasons; Linux `hwmon`, Windows driverless sensors, SQLite history; verified on named hardware |
| M3 | TUI v1 + CLI | `surtr-view` and shared theme tokens (7.1); `surtr tui` with Overview, CPU, Sensors and multi-GPU views for desktop, headless and SSH use; CLI `status`, `info`, `sensors`, `watch` at parity with those views. **The GUI toolkit is not chosen yet** (7.1) |
| M4 | Export | Prometheus `/metrics`, CSV/JSON export, Grafana dashboards: Overview and AI workstation (per-GPU, throttle reasons); per-user daemon (`surtr serve`) with user-level service units, alert engine (threshold and rate-of-change) and native notifications |
| M5 | Local API + MCP (read-only) | `surtr-mcp` with the `read` tools whose dependencies exist (run tools arrive at M6, `correlate_metrics` at M12), auth, audit log; example agent session documented |
| M5b | GUI v1 | **Toolkit decision gate** (spike in 7.1, run here, not earlier); GUI Overview, CPU, Sensors screens at parity with the TUI/CLI on all three OS, built on `surtr-view` and the local API; minimal daemon-owned tray (status dot, tooltip, open, quit, close-to-tray; 5.2.1), alerts and notifications come from the M4 daemon |
| M5c | Read-only privileged sensors | The privileged-service split (no UI, no network, narrow operations, caller check, version handshake) with a Linux root helper and Windows PawnIO **read-only** operations (temperature, package power, voltage, effective clock), so M6 limits are real; no write paths; verified on named hardware |
| M6 | Stress v1 | `surtr-stress` with the `Workload` trait and worker-process runner; verified built-in CPU test; shared run-event schema and plugin protocol v0 documented; host limits, run records + HTML report; MCP `run` scope with per-run confirmation; CPU stress is refused (or capped to a short low-risk mode) when no temperature or power limit sensor is available, so it is Linux first and Windows once M5c sensors exist; run journal with post-reset recovery |
| M7 | Stress v2 + SKU DB | Built-in memory, storage and GPU tests; first example plugin proving the protocol with a real workload, with the minimum plugin trust gate (explicit enable, capability prompt, job-object/rlimit isolation); SKU database and contribution workflow |
| M8 | Privileged backends | SMBus/SPD, AMD/Intel power and the write paths, extending the M5c service and its PawnIO and root-helper backends |
| M9 | Tuning (opt-in) | Profiles, auto-revert, apply-and-stress, stability map; MCP `tune` scope with human confirmation |
| M10 | AI features + OTLP/Influx | Local-LLM summaries and triage, OpenTelemetry export (Influx is on the deferred list), fleet dashboard |
| M11 | Plugin ecosystem | Plugin API v1 with compatibility policy, `surtr plugin` install/trust commands, SDK + examples + `surtr plugin test`, provider and exporter plugins, (WASM script plugins and the curated manifest index are on the deferred list) |
| M12 | Workload adapters + overlay | Application-metric adapters (inference servers, training logs) correlated with sensors; ETW frametime capture and external overlay window; MangoHud interop on Linux; third-party sensor feed |

**CLI parity is a per-milestone exit criterion, not a late milestone:** the
CLI and TUI come first; from M5b a feature is not done until its CLI command
(with `--json`) and, if interactive, its TUI view exist and the GUI screen
reaches parity.

**First public release (changed after the plan review):** after M5, on Linux
and Windows x64 only: TUI and CLI inspector and monitor, Prometheus/Grafana
export, the local API and read-only MCP. The GUI (M5b) with the tray, then
verified stress testing (M5c, M6), follow as separate releases. macOS and
Windows on ARM are best-effort and outside the milestone gates. The gamer
persona is served after the first release; driverless ETW frametime capture is
independent of everything else and may move up to M5b.

**Deferred beyond v1 (not before the core is proven):** WASM script plugins,
InfluxDB and Parquet export, MQTT/Home Assistant and chat notifications, the
electricity-cost estimate, the curated plugin index, and TUI experience modes
beyond Advanced and Workstation.

## 11. Risks and open questions

- **Scope creep.** Mitigation: milestones above, read-only first, plugins.
- **Privileged code and driver trust** (signing, HVCI, blocklists, security
  advisories). Mitigation: narrow service API, PawnIO, security policy.
- **PawnIO dependency.** A single-maintainer driver whose module-signing and
  redistribution terms are unconfirmed. Mitigation: guide-don't-bundle, a
  backend trait, upstream contributions, contact the author before M8, and the
  narrow-driver or fork fallbacks in section 4.
- **Vendor SDK terms.** ADLX, IGCL and the NVML header are not under standard
  open-source licenses. Mitigation: run-time loading, no vendored binaries,
  read each license before adding bindings (section 4).
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
  first stress release, built-in tests sharing the run-event schema, example
  plugins exercising the protocol with a real workload (M7), then v1.
- **Built-in stress tests enlarge the core.** More code to audit and a risk
  that the plugin API is never proven by real use. Mitigation: run built-in
  tests in isolated worker processes with host-enforced limits, keep heavy
  dependencies behind features, and ship example plugins plus `surtr plugin
  test` coverage.
- **Modularity overhead:** many crates and feature combinations raise build
  and CI cost. Mitigation: test a small set of supported feature profiles
  (full, slim CLI, no-AI) instead of every combination.
- **Front-end drift.** Three front ends can diverge in features and look.
  Mitigation: thin front ends over `surtr-view`, shared theme tokens, the
  parity table in 7.2 reviewed per milestone, CLI/TUI snapshot tests.
- **Polish cost.** A slick GUI and TUI take sustained design effort; a rough UI
  undermines a tool people trust with their hardware. Mitigation: explicit
  quality bar (7.1), the toolkit spike at M5b, designed states from the start.
- **Late GUI.** Deferring the GUI toolkit delays the audiences who expect a
  window (gamers, regular users). Mitigation: the TUI/CLI serve enthusiasts and
  AI-workstation owners first; thin front ends keep the later GUI cheap.
- **Terminal variability.** Windows consoles, tmux and SSH differ in color,
  Unicode and mouse support. Mitigation: capability detection, graceful
  fallback, a terminal test matrix.
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
- Open questions: GUI toolkit, decided at the M5b gate (7.1); minimum OS versions; whether WASM script plugins are worth
  their complexity compared with declarative rules; native Rust GPU stress approach
  (wgpu vs vendor APIs); whether to build a Windows service or a scheduled
  elevated helper first; whether to support remote/fleet agents in v1.

## 12. Next steps

1. Pick the license and add LICENSE, README, SECURITY.md, CODE_OF_CONDUCT.
2. Create the workspace and `surtr-model` + `Provider` trait (M0).
3. Move the egui prototype into `prototype/` and keep it as a UI reference
   only. No GUI toolkit decision until M5b.
4. Set up CI on the 3-OS matrix.
5. Verify the crates and services flagged **(verify)**: `rmcp`, the open
   PawnIO items (module signing, redistribution, HVCI/blocklist, winget) and
   the vendor SDK license terms listed in section 4, vendor GPU APIs and their
   Rust bindings,
   ROCm/AMD SMI and Intel Level Zero coverage, PresentMon licensing, and the
   exporters of the AI runtimes listed in 5.7.

### Before making the repository public

The repository is private for now. Do these when switching it to public:

- [ ] **Security reporting:** enable private vulnerability reporting
      (Settings → Code security; GitHub offers it only on public repositories)
      and check that the steps in `SECURITY.md` work.
- [ ] **Code of conduct:** add `CODE_OF_CONDUCT.md` with a non-personal
      reporting contact (a project alias or a form, not a personal address).
- [ ] **Contributing:** add `CONTRIBUTING.md` (DCO sign-off, how to run tests),
      issue and PR templates, and the SKU-entry and sensor-mapping templates
      (section 9).
- [ ] **History check:** confirm every commit's author and committer address
      is the public GitHub one, and that no secrets, tokens, internal paths or
      private notes are in the history. Fixing history after going public
      needs a rewrite.
- [ ] **Branch protection on `main`:** require the CI checks, block force-push
      and deletion.
- [ ] **Security features:** turn on Dependabot alerts and security updates,
      secret scanning and push protection.
- [ ] **Actions settings:** default workflow token read-only; require approval
      before workflows run for first-time outside contributors.
- [ ] **License and naming:** confirm Apache-2.0 OR MIT and the copyright line;
      run the trademark search for "Surtr" (section 9).
- [ ] **Repository page:** description, topics, and a README status note that
      the project is pre-release and that stress tests can crash a machine.
- [ ] **Community:** decide whether to enable Discussions.

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
