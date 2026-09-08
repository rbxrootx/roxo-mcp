//! An MCP server that exposes Roxo to AI agents over stdio.
//!
//! Agents already know how to call MCP tools, so this removes the glue an agent
//! would otherwise need: no shelling out, no parsing human-readable output, and
//! no guessing about whether a sync actually reached Studio. Every tool here is
//! a thin wrapper over the same registry and HTTP API the CLI uses, so the two
//! surfaces can never drift into disagreeing about the state of a session.

use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use clap::Parser;
use serde_json::{json, Value};

use crate::session_registry;

use super::{status::fetch_status, GlobalOptions};

/// The newest MCP revision this server implements. A client asking for an older
/// revision is answered in its own revision, since nothing here depends on
/// features added between them.
const LATEST_PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// How long to wait for a freshly spawned server to advertise itself before
/// reporting the start as failed.
const SERVE_START_TIMEOUT: Duration = Duration::from_secs(20);

/// Serve Roxo's tools to an AI agent over the Model Context Protocol.
///
/// Speaks JSON-RPC over stdin and stdout. All logging goes to stderr so it
/// cannot corrupt the protocol stream.
#[derive(Debug, Parser)]
pub struct McpCommand {
    /// Only expose tools that read state, refusing to start, stop, or build
    /// anything. Useful when handing an agent a session it should observe but
    /// not steer.
    #[clap(long)]
    pub read_only: bool,
}

impl McpCommand {
    pub fn run(self, _global: GlobalOptions) -> anyhow::Result<()> {
        let mut server = McpServer::new(self.read_only);
        server.serve()
    }
}

struct McpServer {
    read_only: bool,

    /// Serve sessions this server started, kept so they can be stopped and
    /// reaped rather than left as orphans when the agent moves on.
    spawned: HashMap<u16, Child>,
}

impl McpServer {
    fn new(read_only: bool) -> Self {
        Self {
            read_only,
            spawned: HashMap::new(),
        }
    }

    fn serve(&mut self) -> anyhow::Result<()> {
        let stdin = std::io::stdin();
        let reader = BufReader::new(stdin.lock());

        for line in reader.lines() {
            let line = line?;

            if line.trim().is_empty() {
                continue;
            }

            let request: Value = match serde_json::from_str(&line) {
                Ok(request) => request,
                Err(err) => {
                    // A malformed frame has no id to answer against, so the
                    // only correct response is a parse error with a null id.
                    respond(&json!({
                        "jsonrpc": "2.0",
                        "id": Value::Null,
                        "error": { "code": -32700, "message": format!("Parse error: {err}") },
                    }))?;
                    continue;
                }
            };

            let id = request.get("id").cloned();
            let method = request
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let params = request.get("params").cloned().unwrap_or(Value::Null);

            // Requests carry an id and expect a reply; notifications do not and
            // must be answered with silence.
            let is_notification = id.is_none();

            let result = self.dispatch(&method, params);

            if is_notification {
                continue;
            }

            let id = id.unwrap_or(Value::Null);

            let response = match result {
                Ok(Some(result)) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                // A method that returns nothing but was called as a request
                // still needs an acknowledgement.
                Ok(None) => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
                Err(err) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": err.code, "message": err.message },
                }),
            };

