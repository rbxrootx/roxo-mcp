# Roxo

MCP-native [Rojo](https://github.com/rojo-rbx/rojo). Roblox filesystem sync that AI agents can drive.

[![CI](https://github.com/rbxrootx/roxo-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/rbxrootx/roxo-mcp/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/rbxrootx/roxo-mcp?label=release)](https://github.com/rbxrootx/roxo-mcp/releases/latest)
[![License](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](LICENSE.txt)

Rojo needs a human to press **Connect** in Studio, and never reports whether anyone did. Roxo adds an MCP server, a status API, and a plugin that connects on its own.

Drop-in replacement: same project format, same port, same protocol. Installs as both `roxo` and `rojo`.

## Install

Download from [releases](https://github.com/rbxrootx/roxo-mcp/releases/latest), or:

```sh
cargo install --git https://github.com/rbxrootx/roxo-mcp roxo
roxo plugin install
```

The plugin replaces Rojo's — same filename, so you don't end up with two. Studio picks it up without a restart.

## MCP

```json
{
  "mcpServers": {
    "roxo": { "command": "roxo", "args": ["mcp"] }
  }
}
```

| Tool | |
| --- | --- |
| `roxo_sessions` | List running servers |
| `roxo_status` | Session state, including whether Studio is connected |
| `roxo_wait_for_studio` | Block until Studio attaches |
| `roxo_serve_start` | Start a server |
| `roxo_serve_stop` | Stop one this server started |
| `roxo_build` | Build a place or model file |
| `roxo_sourcemap` | Map files to instances |

`--read-only` exposes only the read tools.

## CLI

```sh
roxo serve ./my-game &
roxo wait --for studio      # a running server is not a connected one
roxo status --require-studio
```

| | |
| --- | --- |
| `roxo sessions` | What's running on this machine |
| `roxo status` | Is Studio connected, and why did it connect |
| `roxo wait --for studio` | Block until it is |
| `roxo mcp` | MCP server over stdio |

All take `--port`, `--project`, `--json`. Everything Rojo has still works.

Servers advertise themselves in `~/.roxo/sessions`, so nothing has to scan ports. `ROXO_HOME` relocates it.

## Auto-connect

The plugin scans ports 34872-34881 and attaches by itself — but only to a server that proves it owns the open place:

| Match | |
| --- | --- |
| `servePlaceIds` | Project lists this place |
| `gameId` | Project's `gameId` matches |
| Paired | You connected them by hand once |
| Declared | `"autoConnect": "always"`, unpublished places only |

**Two matches connects to neither.** A stronger claim doesn't win a tie — two projects claiming one place is a misconfiguration, and picking wrong overwrites a game with the wrong source.

Auto-connected sessions skip the confirmation dialog, since nobody's there to answer it. The match reason shows up in `roxo status`.

Turn it off in the plugin's settings and Roxo behaves like Rojo.

## Project file

```jsonc
{
  "autoConnect": "matching",  // "off" | "matching" | "always" | bool
  "projectId": "my-game"      // optional; defaults to a hash of the path
}
```

`projectId` isn't stable across machines by default — pairing is a local trust decision.

## API

`GET /api/roxo/status` returns JSON:

```json
{ "studioConnected": true, "clientCount": 1, "patchesSent": 12,
  "clients": [{ "placeId": 123, "autoConnected": true, "matchReason": "servePlaceIds" }] }
```

`studioConnected` means a live subscription, not just that something pinged the server once.

`/api/rojo` gains `serverName`, `projectId`, `autoConnect`, `projectPath`, `capabilities`, `clientId`. All additive — Rojo's plugin works against a Roxo server and vice versa.

## Fixes over Rojo

- Deleting a directory no longer kills the server ([rojo#1236], [rojo#1206], [rojo#1309])
- A file that vanishes mid-event is skipped, not fatal
- An unapplicable filesystem event is logged, not fatal

[rojo#1206]: https://github.com/rojo-rbx/rojo/issues/1206
[rojo#1236]: https://github.com/rojo-rbx/rojo/issues/1236
[rojo#1309]: https://github.com/rojo-rbx/rojo/issues/1309

## Upstream

`main` is Rojo's history with Roxo's commits on top:

```sh
git remote add upstream https://github.com/rojo-rbx/rojo.git
git fetch upstream
git log --oneline upstream/master..main   # the whole fork
```

Upstream is taken by rebase, not merge. A daily workflow replays the patches and tests them; it doesn't touch `main` unless you dispatch it with `apply`.

## License

MPL-2.0, same as Rojo. Not affiliated with rojo-rbx — if you don't need unattended sync, use [Rojo](https://github.com/rojo-rbx/rojo).
