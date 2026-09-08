<div align="center">
    <h1>Roxo</h1>
    <p><strong>MCP-native Rojo.</strong></p>
    <p>
        <a href="https://github.com/rbxrootx/roxo-mcp/actions/workflows/ci.yml"><img src="https://github.com/rbxrootx/roxo-mcp/actions/workflows/ci.yml/badge.svg" alt="CI" /></a>
        <a href="https://github.com/rbxrootx/roxo-mcp/releases/latest"><img src="https://img.shields.io/github/v/release/rbxrootx/roxo-mcp?label=release" alt="Latest release" /></a>
        <a href="LICENSE.txt"><img src="https://img.shields.io/badge/license-MPL--2.0-blue.svg" alt="MPL-2.0" /></a>
    </p>
</div>

<hr />

**Roxo** is a fork of [Rojo](https://github.com/rojo-rbx/rojo) that AI agents can actually drive.

Rojo assumes a person is sitting in Roblox Studio. Its serve session does nothing until someone opens the plugin and presses **Connect**, and the server never reports whether that happened. An agent can start `rojo serve`, write files all day, and have no idea that not one change reached Studio.

Roxo removes that assumption:

```sh
roxo mcp        # 7 tools, JSON-RPC over stdio
roxo status     # is Studio actually connected?
roxo sessions   # what is running on this machine
roxo wait --for studio
```

```json
{
  "mcpServers": {
    "roxo": { "command": "roxo", "args": ["mcp"] }
  }
}
```

An MCP server is only useful if something is listening on the other end, so the load-bearing piece underneath is **auto-connect**: the Studio plugin finds a running server and attaches on its own, gated so it cannot sync the wrong project into the wrong place. That gate is the interesting part, and it is documented in full below.

Everything Rojo does still works. Roxo is a **drop-in replacement** — same `.project.json` format, same default port, same protocol version — and it installs as both `roxo` and `rojo`, so existing scripts, Rokit manifests, and editor extensions keep working untouched.

## Contents

- [Install](#install)
- [MCP server](#mcp-server)
- [Auto-connect](#auto-connect)
- [Agent workflow](#agent-workflow)
- [CLI additions](#cli-additions)
- [Bugs Roxo fixes](#bugs-roxo-fixes)
- [HTTP API additions](#http-api-additions)
- [Project file additions](#project-file-additions)
- [How Roxo tracks Rojo](#how-roxo-tracks-rojo)
- [Building and releasing](#building-and-releasing)
- [Relationship to Rojo](#relationship-to-rojo)
- [License](#license)

## Install

Grab a build from [releases](https://github.com/rbxrootx/roxo-mcp/releases/latest) — every tag ships `roxo` and `rojo` for Linux, macOS, and Windows on both x86-64 and ARM, plus `Roxo.rbxm`. Put the binaries on your `PATH`, then:

```sh
roxo plugin install
```

Or build from source:

```sh
cargo install --git https://github.com/rbxrootx/roxo-mcp roxo
roxo plugin install
```

Both names are installed. `roxo` and `rojo` are the same binary, so Roxo drops into a toolchain that expects Rojo without touching a single script.

`roxo plugin install` writes the Studio plugin to the same file Rojo uses, so installing Roxo **replaces** Rojo's managed plugin rather than leaving two plugins fighting over the same place. That is deliberate — running both at once is exactly the failure Roxo's safety gate exists to prevent. Studio picks the new plugin up without a restart.

## MCP server

```sh
roxo mcp
```

Speaks JSON-RPC over stdin/stdout. All logging goes to stderr so it can't corrupt the protocol stream.

| Tool | Purpose |
| --- | --- |
| `roxo_sessions` | List running serve sessions |
| `roxo_status` | Session state, including whether Studio is connected |
| `roxo_wait_for_studio` | Block until Studio attaches |
| `roxo_serve_start` | Start a serve session for a project |
| `roxo_serve_stop` | Stop a session this server started |
| `roxo_build` | Build a place or model file without Studio |
| `roxo_sourcemap` | Map files onto Roblox instances |

Register it with an MCP client:

```json
{
  "mcpServers": {
    "roxo": {
      "command": "roxo",
      "args": ["mcp"]
    }
  }
}
```

Pass `--read-only` to expose only the tools that read state, for handing an agent a session it should observe but not steer.

`roxo_serve_stop` only stops sessions started through `roxo_serve_start`. Killing a server the agent didn't start would let it silently take down a developer's own session.

## Auto-connect

This is what makes the MCP tools above worth anything. The plugin looks for a serve session and connects without anyone pressing a button — otherwise `roxo_serve_start` would just start a server nobody is listening to.

Removing the button naively would be worse than the problem it solves: a plugin that attaches to whatever server it finds will cheerfully overwrite one game with another game's source. So the rule is that the **server has to prove it belongs to this place**, and the proof has to be unambiguous.

### How a server proves itself

Checked strongest first. Any one of these qualifies a server as a candidate:

| Proof | How it's established |
| --- | --- |
| `servePlaceIds` | The project lists this place's ID |
| `gameId` | The project's `gameId` matches this place's universe |
| Paired | A human connected this place to this project before, matched on project identity — not name |
| Declared | The project opted in with `"autoConnect": "always"` **and** the place is unpublished |

A server is disqualified outright if the place appears in `blockedPlaceIds`, if `servePlaceIds` exists and doesn't list the place, if the project sets `"autoConnect": "off"`, or if the protocol version doesn't match.

### The ambiguity rule

```
scan 34872..34881
  :34872  "MyGame"  servePlaceIds [123]  <- game.PlaceId = 123   MATCH
  :34873  "Other"   servePlaceIds [999]                          no match

exactly 1 candidate  ->  connect
0 candidates         ->  stay idle, keep looking
2+ candidates        ->  notify, connect nothing
```

**Two candidates never resolve to a connection, no matter how strong their claims are.** A stronger proof does not win a tie. Two projects claiming one place is a misconfiguration, and a wrong sync costs far more to undo than a prompt costs to answer.

### Pairing

A project that doesn't list its place IDs still gets auto-connect after one manual connection. Connecting by hand records a pairing between the place and the project's **identity** — a stable ID derived from the project file's path, or set explicitly with `projectId`. Every connection after the first is automatic.

Pairing keys on identity rather than name because "Game" is not a distinguishing label, and because a project file in a repository should not be able to claim someone else's pairing.

### Unpublished places

An unpublished place reports `PlaceId` of 0. There is nothing to match against, so strict matching cannot apply. Two ways forward:

* Connect once by hand to pair the place with the project, or
* Set `"autoConnect": "always"` in the project file.

`always` is a statement that the machine is trusted. It is the only mode where a server will accept a place it cannot identify — and it applies *only* to unpublished places, for exactly that reason.

Once a place has a real `PlaceId` it has an identity, so the strict tiers can do their job and `always` no longer overrides them. Without that limit, a scratch project left on `always` would follow you into whatever real game you opened next.

### Confirmation

Auto-connected sessions skip the patch confirmation dialog. They have to — the case auto-connect exists for is precisely the one where nobody is present to answer a prompt.

This is safe only because reaching that point required either an identity proof or an explicit opt-in in the project file; either way a human already decided the two belong together. The decision is never invisible: the match reason is sent to the server and shows up in `roxo status`.

### Settings

In the Studio plugin, under Settings:

| Setting | Default | What it does |
| --- | --- | --- |
| **Auto Connect** | on | Connect automatically to a server that proves it belongs to this place |
| **Auto Connect Ports** | `34872-34881` | The range searched when looking for a server |

Discovery polls while the place is not syncing, starting every 2 seconds and backing off to 15, so a server started *after* Studio is already open still gets picked up. That last part matters: it's the case Rojo's one-shot reconnect misses, and it's the normal case for an agent.

Turn auto-connect off and Roxo behaves exactly like Rojo.

## Agent workflow

The shape of a reliable unattended sync:

```sh
# Start a server. Not the same thing as being connected.
roxo serve ./my-game --port 34872 &

# Block until Studio actually attaches. This is the step that makes the
# difference between a real sync and writing into the void.
roxo wait --for studio --timeout 120

# Now file changes are reaching Studio. Do the work.
echo 'print("hello")' > src/init.server.luau

# Confirm, and fail loudly if the connection dropped.
roxo status --require-studio
```

`roxo status --json` gives an agent everything it needs to branch on:

```json
{
  "projectName": "MyGame",
  "projectId": "roxo-59ab89ed34cb7943",
  "studioConnected": true,
  "clientCount": 1,
  "clients": [
    {
      "clientName": "roxo-plugin",
      "placeId": 123456789,
      "autoConnected": true,
      "matchReason": "servePlaceIds",
      "subscribed": true
    }
  ],
  "patchesSent": 12,
  "lastPatchAt": 1757352901
}
```

`studioConnected` is the field to branch on. It means a client holds a **live change subscription**, not merely that something once looked at the server. A handshake alone doesn't count.

## CLI additions

Everything Rojo has, plus:

| Command | Purpose |
| --- | --- |
| `roxo sessions` | List running serve sessions on this machine |
| `roxo status` | Report a session's state and whether Studio is attached |
| `roxo wait --for studio` | Block until Studio connects, or time out |
| `roxo mcp` | Serve Roxo's tools to an agent over MCP |

Plus new flags on `serve`:

| Flag | Purpose |
| --- | --- |
| `--auto-connect <off\|matching\|always>` | Override the project's policy for one run (`always` still only applies to unpublished places) |
| `--no-registry` | Don't advertise this session for discovery |

`sessions`, `status`, and `wait` all accept `--port` or `--project` to pick a session, and `--json` for machine-readable output. When several sessions are running and none is named, they report the ambiguity rather than guessing — pointing an agent's writes at whichever project sorted first is the exact failure this all exists to avoid.

### Discovery without port scanning

Each `roxo serve` drops a JSON beacon in `~/.roxo/sessions/` and removes it on exit. Stale beacons — from a server killed with a signal, where cleanup never ran — are pruned by process ID when the directory is listed, so discovery never points at a dead port. Set `ROXO_HOME` to relocate the directory.

The Studio plugin can't read files, so it still scans ports. Everything that *can* read files uses the registry.

## Bugs Roxo fixes

Roxo carries fixes for upstream Rojo bugs that hurt unattended use most. A person watching Studio notices when sync dies; an agent does not, and keeps writing into a session nothing is listening to.

**Deleting a directory no longer kills the server.** Removing a directory removes its children too, and each child's filesystem event arrives naming a parent that is already gone. Resolving that parent used `.unwrap()`, and because the build aborts on panic, the whole process died. `rm -rf`, a branch switch that drops a folder, or dragging a directory to the trash all triggered it — three seconds to reproduce. Upstream has it as [rojo#1236], [rojo#1206], and [rojo#1309]. Removals now resolve against the nearest surviving ancestor.

**A file that vanishes mid-event is skipped, not fatal.** Editors write through temporary files and build tools churn; the race is routine.

**An unapplicable filesystem event is logged, not fatal.**

These are ordinary bug fixes, not Roxo-specific behavior. They are offered upstream too — Roxo would rather Rojo have them.

[rojo#1206]: https://github.com/rojo-rbx/rojo/issues/1206
[rojo#1236]: https://github.com/rojo-rbx/rojo/issues/1236
[rojo#1309]: https://github.com/rojo-rbx/rojo/issues/1309

## HTTP API additions

`GET /api/roxo/status` returns the JSON above. It's served as JSON, not msgpack, so nothing needs a decoder to ask whether Studio is connected.

`GET /api/roxo` is an alias for `/api/rojo`. The handshake response gains these fields:

| Field | Meaning |
| --- | --- |
| `serverName` | `"roxo"`, so a client can tell which server answered |
| `projectId` | Stable project identity used for pairing |
| `autoConnect` | The policy this session honors |
| `projectPath` | Absolute path of the project file |
| `capabilities` | Features supported, so clients feature-check rather than guess from versions |
| `clientId` | The id assigned to this client |

Clients identify themselves with query parameters on the handshake (`placeId`, `gameId`, `client`, `clientVersion`, `autoConnected`, `matchReason`). Query parameters were chosen because `/api/rojo` is a GET that older clients already call with none of them — an upstream Rojo server ignores the extras entirely, and an upstream Rojo *plugin* ignores the extra response fields, since the msgpack encoding is struct-as-map.

**Both directions stay compatible.** Roxo's plugin talks to a Rojo server, and Rojo's plugin talks to a Roxo server. A Rojo server that sets `servePlaceIds` will even auto-connect with Roxo's plugin, because a missing policy is treated as the default rather than as a refusal.

## Project file additions

```jsonc
{
  "name": "MyGame",
  "servePlaceIds": [123456789],

  // "off" | "matching" (default) | "always", or a boolean
  "autoConnect": "matching",

  // Optional. Defaults to a hash of the project file's path. Set it explicitly
  // if the project lives at different paths on different machines and you want
  // pairings to follow it.
  "projectId": "my-game"
}
```

`projectId` is intentionally **not** stable across machines by default. Pairing is a local trust decision, and a shared identifier would let a project file in a repository claim someone else's pairing.

## How Roxo tracks Rojo

Roxo is **upstream Rojo with a patch series on top**, not a fork that drifted. `main` is literally `rojo-rbx/rojo`'s history with a handful of Roxo commits appended, so one command shows you the entire diff of the project:

```sh
git remote add upstream https://github.com/rojo-rbx/rojo.git
git fetch upstream
git log --oneline upstream/master..main    # every change Roxo makes
```

Upstream is taken by **rebase**, not merge. The patches replay onto the new Rojo and the history stays flat, so the fork never accumulates merge commits that obscure what it actually changed.

`.github/workflows/upstream-sync.yml` runs daily: it replays the patch series onto the latest Rojo, runs the test suite, and pushes the result to a branch with a summary. It does not touch `main` on its own — a rebase rewrites history, which is a decision a person makes. Run it from the Actions tab with **apply** checked to land it, or do it by hand:

```sh
git rebase --onto upstream/master $(git merge-base HEAD upstream/master)
git push --force-with-lease origin main
```

To keep replays boring, Roxo's own code lives in new files wherever possible — `src/auto_connect.rs`, `src/client_registry.rs`, `src/session_registry.rs`, `src/cli/{sessions,status,wait,mcp}.rs`, `plugin/src/AutoConnect.lua` — rather than being woven through Rojo's.

## Building and releasing

Every push to `main` runs the full suite across Linux, macOS, and Windows, and attaches a built `roxo` binary and `Roxo.rbxm` plugin to the run, so trying a change needs no local Rust toolchain.

Tagging `v*` builds all six platform targets, ships them with the plugin, and publishes the release once every artifact has landed.

## Relationship to Rojo

Roxo exists because these changes serve a narrower audience than Rojo's, not because anything is wrong with Rojo. Rojo is the upstream, it gets the credit, and Roxo takes its changes continuously.

If you don't need unattended workflows, [use Rojo](https://github.com/rojo-rbx/rojo). Roxo's documentation covers only what it adds; for everything else, [Rojo's documentation](https://rojo.space/docs) applies unchanged.

## License

Roxo is available under the Mozilla Public License, Version 2.0, the same as Rojo. See [LICENSE.txt](LICENSE.txt).
