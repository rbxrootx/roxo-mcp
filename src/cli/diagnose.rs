//! Turns a session's status into an answer to "why isn't Studio connected?".
//!
//! The status endpoint reports facts: clients, probes, and the plugin's
//! verdicts. An agent then has to know that `not-in-serve-place-ids` means
//! "edit this project file and restart", and that an empty probe list means
//! the plugin never looked. This does that interpretation once, so `roxo
//! status`, `roxo wait`, and the MCP tools all give the same advice.
//!
//! It only ever suggests changes. In particular, a suggested `servePlaceIds`
//! edit is the explicit project-file opt-in the auto-connect rules require; it
//! is never applied on anyone's behalf.

use serde::Serialize;
use serde_json::{json, Value};

use crate::{web_api::SessionStatusResponse, ProbeInfo};

use super::status::{describe_place, now_secs};

/// The port ranges the Studio plugin scans by default. Mirrors
/// `defaultPortRanges` in `plugin/src/Config.lua`; keep the two in step.
pub const PLUGIN_DEFAULT_PORT_RANGES: &[(u16, u16)] = &[(34370, 34379), (34872, 34881)];

/// A verdict older than this, relative to the latest probe from the same
/// place, no longer describes what the plugin is doing: it probed again and
/// did not decline, so it has most likely connected or is connecting.
const STALE_VERDICT_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagnosisState {
    /// A Studio client holds a live subscription.
    Connected,
    /// A place probed this server and declined it.
    Declined,
    /// A place found this server and another one both claiming it, and so
    /// connected to neither.
    Ambiguous,
    /// A place probed this server but gave no verdict.
    Probed,
    /// No Roxo plugin has probed this server recently.
    NotProbed,
}

/// An edit to the served project file that would let the place connect.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileChange {
    pub file: String,
    /// Top-level keys to set, with their complete new values.
    pub set: Value,
    /// Whether the serve session must be restarted to see the edit. The
    /// project file's identity fields are read once at startup.
    pub requires_restart: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnosis {
    pub state: DiagnosisState,
    pub summary: String,

    /// What to do about it, if anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_file_change: Option<ProjectFileChange>,

    /// Why the fix should not be applied blindly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caution: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_id: Option<u64>,

    /// The plugin's stable reason code, when it gave one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,

    /// Seconds since the place this is about last probed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_probe_seconds_ago: Option<u64>,
}

const SERVE_PLACE_IDS_CAUTION: &str =
    "Only list this place if it really belongs to this project. A listed place connects without \
     confirmation, and the project's source then overwrites what is in it.";

impl Diagnosis {
    fn new(state: DiagnosisState, summary: String) -> Self {
        Self {
            state,
            summary,
            fix: None,
            project_file_change: None,
            caution: None,
            place_id: None,
            reason_code: None,
            last_probe_seconds_ago: None,
        }
    }
}

/// Diagnoses a session. `port` is the session's port when known, used to tell
/// whether the plugin would scan it at all.
pub fn diagnose(status: &SessionStatusResponse, port: Option<u16>) -> Diagnosis {
    diagnose_at(status, port, now_secs())
}

