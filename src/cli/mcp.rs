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

use super::{
    diagnose::{diagnose, DiagnosisState, PLUGIN_DEFAULT_PORT_RANGES},
    status::{fetch_session_status, now_secs, with_diagnosis},
    GlobalOptions,
};

/// The newest MCP revision this server implements. A client asking for an older
/// revision is answered in its own revision, since nothing here depends on
/// features added between them.
const LATEST_PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The first revision with `structuredContent` in tool results.
const STRUCTURED_CONTENT_VERSION: &str = "2025-06-18";

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

    /// The MCP revision agreed in `initialize`, which decides whether results
    /// may carry `structuredContent`.
    protocol_version: String,
}

impl McpServer {
    fn new(read_only: bool) -> Self {
        Self {
            read_only,
            spawned: HashMap::new(),
            protocol_version: LATEST_PROTOCOL_VERSION.to_owned(),
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

    fn initialize(&mut self, params: Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or(LATEST_PROTOCOL_VERSION);

        let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
            requested
        } else {
            LATEST_PROTOCOL_VERSION
        };
        self.protocol_version = version.to_owned();

        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "roxo",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "instructions":
                "Roxo syncs a filesystem project into Roblox Studio. The usual loop is: \
                 roxo_sessions to see what is running, roxo_serve_start if nothing serves the \
                 project, then roxo_wait_for_studio. A running server does not imply a connected \
                 editor, so check roxo_status before assuming a file change reached Studio. \
                 When Studio is not connected, the `diagnosis` in those results says why \
                 (nothing probed the port, or a place probed and declined) and gives the fix, \
                 including the exact project-file change when one would help. Roxo never \
                 connects a place the project does not identify, so do not apply a suggested \
                 servePlaceIds change unless the place really belongs to the project.",
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
                    "Report a serve session's state: whether Studio is actually connected, which \
                     Studio places probed it recently, and a `diagnosis` with `state` \
                     (connected, declined, ambiguous, probed, not-probed), a `fix`, and when \
                     useful a `projectFileChange`. Check this before assuming file changes \
                     reached Studio.",
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
                    "Block until a Studio client attaches to a serve session. Returns the status \
                     once connected. Fails early when a Studio place reports that it declined \
                     the session, and on timeout, in both cases with a `diagnosis` saying why \
                     and how to fix it. Use after roxo_serve_start.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "port": port_property,
                        "project": {
                            "type": "string",
                            "description": "Path to the project, as an alternative to port.",
                        },
                        "failFast": {
                            "type": "boolean",
                            "description":
                                "Return as soon as a place declines the session instead of \
                                 waiting out the timeout. Defaults to true.",
                        },
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

        // Waiting can end without Studio connecting: a failed call that still
        // carries a structured diagnosis for the agent to act on.
        if name == "roxo_wait_for_studio" {
            return Ok(match self.tool_wait_for_studio(&arguments) {
                Ok((value, connected)) => self.tool_result(value, !connected),
                Err(err) => tool_error(format!("{err:#}")),
            });
        }

        let outcome = match name.as_str() {
            "roxo_sessions" => self.tool_sessions(),
            "roxo_status" => self.tool_status(&arguments),
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
            Ok(value) => self.tool_result(value, false),
            Err(err) => tool_error(format!("{err:#}")),
        })
    }

    /// Wraps a tool's result in MCP's content envelope.
    ///
    /// The value is sent as pretty JSON text, which every MCP client can
    /// render, and also as `structuredContent` to clients on a revision that
    /// defines it, so they need not parse the text.
    fn tool_result(&self, value: Value, is_error: bool) -> Value {
        let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
        let mut result = json!({ "content": [{ "type": "text", "text": text }] });

        // Revisions are dates, so they order as strings.
        if self.protocol_version.as_str() >= STRUCTURED_CONTENT_VERSION && value.is_object() {
            result["structuredContent"] = value;
        }
        if is_error {
            result["isError"] = json!(true);
        }

        result
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
        let (session, status) =
            fetch_session_status(port_of(arguments), project_of(arguments).as_deref())?;

        with_diagnosis(&status, &diagnose(&status, Some(session.port)))
    }