            respond(&response)?;
        }

        // Stdin closed, which means the agent is gone. Anything this server
        // started should go with it rather than linger.
        self.stop_all();

        Ok(())
    }

    fn dispatch(&mut self, method: &str, params: Value) -> Result<Option<Value>, RpcError> {
        match method {
            "initialize" => Ok(Some(self.initialize(params))),
            "ping" => Ok(Some(json!({}))),
            "tools/list" => Ok(Some(json!({ "tools": self.tool_definitions() }))),
            "tools/call" => self.call_tool(params).map(Some),
            // Notifications and capabilities this server does not implement.
            "notifications/initialized" | "notifications/cancelled" => Ok(None),
            "resources/list" => Ok(Some(json!({ "resources": [] }))),
            "prompts/list" => Ok(Some(json!({ "prompts": [] }))),
            other => Err(RpcError::method_not_found(other)),
        }
    }

    fn initialize(&self, params: Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(LATEST_PROTOCOL_VERSION);

        let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
            requested
        } else {
            LATEST_PROTOCOL_VERSION
        };

        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "roxo",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions":
                "Roxo syncs a filesystem project into Roblox Studio. Call roxo_sessions to see \
                 what is running. Before assuming a file change reached Studio, check \
                 roxo_status: a running server does not imply a connected editor. Use \
                 roxo_wait_for_studio after starting a session to block until Studio attaches.",
        })
    }

    fn tool_definitions(&self) -> Vec<Value> {
        let port_property = json!({
            "type": "integer",
            "description":
                "Port of the serve session. Only required when more than one session is running.",
        });

        let mut tools = vec![
            json!({
                "name": "roxo_sessions",
                "description":
                    "List the Roxo serve sessions running on this machine, with the project each \
                     one serves and where it lives on disk.",
                "inputSchema": { "type": "object", "properties": {} },
            }),
            json!({
                "name": "roxo_status",
                "description":
                    "Report a serve session's state, including whether Studio is actually \
                     connected. Check this before assuming file changes reached Studio.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "port": port_property,
                        "project": {
                            "type": "string",
                            "description": "Path to the project, as an alternative to port.",
                        },
                    },
                },
            }),
            json!({
                "name": "roxo_wait_for_studio",
                "description":
                    "Block until a Studio client attaches to a serve session, or until the \
                     timeout expires. Use after starting a session.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "port": port_property,
                        "timeoutSeconds": {
                            "type": "integer",
                            "description": "How long to wait before giving up. Defaults to 60.",
                        },
                    },
                },
            }),
        ];

        if self.read_only {
            return tools;
        }

        tools.push(json!({
            "name": "roxo_serve_start",
            "description":
                "Start a serve session for a project so Studio can sync with it. Returns once the \
                 server is listening. The session stops when this MCP server exits.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": {
                        "type": "string",
                        "description":
                            "Path to the project directory or .project.json file to serve.",
                    },
                    "port": {
                        "type": "integer",
                        "description": "Port to listen on. Defaults to the project's preference.",
                    },
                    "autoConnect": {
                        "type": "string",
                        "enum": ["off", "matching", "always"],
                        "description":
                            "Auto-connect policy. 'matching' only lets places this project proves \
                             it belongs to connect unattended. Use 'always' only for unpublished \
                             places, which have no place ID to match against.",
                    },
                },
                "required": ["project"],
            },
        }));

        tools.push(json!({
            "name": "roxo_serve_stop",
            "description": "Stop a serve session that this MCP server started.",
            "inputSchema": {
                "type": "object",
                "properties": { "port": port_property },
                "required": ["port"],
            },
        }));

        tools.push(json!({
            "name": "roxo_build",
            "description":
                "Build a project into a place or model file without needing Studio open.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Path to the project." },
                    "output": { "type": "string", "description": "Path of the file to write." },
                },
                "required": ["project", "output"],
            },
        }));

        tools.push(json!({
            "name": "roxo_sourcemap",
            "description":
                "Generate a sourcemap describing how files map onto Roblox instances. Use it to \
                 translate an instance path into the file that defines it.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Path to the project." },
                    "includeScriptPaths": {
                        "type": "boolean",
                        "description": "Include file paths for scripts. Defaults to true.",
                    },
                },
                "required": ["project"],
            },
        }));

        tools
    }

    fn call_tool(&mut self, params: Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::invalid_params("Missing tool name"))?
            .to_owned();

        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        if self.read_only
            && !matches!(
                name.as_str(),
                "roxo_sessions" | "roxo_status" | "roxo_wait_for_studio"
            )
        {
            return Ok(tool_error(format!(
                "'{name}' is unavailable because this Roxo MCP server was started with --read-only."
            )));
        }

        let outcome = match name.as_str() {
            "roxo_sessions" => self.tool_sessions(),
            "roxo_status" => self.tool_status(&arguments),
            "roxo_wait_for_studio" => self.tool_wait_for_studio(&arguments),
            "roxo_serve_start" => self.tool_serve_start(&arguments),
            "roxo_serve_stop" => self.tool_serve_stop(&arguments),
            "roxo_build" => self.tool_build(&arguments),
            "roxo_sourcemap" => self.tool_sourcemap(&arguments),
            other => return Err(RpcError::invalid_params(format!("Unknown tool '{other}'"))),
        };

        // A tool that fails is reported through the result rather than as a
        // protocol error, so the agent can read the reason and adjust instead
        // of treating it as a broken connection.
        Ok(match outcome {
            Ok(value) => tool_ok(value),
            Err(err) => tool_error(format!("{err:#}")),
        })
    }

    fn tool_sessions(&self) -> anyhow::Result<Value> {
        let sessions = session_registry::list()?;

        if sessions.is_empty() {
            return Ok(json!({
                "sessions": [],
                "hint": "No serve sessions are running. Start one with roxo_serve_start.",
            }));
        }

        Ok(json!({ "sessions": sessions }))
    }

    fn tool_status(&self, arguments: &Value) -> anyhow::Result<Value> {
        let status = fetch_status(port_of(arguments), project_of(arguments).as_deref())?;

        Ok(serde_json::to_value(status)?)
    }

    fn tool_wait_for_studio(&self, arguments: &Value) -> anyhow::Result<Value> {
        let timeout = arguments
            .get("timeoutSeconds")
            .and_then(Value::as_u64)
            .unwrap_or(60);

        let port = port_of(arguments);
        let deadline = Instant::now() + Duration::from_secs(timeout);
        let mut last_error = None;

        loop {
            match fetch_status(port, None) {
                Ok(status) if status.studio_connected => {
                    return Ok(serde_json::to_value(status)?);
                }
                Ok(_) => {}
                Err(err) => last_error = Some(err),
            }

            if Instant::now() >= deadline {
                anyhow::bail!(
                    "Studio did not connect within {timeout}s. Check that the place is open in \
                     Studio with the Roxo plugin installed, and that the project's auto-connect \
                     policy is not 'off'.{}",
                    last_error
                        .map(|err| format!(" Last attempt: {err:#}"))
                        .unwrap_or_default()
                );
            }

            thread::sleep(Duration::from_millis(500));
        }
    }

    fn tool_serve_start(&mut self, arguments: &Value) -> anyhow::Result<Value> {
        let project = project_of(arguments)
            .ok_or_else(|| anyhow::anyhow!("The 'project' argument is required"))?;

        let mut command = Command::new(std::env::current_exe()?);
        command.arg("serve").arg(&project);

        if let Some(port) = arguments.get("port").and_then(Value::as_u64) {
            command.arg("--port").arg(port.to_string());
        }

        if let Some(policy) = arguments.get("autoConnect").and_then(Value::as_str) {
            command.arg("--auto-connect").arg(policy);
        }

        // The server's own output is not the agent's channel; discard it so it
        // can never interleave with this process's JSON-RPC stream.
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        let pid = child.id();

        // Wait for the session to advertise itself rather than returning
        // optimistically, so a project that fails to load is reported as a
        // failure here instead of as a confusing timeout later.
        let deadline = Instant::now() + SERVE_START_TIMEOUT;
        let started = loop {
            if let Some(session) = session_registry::list()?
                .into_iter()
                .find(|session| session.pid == pid)
            {
                break Some(session);
            }

            if Instant::now() >= deadline {
                break None;
            }

            thread::sleep(Duration::from_millis(200));
        };

        match started {
            Some(session) => {
                self.spawned.insert(session.port, child);

                Ok(json!({
                    "started": true,
                    "session": session,
                    "hint":
                        "The server is listening, but Studio is not necessarily connected. Call \
                         roxo_wait_for_studio to confirm before relying on the sync.",
                }))
            }
            None => {
                let mut child = child;
                let _ = child.kill();
                let _ = child.wait();

                anyhow::bail!(
                    "The server for '{}' did not start within {}s. The project may have failed to \
                     load, or the port may already be in use. Run `roxo serve` on it directly to \
                     see the error.",
                    project.display(),
                    SERVE_START_TIMEOUT.as_secs()
                )
            }
        }
    }

    fn tool_serve_stop(&mut self, arguments: &Value) -> anyhow::Result<Value> {
        let port = arguments
            .get("port")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("The 'port' argument is required"))?
            as u16;

        match self.spawned.remove(&port) {
            Some(mut child) => {
                child.kill()?;
                child.wait()?;

                Ok(json!({ "stopped": true, "port": port }))
            }
            // Killing a server this process did not start would let an agent
            // silently take down a developer's own session.
            None => anyhow::bail!(
                "No serve session on port {port} was started by this MCP server. Only sessions \
                 started with roxo_serve_start can be stopped here."
            ),
        }
    }

    fn tool_build(&self, arguments: &Value) -> anyhow::Result<Value> {
        let project = project_of(arguments)
            .ok_or_else(|| anyhow::anyhow!("The 'project' argument is required"))?;

        let output = arguments
            .get("output")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("The 'output' argument is required"))?;

        let result = Command::new(std::env::current_exe()?)
            .arg("build")
            .arg(&project)
            .arg("--output")
            .arg(output)
            .output()?;

        if !result.status.success() {
            anyhow::bail!(
                "Build failed:\n{}",
                String::from_utf8_lossy(&result.stderr).trim()
            );
        }

        Ok(json!({ "built": true, "output": output }))
    }

    fn tool_sourcemap(&self, arguments: &Value) -> anyhow::Result<Value> {
        let project = project_of(arguments)
            .ok_or_else(|| anyhow::anyhow!("The 'project' argument is required"))?;

        let mut command = Command::new(std::env::current_exe()?);
        command.arg("sourcemap").arg(&project);

        if arguments
            .get("includeScriptPaths")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            command.arg("--include-non-scripts");
        }

        let result = command.output()?;

        if !result.status.success() {
            anyhow::bail!(
                "Sourcemap generation failed:\n{}",
                String::from_utf8_lossy(&result.stderr).trim()
            );
        }

        Ok(serde_json::from_slice(&result.stdout)?)
    }

    fn stop_all(&mut self) {
        for (_port, mut child) in self.spawned.drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn port_of(arguments: &Value) -> Option<u16> {
    arguments
        .get("port")
        .and_then(Value::as_u64)
        .map(|port| port as u16)
}

fn project_of(arguments: &Value) -> Option<PathBuf> {
    arguments
        .get("project")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

/// Wraps a successful tool result in MCP's content envelope.
///
/// The value is sent as pretty JSON text because that is what every MCP client
/// can render, and it keeps the payload readable in an agent's transcript.
fn tool_ok(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());

    json!({ "content": [{ "type": "text", "text": text }] })
}

fn tool_error(message: String) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

fn respond(response: &Value) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();

    serde_json::to_writer(&mut stdout, response)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;

    Ok(())
}

