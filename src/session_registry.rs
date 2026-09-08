//! A machine-local directory of running serve sessions.
//!
//! An agent that wants to talk to Roxo has to find it first, and port scanning
//! is a poor way to do that: it is slow, it cannot tell one project from
//! another without connecting, and it silently misses any session on a
//! non-default port. Instead each `roxo serve` drops a small JSON beacon in a
//! well-known directory and removes it on exit, so discovery is a directory
//! listing.
//!
//! The Studio plugin cannot read the filesystem and still has to scan ports.
//! This registry is for everything that *can* read files: the CLI, the MCP
//! server, and any agent tooling built on top of them.

use std::{
    env,
    path::{Path, PathBuf},
    process,
};

use fs_err as fs;
use serde::{Deserialize, Serialize};

use crate::session_id::SessionId;

/// One running serve session, as advertised to other processes on this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeacon {
    pub session_id: SessionId,
    pub pid: u32,

    pub address: String,
    pub port: u16,

    pub project_name: String,
    pub project_id: String,

    /// Absolute path to the project file, so an agent can tell which checkout a
    /// session belongs to without asking the server.
    pub project_path: PathBuf,

    pub place_id: Option<u64>,
    pub game_id: Option<u64>,
    pub serve_place_ids: Option<Vec<u64>>,

    pub auto_connect: String,
    pub server_version: String,
    pub started_at: u64,
}

impl SessionBeacon {
    /// The base URL a client should use to reach this session.
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.address, self.port)
    }
}

/// Removes a session's beacon when the session ends.
///
/// A beacon that outlives its server is worse than no beacon at all, because it
/// sends agents to a dead port. Tying removal to a guard means the file goes
/// away on every ordinary exit path, and `list` prunes the rest.
pub struct BeaconGuard {
    path: PathBuf,
}

impl Drop for BeaconGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Returns the directory holding session beacons, creating it if needed.
pub fn sessions_dir() -> anyhow::Result<PathBuf> {
    let home = env::var_os("ROXO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".roxo")))
        .or_else(|| env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join(".roxo")))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Could not determine a home directory to store Roxo session state in. \
                 Set the ROXO_HOME environment variable to choose one explicitly."
            )
        })?;

    let dir = home.join("sessions");
    fs::create_dir_all(&dir)?;

    Ok(dir)
}

/// Publishes a beacon for a session and returns a guard that removes it.
pub fn publish(beacon: &SessionBeacon) -> anyhow::Result<BeaconGuard> {
    let path = sessions_dir()?.join(format!("{}.json", beacon.port));

    fs::write(&path, serde_json::to_vec_pretty(beacon)?)?;

    Ok(BeaconGuard { path })
}

/// Lists sessions believed to be running, pruning beacons whose process is
/// gone.
///
/// Liveness is checked by process id rather than by connecting, so that listing
/// sessions never blocks on a hung server.
pub fn list() -> anyhow::Result<Vec<SessionBeacon>> {
    let dir = match sessions_dir() {
        Ok(dir) => dir,
        // No directory means nothing has ever served, which is not an error.
        Err(_) => return Ok(Vec::new()),
    };

    let mut sessions = Vec::new();

    for entry in fs::read_dir(&dir)? {
        let path = entry?.path();

        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }

        let beacon: SessionBeacon = match fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        {
            Some(beacon) => beacon,
            None => {
                // A malformed or half-written beacon is not recoverable and
                // will never become valid, so drop it.
                let _ = fs::remove_file(&path);
                continue;
            }
        };

        if is_process_alive(beacon.pid) {
            sessions.push(beacon);
        } else {
            let _ = fs::remove_file(&path);
        }
    }

    sessions.sort_by_key(|session| session.port);

    Ok(sessions)
}

/// Finds a single session, optionally narrowed by port or project path.
///
/// Returns an error rather than a guess when the filter is ambiguous. Picking
/// arbitrarily here is exactly the failure the caller is trying to avoid: it
/// would point an agent's writes at whichever project happened to sort first.
pub fn resolve(port: Option<u16>, project: Option<&Path>) -> anyhow::Result<SessionBeacon> {
    let mut sessions = list()?;

    if let Some(port) = port {
        sessions.retain(|session| session.port == port);
    }

    if let Some(project) = project {
        let project = dunce::canonicalize(project).unwrap_or_else(|_| project.to_path_buf());

        sessions.retain(|session| session.project_path.starts_with(&project));
    }

    match sessions.len() {
        0 => anyhow::bail!(
            "No running Roxo serve session was found. Start one with `roxo serve`, \
             or pass --port to name a session on a specific port."
        ),
        1 => Ok(sessions.remove(0)),
        _ => {
            let listing = sessions
                .iter()
                .map(|session| format!("  port {} - {}", session.port, session.project_name))
                .collect::<Vec<_>>()
                .join("\n");

            anyhow::bail!(
                "Multiple Roxo serve sessions are running. \
                 Use --port to pick one:\n{listing}"
            )
        }
    }
}

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    if pid == process::id() {
        return true;
    }

    // Signal 0 performs the permission and existence checks without actually
    // sending anything, which is the standard way to probe for a live process.
    unsafe { libc_kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    use std::process::Command;

    if pid == process::id() {
        return true;
    }

    // Without a process-handle crate, asking the task list is the least
    // invasive check available. A failure to run it is treated as "alive" so
    // that a live session is never pruned by accident.
    match Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
    {
        Ok(output) => String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()),
        Err(_) => true,
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn the_current_process_is_alive() {
        assert!(is_process_alive(process::id()));
    }

    #[test]
    fn base_url_uses_the_advertised_address() {
        let beacon = SessionBeacon {
            session_id: SessionId::new(),
            pid: 1,
            address: "127.0.0.1".to_owned(),
            port: 34872,
            project_name: "Test".to_owned(),
            project_id: "roxo-abc".to_owned(),
            project_path: PathBuf::from("/tmp/default.project.json"),
            place_id: None,
            game_id: None,
            serve_place_ids: None,
            auto_connect: "matching".to_owned(),
            server_version: "7.7.0".to_owned(),
            started_at: 0,
        };

        assert_eq!(beacon.base_url(), "http://127.0.0.1:34872");
    }
}
