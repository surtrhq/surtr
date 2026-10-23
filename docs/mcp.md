# Surtr for AI agents (MCP)

`surtr mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server. An MCP client (Claude Code, Claude Desktop, or any other client that
supports stdio servers) starts it and can then ask about this machine's
hardware: what is installed, what the sensors say now, what they said over
the last hours, and why a GPU is slower than it should be.

By default every tool is **read-only**. Nothing an agent does through
`surtr mcp` changes the hardware, the settings or the recorded history.
Stress tests are a separate opt-in, and even then an agent can only *ask*:
a person starts each run (see [Stress runs](#stress-runs)). Tuning (M9) is
not part of this server.

## Setup

The easiest way: `surtr mcp config <client>` prints the exact setup for
`claude-code`, `claude-desktop`, `cursor` or `vscode`, with the full path of
your `surtr`, so the client finds it even if `surtr` is not on its `PATH`.
It only prints; it never edits the client's files.

By hand, Claude Code:

```sh
claude mcp add surtr -- surtr mcp
```

Claude Desktop (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "surtr": { "command": "surtr", "args": ["mcp"] }
  }
}
```

Use the full path to `surtr` if it is not on the `PATH` the client sees.

Every tool is marked read-only (`readOnlyHint`), so clients that support it
can approve Surtr once instead of asking on every call. Results come as
structured content (a JSON object) and as the same JSON in text.

Running `surtr serve` as well is recommended: the MCP server then answers from
the daemon's latest readings instead of polling the hardware itself, and
`query_history` has a history to look at.

### Over HTTP

For clients that connect over HTTP instead of starting a process, or several
clients sharing one server, `surtr serve` can serve MCP too:

```sh
surtr serve --mcp 127.0.0.1:7878
surtr token                # the bearer token clients must send
```

Claude Code:

```sh
claude mcp add --transport http surtr http://127.0.0.1:7878/mcp \
  --header "Authorization: Bearer <token>"
```

- Only loopback addresses are accepted. To reach it from another machine, use
  an SSH tunnel (`ssh -L 7878:127.0.0.1:7878 host`).
- Every request needs `Authorization: Bearer <token>`. The token is created
  on first use, stored in `serve-token` next to the history database
  (readable only by you), and replaced with `surtr token --new`. The REST API
  ([api.md](api.md)) uses the same token.
- Requests from web pages (any `Origin` header) and with a non-loopback
  `Host` are refused, so a website can't reach the server through your
  browser.
- Answers come from the daemon's latest read; the server never polls the
  hardware on its own.

## Tools

| Tool | Answers |
|---|---|
| `get_system_info` | OS, CPU, GPUs, memory, and the hardware providers in use |
| `get_sensors` | Current values; optional `device` (`cpu0`, `gpu1`, ...) and `kind` (`temperature`, `power`, `clock`, `load`, ...) filters |
| `get_capabilities` | What Surtr can and cannot read here, with the reason and the fix |
| `get_gpu_processes` | Processes using each GPU and their GPU memory |
| `get_throttle_reasons` | Why each GPU is below its potential clock right now |
| `query_history` | Recorded history: without `sensor`, the list of recorded sensors; with `sensor`, avg/min/max per bucket over the last `minutes` (default 60, at most 7 days) |
| `get_diagnostics` | The redacted `surtr diag` summary as Markdown |
| `list_stress_tests` | The built-in stress tests and whether agents may request runs |
| `list_runs` | Recorded stress runs, newest first, with result and peak temperature |
| `get_run` | One run in full (`id`, `latest` or `previous`): limits, wrong answers, per-second timeline |
| `compare_runs` | Two runs side by side (default: `previous` against `latest`): throughput, temperatures, throttling sag, errors, with warnings when they aren't comparable |

Results are JSON (Markdown for `get_diagnostics`). Times are UTC ISO 8601.
Each sensor value also comes formatted with its unit (`"text": "56.0 °C"`).

