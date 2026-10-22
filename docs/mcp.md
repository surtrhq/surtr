# Surtr for AI agents (MCP)

`surtr mcp` is a [Model Context Protocol](https://modelcontextprotocol.io)
server. An MCP client (Claude Code, Claude Desktop, or any other client that
supports stdio servers) starts it and can then ask about this machine's
hardware: what is installed, what the sensors say now, what they said over
the last hours, and why a GPU is slower than it should be.

Every tool is **read-only**. Nothing an agent does through `surtr mcp` changes
the hardware, the settings or the recorded history. Stress tests (M6) and
tuning (M9) will be separate opt-in scopes with confirmation outside the
agent; they are not part of this server today.

## Setup

Claude Code:

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

Running `surtr serve` as well is recommended: the MCP server then answers from
the daemon's latest readings instead of polling the hardware itself, and
`query_history` has a history to look at.

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

Results are JSON (Markdown for `get_diagnostics`). Times are UTC ISO 8601.
Each sensor value also comes formatted with its unit (`"text": "56.0 °C"`).

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
- The server talks only over stdin and stdout of the process the client
  started; it opens no network port.
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