#[derive(Debug)]
struct RpcError {
    code: i32,
    message: String,
}

impl RpcError {
    fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Method not found: {method}"),
        }
    }

    fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn read_only_servers_hide_mutating_tools() {
        let names = |read_only: bool| -> Vec<String> {
            McpServer::new(read_only)
                .tool_definitions()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap().to_owned())
                .collect()
        };

        let read_only = names(true);
        assert!(read_only.contains(&"roxo_status".to_owned()));
        assert!(!read_only.contains(&"roxo_serve_start".to_owned()));

        assert!(names(false).contains(&"roxo_serve_start".to_owned()));
    }

    #[test]
    fn initialize_echoes_a_supported_protocol_version() {
        let server = McpServer::new(false);

        let result = server.initialize(json!({ "protocolVersion": "2024-11-05" }));
        assert_eq!(result["protocolVersion"], "2024-11-05");

        // An unknown revision falls back to the newest one we implement.
        let result = server.initialize(json!({ "protocolVersion": "1999-01-01" }));
        assert_eq!(result["protocolVersion"], LATEST_PROTOCOL_VERSION);
    }

    #[test]
    fn notifications_produce_no_result() {
        let mut server = McpServer::new(false);

        let result = server
            .dispatch("notifications/initialized", Value::Null)
            .unwrap();

        assert!(result.is_none());
    }

    #[test]
    fn unknown_methods_are_reported_as_method_not_found() {
        let mut server = McpServer::new(false);

        let error = server.dispatch("does/not/exist", Value::Null).unwrap_err();

        assert_eq!(error.code, -32601);
    }

    #[test]
    fn stopping_a_session_this_server_did_not_start_is_refused() {
        let mut server = McpServer::new(false);

        let error = server
            .tool_serve_stop(&json!({ "port": 34872 }))
            .unwrap_err();

        assert!(error.to_string().contains("started by this MCP server"));
    }
}