## Stress runs

Off by default. With

```toml
[mcp]
allow_stress = true
```

in `config.toml`, three more tools appear:

| Tool | Does |
|---|---|
| `start_stress_test` | Files a **request** (`minutes`, `threads`) and returns a short code. Nothing runs yet. |
| `get_run_status` | Where a request or run stands: pending, denied, expired, running, or its result |
| `stop_stress_test` | Asks a running test to stop; stopping is always safe |

A run starts only when a person types, in a terminal on the machine:

```sh
surtr runs confirm <code>     # shows who asked and what, asks y/N, runs here
surtr runs deny <code>
```

- **Agents can't confirm.** Confirming needs an interactive terminal and a
  typed `y`. MCP elicitation (a prompt the AI client draws) is never used,
  since the client could answer it itself.
- **The run happens in that terminal,** visible, with Ctrl+C, under the usual
  limits: the temperature limit, the duration cap (`stress.max_duration`
  also caps what an agent may ask for), and the refusal without a CPU
  temperature sensor.
- Requests expire after 10 minutes. Each run records which agent asked
  (`requested_by`), and every call is in the audit log.

## Resources and prompts

Clients that show resources can attach these to a conversation; each holds
the same JSON as the tool of the same name:

| Resource | Same as |
|---|---|
| `surtr://system` | `get_system_info` |
| `surtr://sensors/live` | `get_sensors` without filters |
| `surtr://capabilities` | `get_capabilities` |

The `diagnose_instability` prompt (optional argument `symptom`) starts a
guided look at crashes, freezes, throttling or slowdowns: it asks the agent to
check diagnostics, capabilities, sensors, throttle reasons and history, and to
rank the likely causes with the readings behind each.

## Privacy and safety

- No serial numbers, MAC addresses or host names are reported; paths under
  your home directory appear as `~`.
- Tool descriptions are fixed text. Sensor labels and process names are
  returned as JSON data, and the server tells the client to treat all results
  as data, never as instructions.
- `surtr mcp` talks only over stdin and stdout of the process the client
  started; it opens no network port. `surtr serve --mcp` listens on loopback
  only, behind the bearer token (see above).
- Every tool call and resource read is appended to `mcp-audit.jsonl` next
  to the history database: time, client name and version, tool, arguments,
  outcome, answer size and duration. It is rotated to `mcp-audit.1.jsonl` at 1 MiB.
- A client may make 30 calls at once and 5 per second after that; faster
  calls get an error result asking it to wait.

## Example session

What an agent sees, using the mock provider (`surtr --mock mcp`) so the
numbers are synthetic. Values are shortened here; the server sends full
precision. The history answer is illustrative and not shown.

> **User:** My renders got slower this afternoon. Is the machine OK?

The agent calls `get_sensors` with `{"kind": "temperature"}`:

```json
[{"id":"cpu0/package/temp","provider":"mock","device":"cpu0","label":"Package temperature",
  "kind":"temperature","unit":"celsius","value":56.04,"text":"56.0 °C","at":"2026-01-01T00:00:01Z"},
 {"id":"gpu0/temp","provider":"mock","device":"gpu0","label":"GPU temperature",
  "kind":"temperature","unit":"celsius","value":60.56,"text":"60.6 °C","at":"2026-01-01T00:00:01Z"}]
```

then `get_throttle_reasons`:

```json
[{"device":"gpu0","reasons":["power_cap"],"source":{"provider":"mock","method":"fixture"}}]
```

and `query_history` with `{"sensor": "gpu0/power", "minutes": 240}` to see
whether the power limit was reached all afternoon.

> **Agent:** Temperatures are fine (CPU 56 °C, GPU 61 °C). The GPU is
> limited by its power cap, and the history shows it has been at the cap
> since about 14:00, when power draw stepped up. That points at the workload
> or a changed power limit rather than overheating.