    /// Returns the result and whether Studio connected.
    fn tool_wait_for_studio(&self, arguments: &Value) -> anyhow::Result<(Value, bool)> {
        let timeout = arguments
            .get("timeoutSeconds")
            .and_then(Value::as_u64)
            .unwrap_or(60);
        let fail_fast = arguments
            .get("failFast")
            .and_then(Value::as_bool)
            .unwrap_or(true);

        let port = port_of(arguments);
        let project = project_of(arguments);
        let started_at = now_secs();
        let deadline = Instant::now() + Duration::from_secs(timeout);
        let mut last_error = None;
        let mut last_report = None;

        loop {
            match fetch_session_status(port, project.as_deref()) {
                Ok((session, status)) => {
                    let diagnosis = diagnose(&status, Some(session.port));
                    let report = with_diagnosis(&status, &diagnosis)?;

                    if status.studio_connected {
                        return Ok((report, true));
                    }

                    // Only a verdict given after this wait began counts. One
                    // from before may predate a fix the agent just applied.
                    let fresh_decline = matches!(
                        diagnosis.state,
                        DiagnosisState::Declined | DiagnosisState::Ambiguous
                    ) && status.recent_probes.iter().any(|probe| {
                        probe
                            .verdict
                            .as_ref()
                            .is_some_and(|verdict| verdict.reported_at > started_at)
                    });

                    if fail_fast && fresh_decline {
                        return Ok((wait_failure("declined", timeout, report), false));
                    }

                    last_report = Some(report);
                }
                Err(err) => last_error = Some(err),
            }

            if Instant::now() >= deadline {
                return match last_report {
                    Some(report) => Ok((wait_failure("timeout", timeout, report), false)),
                    None => anyhow::bail!(
                        "No serve session answered within {timeout}s.{}",
                        last_error
                            .map(|err| format!(" Last attempt: {err:#}"))
                            .unwrap_or_default()
                    ),
                };
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

                let scanned = PLUGIN_DEFAULT_PORT_RANGES
                    .iter()
                    .any(|(low, high)| (*low..=*high).contains(&session.port));

                let hint = if scanned {
                    "The server is listening, but Studio is not necessarily connected. Call \
                     roxo_wait_for_studio to confirm before relying on the sync."
                } else {
                    "The server is listening on a port outside the Studio plugin's default scan \
                     ranges. A current plugin still finds it through any other Roxo session \
                     inside them; otherwise restart it on a port inside them. Call \
                     roxo_wait_for_studio to confirm."
                };

                Ok(json!({
                    "started": true,
                    "session": session,
                    "inPluginScanRange": scanned,
                    "hint": hint,
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

/// The result of a wait that ended without Studio connecting. The diagnosis is
/// lifted to the top so it is the first thing an agent reads.
fn wait_failure(outcome: &str, timeout: u64, mut report: Value) -> Value {
    let diagnosis = report
        .as_object_mut()
        .and_then(|report| report.remove("diagnosis"))
        .unwrap_or(Value::Null);

    json!({
        "connected": false,
        "outcome": outcome,
        "timeoutSeconds": timeout,
        "diagnosis": diagnosis,
        "status": report,
    })
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
        let mut server = McpServer::new(false);

        let result = server.initialize(json!({ "protocolVersion": "2024-11-05" }));
        assert_eq!(result["protocolVersion"], "2024-11-05");

        // An unknown revision falls back to the newest one we implement.
        let result = server.initialize(json!({ "protocolVersion": "1999-01-01" }));
        assert_eq!(result["protocolVersion"], LATEST_PROTOCOL_VERSION);
    }

    #[test]
    fn structured_content_is_only_sent_to_revisions_that_define_it() {
        let mut server = McpServer::new(false);
        let value = json!({ "studioConnected": false });

        server.initialize(json!({ "protocolVersion": "2025-06-18" }));
        let result = server.tool_result(value.clone(), false);
        assert_eq!(result["structuredContent"], value);
        assert!(result.get("isError").is_none());

        server.initialize(json!({ "protocolVersion": "2025-03-26" }));
        let result = server.tool_result(value, true);
        assert!(result.get("structuredContent").is_none());
        assert_eq!(result["isError"], true);
        // The text form is always there for clients that only read content.
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("studioConnected"));
    }

    #[test]
    fn a_failed_wait_leads_with_the_diagnosis() {
        let report = json!({
            "studioConnected": false,
            "diagnosis": { "state": "declined", "fix": "add the place" },
        });

        let result = wait_failure("declined", 60, report);

        assert_eq!(result["connected"], false);
        assert_eq!(result["outcome"], "declined");
        assert_eq!(result["diagnosis"]["state"], "declined");
        // Not duplicated inside the status it was lifted out of.
        assert!(result["status"].get("diagnosis").is_none());
        assert_eq!(result["status"]["studioConnected"], false);
    }

    #[test]
    fn waiting_accepts_a_project_path_like_status_does() {
        let tools = McpServer::new(true).tool_definitions();
        let wait = tools
            .iter()
            .find(|tool| tool["name"] == "roxo_wait_for_studio")
            .unwrap();

        let properties = &wait["inputSchema"]["properties"];
        assert!(properties.get("project").is_some());
        assert!(properties.get("failFast").is_some());
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