fn diagnose_at(status: &SessionStatusResponse, port: Option<u16>, now: u64) -> Diagnosis {
    if status.studio_connected {
        return connected(status);
    }

    // Probes arrive most recent first. Prefer a published place that gave a
    // verdict, since that is the one with something to act on.
    let probe = status
        .recent_probes
        .iter()
        .find(|probe| fresh_verdict(probe).is_some())
        .or_else(|| status.recent_probes.first());

    let Some(probe) = probe else {
        return not_probed(port);
    };

    let mut diagnosis = match fresh_verdict(probe) {
        Some(verdict) if verdict.outcome == "ambiguous" => {
            let mut diagnosis = Diagnosis::new(
                DiagnosisState::Ambiguous,
                format!(
                    "{} found more than one server claiming it, so it connected to none: {}",
                    describe_place(probe.place_id),
                    verdict.reason
                ),
            );
            diagnosis.fix = Some(format!(
                "Stop the other serve sessions for this place{}, or remove the place from their \
                 servePlaceIds. Roxo never picks between two claims.",
                if verdict.conflicting_ports.is_empty() {
                    String::new()
                } else {
                    format!(
                        " (ports {})",
                        verdict
                            .conflicting_ports
                            .iter()
                            .map(u16::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }
            ));
            diagnosis
        }
        Some(verdict) => {
            let mut diagnosis = Diagnosis::new(
                DiagnosisState::Declined,
                format!(
                    "{} probed this server and declined it: {}",
                    describe_place(probe.place_id),
                    verdict.reason
                ),
            );
            apply_fix(
                &mut diagnosis,
                status,
                probe.place_id,
                verdict.code.as_deref(),
            );
            diagnosis
        }
        None => {
            let mut diagnosis = Diagnosis::new(
                DiagnosisState::Probed,
                format!(
                    "{} probed this server but did not say it declined.",
                    describe_place(probe.place_id)
                ),
            );
            diagnosis.fix = Some(
                "If it keeps probing without connecting, its plugin may predate verdict reports: \
                 update it with `roxo plugin install` and restart Studio. If it just started \
                 connecting, wait a few seconds and check again."
                    .to_owned(),
            );
            diagnosis
        }
    };

    diagnosis.place_id = probe.place_id;
    diagnosis.reason_code = fresh_verdict(probe).and_then(|verdict| verdict.code.clone());
    diagnosis.last_probe_seconds_ago = Some(now.saturating_sub(probe.last_probe_at));
    diagnosis
}

fn fresh_verdict(probe: &ProbeInfo) -> Option<&crate::ProbeVerdict> {
    probe
        .verdict
        .as_ref()
        .filter(|verdict| verdict.reported_at + STALE_VERDICT_SECS >= probe.last_probe_at)
}

fn connected(status: &SessionStatusResponse) -> Diagnosis {
    let client = status.clients.iter().find(|client| client.subscribed);

    let summary = match client {
        Some(client) => {
            let how = match (&client.match_reason, client.auto_connected) {
                (Some(reason), true) => format!("auto-connected via {reason}"),
                (None, true) => "auto-connected".to_owned(),
                _ => "connected manually".to_owned(),
            };
            format!(
                "Studio is connected: {}, {how}.",
                describe_place(client.place_id)
            )
        }
        None => "Studio is connected.".to_owned(),
    };

    let mut diagnosis = Diagnosis::new(DiagnosisState::Connected, summary);
    diagnosis.place_id = client.and_then(|client| client.place_id);
    diagnosis
}

fn not_probed(port: Option<u16>) -> Diagnosis {
    let ranges = PLUGIN_DEFAULT_PORT_RANGES
        .iter()
        .map(|(low, high)| format!("{low}-{high}"))
        .collect::<Vec<_>>()
        .join(", ");

    let outside = port.filter(|port| {
        !PLUGIN_DEFAULT_PORT_RANGES
            .iter()
            .any(|(low, high)| (low..=high).contains(&port))
    });

    let mut diagnosis = Diagnosis::new(
        DiagnosisState::NotProbed,
        "No Roxo plugin has probed this server in the last few minutes, so no Studio place has \
         even looked at it."
            .to_owned(),
    );

    diagnosis.fix = Some(match outside {
        Some(port) => format!(
            "Port {port} is outside the plugin's default scan ranges ({ranges}). Serve on a port \
             inside them, add {port} to the plugin's auto-connect ports setting, or keep another \
             Roxo session running inside the ranges: a current plugin probes every session that \
             one lists as a peer."
        ),
        None => format!(
            "Check that Studio is open with the place loaded, that the Roxo plugin is installed \
             (`roxo plugin install`) with auto-connect on, and that its auto-connect ports \
             setting covers this port (default: {ranges})."
        ),
    });

    diagnosis
}

/// Adds the remedy for a decline reason. Codes come from the plugin's
/// `AutoConnect.RejectCode`.
fn apply_fix(
    diagnosis: &mut Diagnosis,
    status: &SessionStatusResponse,
    place_id: Option<u64>,
    code: Option<&str>,
) {
    let project = status.project_path.clone();
    let published_place = place_id.filter(|id| *id != 0);

    let list_place = |diagnosis: &mut Diagnosis, mut set: Value| {
        if let Some(place) = published_place {
            let mut ids: Vec<u64> = status.serve_place_ids.iter().flatten().copied().collect();
            ids.push(place);
            ids.sort_unstable();
            ids.dedup();
            set["servePlaceIds"] = json!(ids);
        }

        diagnosis.project_file_change = Some(ProjectFileChange {
            file: project.clone(),
            set,
            requires_restart: true,
        });
        diagnosis.caution = Some(SERVE_PLACE_IDS_CAUTION.to_owned());
    };

    match code {
        Some("not-in-serve-place-ids" | "no-identity-match" | "always-on-published-place") => {
            diagnosis.fix = Some(format!(
                "Add this place to servePlaceIds in {project} and restart the serve session, or \
                 connect once by hand in Studio to pair them."
            ));
            list_place(diagnosis, json!({}));
        }
        Some("auto-connect-off") => {
            diagnosis.fix = Some(format!(
                "The project turns auto-connect off. Set \"autoConnect\": \"matching\" in \
                 {project} (or drop --auto-connect off), make sure servePlaceIds lists this place, \
                 and restart the serve session."
            ));
            list_place(diagnosis, json!({ "autoConnect": "matching" }));
        }
        Some("unpublished-place") => {
            diagnosis.fix = Some(format!(
                "An unpublished place has no ID to match. If this is a scratch project, set \
                 \"autoConnect\": \"always\" in {project} and restart the serve session; it only \
                 ever applies to unpublished places. Otherwise publish the place, or connect \
                 once by hand."
            ));
            diagnosis.project_file_change = Some(ProjectFileChange {
                file: project,
                set: json!({ "autoConnect": "always" }),
                requires_restart: true,
            });
        }
        Some("blocked-place") => {
            diagnosis.fix = Some(format!(
                "The project lists this place in blockedPlaceIds. That is deliberate in {project}; \
                 remove it from that list only if you do mean to sync into it."
            ));
        }
        Some("protocol-mismatch") => {
            diagnosis.fix = Some(
                "The plugin and server speak different protocol versions. Update the plugin with \
                 `roxo plugin install` and restart Studio, or update the server."
                    .to_owned(),
            );
        }
        _ => {
            diagnosis.fix = Some(
                "See the reason above. Update the plugin with `roxo plugin install` for more \
                 specific diagnoses."
                    .to_owned(),
            );
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::{ProbeInfo, ProbeVerdict};

    fn status() -> SessionStatusResponse {
        serde_json::from_value(json!({
            "sessionId": "00000000-0000-0000-0000-000000000000",
            "serverName": "roxo",
            "serverVersion": "7.7.0",
            "protocolVersion": 5,
            "projectName": "Game",
            "projectId": "roxo-1",
            "projectPath": "C:/game/default.project.json",
            "placeId": null,
            "gameId": null,
            "servePlaceIds": [1],
            "autoConnect": "matching",
            "uptimeSeconds": 10,
            "studioConnected": false,
            "clientCount": 0,
            "clients": [],
            "lastPatchAt": null,
            "patchesSent": 0,
        }))
        .unwrap()
    }

    fn probe(place_id: u64, last: u64, verdict: Option<(&str, &str, u64)>) -> ProbeInfo {
        ProbeInfo {
            place_id: Some(place_id),
            game_id: None,
            client_version: None,
            remote_addr: String::new(),
            first_probe_at: last,
            last_probe_at: last,
            probe_count: 1,
            verdict: verdict.map(|(outcome, code, at)| ProbeVerdict {
                outcome: outcome.to_owned(),
                code: Some(code.to_owned()),
                reason: "because".to_owned(),
                conflicting_ports: vec![34374, 34375],
                reported_at: at,
            }),
        }
    }

    #[test]
    fn nothing_probing_is_told_apart_from_a_refusal() {
        let diagnosis = diagnose_at(&status(), Some(34375), 1000);
        assert_eq!(diagnosis.state, DiagnosisState::NotProbed);
        assert!(!diagnosis.fix.unwrap().contains("outside"));
    }

    #[test]
    fn a_port_outside_the_scan_ranges_is_called_out() {
        let diagnosis = diagnose_at(&status(), Some(40000), 1000);
        assert!(diagnosis.fix.unwrap().contains("Port 40000 is outside"));
    }

    #[test]
    fn an_unlisted_place_gets_the_exact_project_change() {
        let mut status = status();
        status.recent_probes = vec![probe(
            99657347679962,
            997,
            Some(("rejected", "not-in-serve-place-ids", 997)),
        )];

        let diagnosis = diagnose_at(&status, Some(34375), 1000);

        assert_eq!(diagnosis.state, DiagnosisState::Declined);
        assert_eq!(diagnosis.place_id, Some(99657347679962));
        assert_eq!(diagnosis.last_probe_seconds_ago, Some(3));
        assert_eq!(
            diagnosis.reason_code.as_deref(),
            Some("not-in-serve-place-ids")
        );

        let change = diagnosis.project_file_change.unwrap();
        assert_eq!(change.file, "C:/game/default.project.json");
        // Existing entries are kept; the place is added, not substituted.
        assert_eq!(change.set["servePlaceIds"], json!([1, 99657347679962u64]));
        assert!(change.requires_restart);
        assert!(diagnosis.caution.is_some());
    }

    #[test]
    fn an_unpublished_place_is_never_offered_a_place_id_change() {
        let mut status = status();
        status.recent_probes = vec![probe(0, 997, Some(("rejected", "unpublished-place", 997)))];

        let change = diagnose_at(&status, None, 1000)
            .project_file_change
            .unwrap();

        assert_eq!(change.set, json!({ "autoConnect": "always" }));
    }

    #[test]
    fn ambiguity_names_the_conflicting_ports_and_suggests_no_edit() {
        let mut status = status();
        status.recent_probes = vec![probe(5, 997, Some(("ambiguous", "ambiguous", 997)))];

        let diagnosis = diagnose_at(&status, None, 1000);

        assert_eq!(diagnosis.state, DiagnosisState::Ambiguous);
        assert!(diagnosis.fix.unwrap().contains("34374, 34375"));
        assert!(diagnosis.project_file_change.is_none());
    }

    #[test]
    fn a_stale_verdict_is_not_reported_as_current() {
        let mut status = status();
        status.recent_probes = vec![probe(
            5,
            900,
            Some(("rejected", "not-in-serve-place-ids", 800)),
        )];

        let diagnosis = diagnose_at(&status, None, 1000);
        assert_eq!(diagnosis.state, DiagnosisState::Probed);
        assert!(diagnosis.project_file_change.is_none());
    }
}
