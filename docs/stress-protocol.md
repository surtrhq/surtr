# Stress run protocol

How a stress test talks to Surtr. Built-in tests (`surtr-stress`) use it
today; plugins will use the same events (plugin protocol v0, below), so run
records, the CLI, the TUI, MCP and later the GUI treat both alike (plan,
section 4).

## Host and worker

A stress test never runs inside the process that watches it.

- The **host** (`surtr stress`, or `surtr runs confirm` for a run an agent
  asked for) picks the limits, writes the run journal, starts the worker,
  reads its events, polls the temperatures twice a second, and kills the
  worker at the temperature limit, past its deadline, or on Ctrl+C.
- The **worker** (`surtr worker <workload> --threads N --duration-ms D`, a
  hidden command) only computes and reports. It writes one JSON object per
  line to standard output and stops by itself when its duration has passed.

A worker that crashes, hangs, is killed by the OS or prints garbage is a
**result** (`worker_failed`), never a host failure. Lines that aren't
events are ignored.

## Run events (schema 1)

Each event is an object with an `event` field.

| Event | Fields | When |
|---|---|---|
| `started` | `schema` (1), `workload`, `threads` | First, before any load |
| `progress` | `elapsed_ms`, `iterations` (one count per thread) | About once a second |
| `error` | `thread`, `cpu` (logical CPU if pinned, else null), `kernel`, `elapsed_ms` | A computation gave a wrong answer |
| `finished` | `iterations` | The duration passed; last line |

```json
{"event":"started","schema":1,"workload":"cpu","threads":8}
{"event":"progress","elapsed_ms":1000,"iterations":[412,409,415,411,410,408,413,412]}
{"event":"error","thread":3,"cpu":3,"kernel":"float","elapsed_ms":1834}
{"event":"finished","iterations":[24690,24601,24712,24655,24630,24588,24701,24660]}
```

A worker reports at most ten errors per thread and then stops reporting
that thread's errors, so a broken CPU can't flood the host; one error is
enough for the verdict. New optional fields may appear in later
schema versions; hosts ignore fields they don't know.

## What the host adds

The host turns events into a run record (`surtr runs show <id> --json`):

- **Outcome:** `passed`, `errors_found` (any wrong answer wins), `temperature_limit`, `stopped` (user or agent), or `worker_failed`. Exit codes: 0 passed, 2 stopped early, 3 errors or failure.
- **Failures:** each one with the clock of its CPU at the last read before the error.
- **Timeline:** once a second, the highest watched temperature and the total iterations, which show throttling as a drop in throughput.
- **Limits:** the temperatures watched and where each stops the run.

## Plugin protocol v0 (planned, M7)

Plugins are separate programs that the user installs; Surtr bundles no
third-party binaries.

- `plugin --describe` prints a JSON manifest: name, what it tests, the
  parameters with types and ranges, and the privileges it needs.
- The host runs `plugin --run` with the parameters as JSON on standard
  input, and the plugin writes the run events above to standard output.
- Plugins run with the same isolation, limits, journal and records as
  built-in tests. The first example plugin in M7 will prove the protocol
  with a real workload, behind an explicit enable step and a capability
  prompt.
