# The MCP server (`impresario mcp`)

`impresario mcp` serves a [Model Context Protocol](https://modelcontextprotocol.io)
interface on **stdio** so any MCP-capable AI agent can inspect the
workspace, author tests against the DSL reference, check devices, run plays
and debug failures — without shelling out to the CLI.

It is a CLI addition, not part of the spec surface (decision I-80, the same
pattern as `mock-agent`, I-77).

```sh
cd my-workspace
impresario mcp        # serves until stdin closes; logs and diagnostics go to stderr
```

`--workspace`, `--config` and `--devices-config` select the workspace (they
default to the current directory and `impresario.config` /
`impresario.devices.config`, exactly like every other command).

## Connecting a client

Any client that can launch a stdio MCP server works. Examples:

```sh
# Claude Code, from the workspace directory
claude mcp add impresario -- impresario mcp

# Claude Code, pinned to a workspace
claude mcp add impresario -- impresario --workspace D:/Projects/my-app mcp
```

```jsonc
// Claude Desktop (claude_desktop_config.json); VS Code / Cursor use the
// same object under "servers" (VS Code, .vscode/mcp.json) or "mcpServers".
{
  "mcpServers": {
    "impresario": {
      "command": "impresario",
      "args": ["--workspace", "D:/Projects/my-app", "mcp"]
    }
  }
}
```

The server is **dual-era**: a 2026-07-28 client discovers it with
`server/discover` and sends `_meta.io.modelcontextprotocol/protocolVersion`
on every request (stateless); older clients negotiate with `initialize`
(handshake era, 2025-11-25 and back). Both get the same tools.

## Tools

All paths are workspace-relative unless absolute. Results carry a text
block (a summary line plus pretty JSON) and, where it matters,
`structuredContent`; screenshots come back as MCP **image** content so the
agent can look at them. Operation failures (a device is down, an artifact
does not exist, an argument is out of range) are tool errors
(`isError: true`), not protocol errors.

| Tool | Arguments | What it does |
| --- | --- | --- |
| `impresario_workspace_info` | — | Config defaults, devices **with each platform's action/check support matrix**, scenes, plays, acts, tags and diagnostics. Call this first. |
| `impresario_validate` | `paths?` | Parse/validate the workspace (or listed files/dirs) without touching a device. Returns core diagnostics with codes. |
| `impresario_devices_list` | — | The devices file, verbatim: name, platform, endpoint, app id/url, agent address, connector. No device contacted. |
| `impresario_devices_check` | `devices?` | Prepare → connect → handshake → `DeviceInfo` → teardown per device. Contacts hardware; takes seconds per device. |
| `impresario_run` | `plays?`, `devices?`, `tags?`, `exclude_tags?`, `acts?`, `vars?`, `timeout_ms?`, `retries?`, `fail_fast?`, `reporters?` | Plan and execute; writes `report_output/<run_id>` and returns a compact summary (statuses, counts, failures with the last 10 log lines, `run_dir`). `plays` entries may be bare names or simple globs (resolved against the plays directory) as well as workspace-relative paths or globs; a refusal lists the underlying diagnostics, not just a count. |
| `impresario_report_get` | `run_id?` | The full `report.json` (Appendix B document). Default: the newest run. |
| `impresario_artifacts_list` | `run_id?` | Files of a run with sizes and kinds (`report.json`, `junit.xml`, `report.html`, `screenshots/<device>/*.png`). |
| `impresario_artifact_get` | `path`, `run_id?` | One artifact. Images (PNG/JPEG/WebP) return as image content; text artifacts return as text (truncated at 2 MiB). |

Notes:

- **The console reporter is not available over MCP**: it writes to stdout,
  which carries the protocol. Runs default to `json,junit`; pass
  `reporters: ["json","junit","html"]` for more files. Asking for
  `console` is refused with an explanation.
- **One run or device check at a time**: both take a process-wide lock, so
  two agents (or two tool calls) can never drive one device concurrently.
  Other tools stay available while a run is in flight, and responses may
  arrive out of order (they are correlated by JSON-RPC id, as the spec
  allows).
- Runs respect the config's per-step timeouts and retries; there is no
  separate MCP-level timeout, so a long run is a long tool call.

## Resources

| URI | Content |
| --- | --- |
| `impresario://docs/dsl` | The DSL reference (`docs/DSL.md`), embedded at build time. |
| `impresario://docs/parity` | The platform support matrix (`docs/PARITY.md`), generated from the planner tables. |
| `impresario://docs/devices` | Device setup guide (`docs/DEVICES.md`): adb/Wi-Fi Android, Android TV, web, webOS/Tizen, emulators. |
| `impresario://docs/mcp` | This document. |
| `impresario://workspace/info` | Live JSON of the workspace summary (same payload as `impresario_workspace_info`). |

Embedding the docs means the agent always reads the documentation that
matches its server build.

## A suggested loop for an agent

1. `impresario_workspace_info` — learn the devices and the existing scenes.
2. Read `impresario://docs/dsl` and `impresario://docs/parity` — authoring
   rules and what the target platform supports (e.g. `swipe` is
   unsupported on TVs; D-pad navigation uses `focus`/`keys`).
3. Write or edit `.scene`/`.play` files with your own file tools.
4. `impresario_validate` — fix diagnostics before touching hardware.
5. `impresario_devices_check` — is the target reachable (agent installed,
   screen on, adb connected)?
6. `impresario_run` — run the play; on failure the summary points at the
   failed step and screenshots.
7. `impresario_artifact_get` on the failure screenshot (you will receive an
   image), `impresario_report_get` for the full document, then iterate.

## Implementation notes

- Transport: newline-delimited JSON-RPC 2.0 on stdio (the MCP stdio
  binding), one message per line, `tools/call` answered on a worker thread.
- Stdout is protocol-only. Every log, diagnostic and progress note goes to
  stderr (`tracing`), including the CLI diagnostics the run pipeline
  renders.
- Errors: `-32700` parse, `-32600` invalid request, `-32601` unknown
  method, `-32602` invalid params (including unknown tools), `-32603`
  internal, `-32022` unsupported protocol version (2026-07-28 revision)
  with `data.supported`.
- Artifact reads are confined to the run directory: absolute paths, `..`
  components and symlinks that resolve outside are rejected.
- The server exits when stdin closes (the client hung up); an in-flight run
  is abandoned with the process.
- No new dependencies: the protocol layer is hand-rolled on `serde_json`,
  and device work reuses the same code paths as the CLI (`validate`,
  `device check`/`devices check`, `run`). `run` accepts an optional `app`
  argument that overrides the selected plays' `App:` headers.

## Security

Any process that can spawn `impresario mcp` can drive the configured
devices (that is the point). The server adds no authentication of its own —
wire it only into clients you trust, and remember that run reports are
redacted by the same rules as CLI runs (spec 5.6).
