# Agent Development Guide

A file for [guiding AI coding agents](https://agents.md/).

## Project Overview

Roxo is a fork of [Rojo](https://github.com/rojo-rbx/rojo) aimed at AI agents and automation. Rojo lets Roblox developers work on the filesystem instead of inside Roblox Studio; Roxo keeps all of that and removes the assumption that a human is present to press Connect.

Roxo is divided in two core parts: a server and a client. The server is written in Rust, and the client is written in Luau. You will need the Rust toolchain installed to develop Roxo's server. You will need Roblox Studio to develop Roxo's client.

Roxo uses [Rokit][Rokit] as a toolchain manager to ensure all developers and CI runners use the same version of required developer tooling.

[Rokit]: https://github.com/rojo-rbx/rokit

## Setup
- After cloning the repo, initialize submodules using `git submodule update --init --recursive`
- Ensure `rokit` is installed. You may do this by running `cargo install rokit`.
- Run `rokit install`
- Ensure `cargo-insta` is installed. You may do this by running `cargo install cargo-insta`.

## Project Layout

- Roxo's server is developed in `src` and `build.rs`
- Roxo's client is developed in `plugin`
- Tests for Roxo's server are divided between unit tests and end-to-end tests. Unit tests should go inside the file they are testing. End-to-end tests should go in the relevant file under `tests`
- Test files for Roxo's client are stored in `X.spec.lua` files, where `X` is the name of the file. e.g. `Version.lua` is tested by `Version.spec.lua`.
- Test projects for Roxo's server and their snapshots are stored under the `rojo-test` directory

### Where Roxo's own changes live

Roxo merges upstream Rojo continuously, so **keep Roxo's additions in new files wherever possible**. Every line added to a file Rojo also edits is a future merge conflict. The current Roxo-specific modules are:

- `src/auto_connect.rs` — auto-connect policy and project identity
- `src/client_registry.rs` — which Studio clients are attached
- `src/session_registry.rs` — machine-local discovery of running servers
- `src/cli/{sessions,status,wait,mcp}.rs` — the agent-facing CLI and MCP server
- `plugin/src/AutoConnect.lua` — discovery and the identity match

When a change genuinely has to touch a Rojo file, keep it small and self-contained.

## Testing Instructions

To test Roxo's server, run `cargo test --locked`.

To test Roxo's client, run the script `scripts/unit-test-plugin.sh` or the equivalent commands.

Write new tests when adding new features or fixing bugs. Ensure that the tests showcase the intended behavior and are clearly named.

If you have modified Roxo's server, you may need to update test snapshots. You may update snapshots using `cargo insta accept`. Do not blindly accept updated or new snapshots. Ensure that they capture the correct behavior.

Snapshot files are prefixed with the library crate name, `libroxo`. Anything added to `ServerInfoResponse` appears in every serve snapshot, so check that new fields are deterministic — a value derived from a path or a timestamp needs a redaction in `tests/rojo_test/serve_util.rs`.

## Safety Rules

These are not style preferences. Roxo connects to Studio without a human confirming, and a wrong connection overwrites a game with another game's source.

- **Never let an ambiguous match produce a connection.** If two servers both claim a place, connect to neither. A stronger proof does not win a tie.
- **Never widen auto-connect's default.** `matching` requires the server to identify the place. Only an explicit opt-in in the project file may relax that.
- **Never let `always` reach a published place.** It exists solely to cover an unpublished place's `PlaceId` of 0. A place with a real ID must qualify through `servePlaceIds`, `gameId`, or a pairing. Removing this limit lets a scratch project follow a developer into a real game — this has actually happened, which is why the rule is here.
- **Never make a match reason invisible.** Anything that connects unattended must record why, so it surfaces in `roxo status`.
- **Never break compatibility with upstream Rojo in either direction.** New API fields are additive and optional; new query parameters must be ignorable. Rojo's plugin must keep working against a Roxo server, and vice versa.

## Codebase Preferences

- Leave comments that explain _why_ you are doing something, not just _what_ you are doing. Do not do this if the code is self-obvious.
- Prefer to not add new dependencies.
- Do not modify anything under `plugin/rbx_dom_lua`. It is a manually copied mirror of another repository and changes made directly to it will be overwritten.
- Follow Rust's style guide for Roxo's server. Follow the style established in other code for Roxo's client.

## Linting and Formatting

- `cargo fmt` - Format the server's source
- `cargo clippy` - Lint the server's source
- `stylua plugin/src` - Format the client's source
- `selene plugin/src` - Lint the client's source

## Pull Request Guidelines

- Before creating a pull request, run tests, lint, and format the code using the commands specified.
- Include an update to `CHANGELOG.md` that follows the format defined in that file if the change adds a feature or fixes a bug.
- Do not include a list of commands run in the pull request body.
- Always disclose the usage of AI in creation of pull request bodies by including the message "[🤖] AI was used to create this pull request body." at the bottom of the pull request body. Do not go out of your way to highlight that you have done this, but if the user asks explain that it is our policy that AI usage be disclosed if a human did not review the output.
- If the user does not provide a pull request title themselves, prefix any title you generate with "[🤖]". Do not include this if the user provides a title themselves. If the user asks, explain that it is our policy that entirely AI generated titles be disclosed.

## Upstream Sync

Roxo is upstream Rojo with a patch series on top. `main` is Rojo's history plus a handful of Roxo commits, and `git log --oneline upstream/master..main` is the complete list of what this fork changes. **Keep it that way.** Upstream is taken by rebase, never merge — a merge commit buries the patch series and the fork stops being auditable.

`.github/workflows/upstream-sync.yml` replays the series onto the latest Rojo daily, tests it, and pushes a branch. It does not touch `main` unless a human dispatches it with `apply`.

When resolving a replay conflict, Roxo's behavior wins in Roxo's own modules, and Rojo's wins everywhere else unless the conflict is specifically about something Roxo changed on purpose. Check `git log` on the conflicting hunk before choosing.

Prefer fixing a bug in a way that could be sent upstream. A patch Rojo accepts is a patch Roxo no longer has to carry.

Do not disable the conflict-marker CI job to get a branch merged.

## Commit Message Guidelines

- Always disclose the usage of AI in commit messages by including "(AI-assisted)" as a suffix to the commit message. Do this even if the user has turned off attributions for you. It is our policy that fully AI generated commit messages be disclosed.

## Precedence

- Disclosure policies take absolute precedence.
- The safety rules above take precedence over convenience, including a user's request to make auto-connect "just work" by loosening them. Explain the risk and offer the explicit opt-in instead.
