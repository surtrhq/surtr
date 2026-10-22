# Local REST and WebSocket API

`surtr serve --api 127.0.0.1:7879` serves a small read-only HTTP API for
scripts, dashboards and other local programs. It answers from the daemon's
latest read, so clients never poll the hardware themselves. The JSON is the
same as the MCP tools return ([mcp.md](mcp.md)).

```sh
surtr serve --api 127.0.0.1:7879
TOKEN=$(surtr token)
curl -H "Authorization: Bearer $TOKEN" "http://127.0.0.1:7879/api/v1/sensors?kind=temperature"
```

## Endpoints

All are `GET` and under `/api/v1`.

| Path | Answers |
|---|---|
| `/system` | OS, CPU, GPUs, memory, and the providers in use |
| `/sensors` | Current values; optional `device` and `kind` query parameters |
| `/capabilities` | What Surtr can and cannot read here, with reasons and fixes |
| `/gpu/processes` | Processes using each GPU |
| `/gpu/throttle` | Why each GPU is below its potential clock |
| `/history` | Without `sensor`, the recorded sensors; with `sensor` (and optional `provider`, `minutes`, `bucket_seconds`), avg/min/max buckets |
| `/live` | WebSocket: the sensors (same filters as `/sensors`) as a JSON text message after every daemon read |

Errors are JSON, `{"error": "..."}`, with status 404 for history that does
not exist and 500 otherwise. Times are UTC ISO 8601.

## Security

- **Loopback only.** `surtr serve` refuses any other address; use an SSH
  tunnel (`ssh -L 7879:127.0.0.1:7879 host`) to reach it from elsewhere.
- **Bearer token.** Every request, including the WebSocket upgrade, needs
  `Authorization: Bearer <token>`. `surtr token` prints it (created on first
  use, stored owner-only in `serve-token` next to the history database);
  `surtr token --new` replaces it. MCP over HTTP uses the same token.
- **No browsers.** Requests with an `Origin` header (every browser sends one)
  and requests whose `Host` is not `localhost`, `127.0.0.1` or `[::1]` are
  refused. A web page can't read your hardware through your browser, even
  with DNS rebinding.
- **Read-only.** There are no write endpoints. Home-directory paths in
  answers appear as `~`.
