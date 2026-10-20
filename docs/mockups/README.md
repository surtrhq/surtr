# Surtr UI mockups

Design sketches for the screens described in [../PLAN.md](../PLAN.md). All
numbers are **sample data**. Hardware names are examples, not endorsements.

The files under `project/` are Design Component pages (`*.dc.html`) laid out
on a canvas by `project/canvas.json`. They need the Design canvas runtime
(`support.js`) to render, so opening them directly in a browser will not show
the designs. They are kept here as the source of truth for the mockups.

| Board | File | Persona / purpose |
|---|---|---|
| Overview | `Main.dc.html` | Advanced: CPU, GPU, memory, cooling, 5-minute timeline, recent runs |
| CPU Detail | `Cpu.dc.html` | Enthusiast: spec tabs, per-core tiles with preferred-core rank, limits |
| Stress Test | `Stress.dc.html` | Enthusiast: plugin workloads, host-enforced limits, live telemetry, per-core workers |
| Tuning | `Tuning.dc.html` | Enthusiast: profiles, per-core offsets, power limits, fan curve, auto-revert apply bar |
| AI Workstation | `Workstation.dc.html` | AI: per-GPU VRAM/power/throttle, per-process view, tokens/s vs sensors, alerts, energy |
| Game Session | `Game.dc.html` | Gamer: overlay HUD (external window, no injection), frametimes, session summary |
| Simple Mode | `Simple.dc.html` | Regular user: plain-language health summary |
| Run Report | `Report.dc.html` | Shareable, redacted stability report |
| Connections | `Connections.dc.html` | API, MCP scopes, exporters, AI assistant preview, audit log |
| Terminal UI | `Tui.dc.html` | `surtr tui` for headless and SSH use |

## Notes

- Design language: dark graphite, one amber accent (a tweakable prop), IBM Plex
  Sans and Mono, dense numeric layout.
- The first four boards use the shorter navigation from the first iteration.
  The later boards add Workstation, Game Session, Reports and Connections.
- The Connections mockup shows only a masked token. No real secrets belong in
  mockups or in the repository.
- A throwaway egui prototype of the Overview screen (outside this repo, in
  `hwx-proto`) is the reference for what egui can reproduce.
