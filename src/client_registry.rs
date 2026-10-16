//! Tracks which Studio clients are attached to a serve session.
//!
//! Rojo's server is write-only from the operator's point of view: you start it
//! and it prints an address, but it never tells you whether Studio actually
//! showed up. A human doesn't need to be told, because they can see the plugin
//! window. An agent can't, so without this it has no way to distinguish "the
//! sync worked" from "nothing is listening" and will happily report success
//! into the void. Everything an agent needs to answer that question lives here.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

/// Identifies one connected client for the lifetime of its session.
pub type ClientId = u64;

/// What a client told us about itself during its handshake, plus what we
/// observed about it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    pub id: ClientId,

    /// The Roblox place the client is editing. `Some(0)` means an unpublished
    /// place, which is distinct from `None` (a client too old to tell us).
    pub place_id: Option<u64>,
    pub game_id: Option<u64>,

    /// Name and version the client reported, e.g. `roxo-plugin` / `7.7.0`.
    /// Clients that predate the handshake report as `unknown`.
    pub client_name: String,
    pub client_version: Option<String>,

    /// Whether the client reached us without a human pressing Connect, and
    /// which rule let it. Recorded so `roxo status` can show an operator
    /// exactly why an unattended connection was allowed.
    pub auto_connected: bool,
    pub match_reason: Option<String>,

    pub remote_addr: String,

    /// Seconds since the Unix epoch, chosen over `Instant` so these can cross
    /// the API boundary into an agent's JSON.
    pub connected_at: u64,
    pub last_seen: u64,

    /// Whether the client currently holds an open change subscription. This is
    /// the signal that actually means "live sync is running"; a handshake alone
    /// only means something looked at us once.
    pub subscribed: bool,
}

/// Details a client provides about itself when it connects.
#[derive(Debug, Clone, Default)]
pub struct ClientHandshake {
    pub place_id: Option<u64>,
    pub game_id: Option<u64>,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
    pub auto_connected: bool,
    pub match_reason: Option<String>,
}

/// A place that looked at this server during auto-connect discovery, and what
/// it decided.
///
/// This is what separates "Studio is open but refused this server, and here is
/// why" from "no Studio has looked at this port at all". Without it the plugin's
/// reasons stay in Studio's Output, where an agent cannot read them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInfo {
    /// `None` for a plugin too old to say which place it is.
    pub place_id: Option<u64>,
    pub game_id: Option<u64>,
    pub client_version: Option<String>,
    pub remote_addr: String,

    pub first_probe_at: u64,
    pub last_probe_at: u64,
    pub probe_count: u64,

    /// Why the place did not connect, if its plugin said. Older plugins probe
    /// without ever reporting, so absence is not proof of acceptance.
    pub verdict: Option<ProbeVerdict>,
}

/// The plugin's reason for not connecting to this server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeVerdict {
    /// `rejected` (this server does not qualify) or `ambiguous` (it qualifies,
    /// but so does another server, so neither was chosen).
    pub outcome: String,

    /// A stable identifier for the reason, such as `not-in-serve-place-ids`,
    /// for tooling to branch on instead of parsing `reason`.
    pub code: Option<String>,
    pub reason: String,

    /// For `ambiguous`, the ports of every server that claimed the place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicting_ports: Vec<u16>,

    pub reported_at: u64,
}

/// What the plugin sends to `/api/roxo/probe-report`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeReport {
    #[serde(default, deserialize_with = "lenient_id")]
    pub place_id: Option<u64>,
    #[serde(default, deserialize_with = "lenient_id")]
    pub game_id: Option<u64>,
    pub outcome: String,
    pub code: Option<String>,
    pub reason: String,
    #[serde(default)]
    pub conflicting_ports: Vec<u16>,
}

/// Accepts an ID as a number or a string. Studio's JSON encoder cannot be
/// relied on to write a 14-digit place ID in integer form, so the plugin sends
/// IDs as strings; numbers are still accepted from other clients.
fn lenient_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Number(u64),
        Text(String),
    }

    Ok(match Option::<Repr>::deserialize(deserializer)? {
        Some(Repr::Number(id)) => Some(id),
        Some(Repr::Text(text)) => Some(text.trim().parse().map_err(serde::de::Error::custom)?),
        None => None,
    })
}

type ProbeKey = (Option<u64>, Option<u64>);

#[derive(Debug, Default)]
pub struct ClientRegistry {
    clients: Mutex<HashMap<ClientId, ClientInfo>>,
    next_id: AtomicU64,
    probes: Mutex<HashMap<ProbeKey, ProbeInfo>>,
}

/// Clips client-supplied text. These arrive on an unauthenticated route and are
/// held in memory and echoed to agents, so their size is bounded here.
fn clip(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

impl ClientRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// How long a client that handshook but never subscribed is remembered.
    ///
    /// Discovery makes a handshake and walks away: the plugin probes every
    /// candidate port to see what is there, and only one of those probes turns
    /// into a session. Without an expiry those probes would accumulate forever
    /// and every scan would inflate the client count.
    const UNSUBSCRIBED_TTL_SECS: u64 = 60;

    /// Records a client handshake and returns the id assigned to it.
    pub fn handshake(&self, handshake: ClientHandshake, remote_addr: String) -> ClientId {
        self.prune_stale();

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let now = now_secs();

        let info = ClientInfo {
            id,
            place_id: handshake.place_id,
            game_id: handshake.game_id,
            client_name: handshake
                .client_name
                .unwrap_or_else(|| "unknown".to_owned()),
            client_version: handshake.client_version,
            auto_connected: handshake.auto_connected,
            match_reason: handshake.match_reason,
            remote_addr,
            connected_at: now,
            last_seen: now,
            subscribed: false,
        };

        self.clients.lock().unwrap().insert(id, info);
        id
    }

    /// Marks a client as holding (or having released) a live subscription.
    ///
    /// Clients that never handshook still get an entry, so that an old plugin
    /// or a hand-rolled client still shows up as connected rather than being
    /// invisible.
    pub fn set_subscribed(&self, id: ClientId, subscribed: bool) {
        let mut clients = self.clients.lock().unwrap();

        if let Some(client) = clients.get_mut(&id) {
            client.subscribed = subscribed;
            client.last_seen = now_secs();
        }
    }

    pub fn touch(&self, id: ClientId) {
        if let Some(client) = self.clients.lock().unwrap().get_mut(&id) {
            client.last_seen = now_secs();
        }
    }

    pub fn remove(&self, id: ClientId) {
        self.clients.lock().unwrap().remove(&id);
    }

    /// Forgets clients that handshook but never established a subscription.
    ///
    /// Subscribed clients are never pruned by age; they are removed when their
    /// connection actually closes.
    pub fn prune_stale(&self) {
        let now = now_secs();

        self.clients.lock().unwrap().retain(|_id, client| {
            client.subscribed || now.saturating_sub(client.last_seen) < Self::UNSUBSCRIBED_TTL_SECS
        });
    }

    pub fn clients(&self) -> Vec<ClientInfo> {
        self.prune_stale();

        let mut clients: Vec<_> = self.clients.lock().unwrap().values().cloned().collect();
        clients.sort_by_key(|client| client.id);
        clients
    }

    /// The number of clients with a live subscription, which is what callers
    /// mean when they ask whether Studio is connected.
    pub fn subscribed_count(&self) -> usize {
        self.clients
            .lock()
            .unwrap()
            .values()
            .filter(|client| client.subscribed)
            .count()
    }

    /// How long a place's probes are remembered after the last one. Long
    /// enough to outlast the plugin's slowest poll interval many times over,
    /// short enough that a place closed a while ago stops being reported.
    const PROBE_TTL_SECS: u64 = 600;

    /// How many places are remembered at once. One per open Studio place in
    /// practice; the bound only matters against a misbehaving client.
    const MAX_PROBES: usize = 16;

    const MAX_REASON_CHARS: usize = 500;
    const MAX_FIELD_CHARS: usize = 64;

    /// Records a discovery probe. Probes are kept apart from clients: a probe
    /// is the plugin looking, not something attached.
    pub fn record_probe(&self, handshake: &ClientHandshake, remote_addr: String) {
        let now = now_secs();
        let mut probes = self.probes.lock().unwrap();

        let entry = Self::probe_entry(&mut probes, handshake.place_id, handshake.game_id, now);
        entry.last_probe_at = now;
        entry.probe_count += 1;
        entry.remote_addr = remote_addr;
        if let Some(version) = &handshake.client_version {
            entry.client_version = Some(clip(version, Self::MAX_FIELD_CHARS));
        }
    }

    /// Records why a place declined this server.
    pub fn record_probe_verdict(&self, report: ProbeReport, remote_addr: String) {
        let now = now_secs();
        let mut probes = self.probes.lock().unwrap();

        let entry = Self::probe_entry(&mut probes, report.place_id, report.game_id, now);
        if entry.remote_addr.is_empty() {
            entry.remote_addr = remote_addr;
        }

        let mut conflicting_ports = report.conflicting_ports;
        conflicting_ports.truncate(Self::MAX_PROBES);

        entry.verdict = Some(ProbeVerdict {
            outcome: clip(&report.outcome, Self::MAX_FIELD_CHARS),
            code: report.code.map(|code| clip(&code, Self::MAX_FIELD_CHARS)),
            reason: clip(&report.reason, Self::MAX_REASON_CHARS),
            conflicting_ports,
            reported_at: now,
        });
    }

    /// Recently seen probes, most recent first.
    pub fn recent_probes(&self) -> Vec<ProbeInfo> {
        let now = now_secs();
        let mut probes = self.probes.lock().unwrap();
        Self::prune_probes(&mut probes, now);

        let mut list: Vec<_> = probes.values().cloned().collect();
        list.sort_by(|a, b| b.last_probe_at.cmp(&a.last_probe_at));
        list
    }

    fn probe_entry(
        probes: &mut HashMap<ProbeKey, ProbeInfo>,
        place_id: Option<u64>,
        game_id: Option<u64>,
        now: u64,
    ) -> &mut ProbeInfo {
        Self::prune_probes(probes, now);

        let key = (place_id, game_id);
        if !probes.contains_key(&key) && probes.len() >= Self::MAX_PROBES {
            if let Some(oldest) = probes
                .iter()
                .min_by_key(|(_key, probe)| probe.last_probe_at)
                .map(|(key, _probe)| *key)
            {
                probes.remove(&oldest);
            }
        }

        probes.entry(key).or_insert_with(|| ProbeInfo {
            place_id,
            game_id,
            client_version: None,
            remote_addr: String::new(),
            first_probe_at: now,
            last_probe_at: now,
            probe_count: 0,
            verdict: None,
        })
    }

    fn prune_probes(probes: &mut HashMap<ProbeKey, ProbeInfo>, now: u64) {
        probes.retain(|_key, probe| {
            let last = probe
                .verdict
                .as_ref()
                .map_or(probe.last_probe_at, |verdict| {
                    verdict.reported_at.max(probe.last_probe_at)
                });
            now.saturating_sub(last) < Self::PROBE_TTL_SECS
        });
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn handshake_of(place_id: u64) -> ClientHandshake {
        ClientHandshake {
            place_id: Some(place_id),
            client_name: Some("roxo-plugin".to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn handshake_records_a_client() {
        let registry = ClientRegistry::new();
        let id = registry.handshake(handshake_of(123), "127.0.0.1:1".to_owned());

        let clients = registry.clients();
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].id, id);
        assert_eq!(clients[0].place_id, Some(123));
        assert_eq!(clients[0].client_name, "roxo-plugin");
    }

    #[test]
    fn only_subscribed_clients_count_as_connected() {
        let registry = ClientRegistry::new();
        let id = registry.handshake(handshake_of(123), "127.0.0.1:1".to_owned());

        // A handshake alone is not a live sync.
        assert_eq!(registry.subscribed_count(), 0);

        registry.set_subscribed(id, true);
        assert_eq!(registry.subscribed_count(), 1);

        registry.set_subscribed(id, false);
        assert_eq!(registry.subscribed_count(), 0);
    }

    #[test]
    fn stale_handshakes_are_pruned_but_subscribers_are_kept() {
        let registry = ClientRegistry::new();
        let probe = registry.handshake(handshake_of(1), "127.0.0.1:1".to_owned());
        let session = registry.handshake(handshake_of(2), "127.0.0.1:2".to_owned());

        registry.set_subscribed(session, true);

        // Age both clients past the TTL. A long-lived subscriber legitimately
        // sits quiet for hours, so only the unsubscribed probe should go.
        {
            let mut clients = registry.clients.lock().unwrap();
            for client in clients.values_mut() {
                client.last_seen = now_secs() - (ClientRegistry::UNSUBSCRIBED_TTL_SECS + 1);
            }
        }

        registry.prune_stale();

        let ids: Vec<_> = registry.clients().iter().map(|client| client.id).collect();
        assert_eq!(ids, vec![session]);
        assert!(!ids.contains(&probe));
    }

    fn probe_of(place_id: u64) -> ClientHandshake {
        ClientHandshake {
            place_id: Some(place_id),
            game_id: Some(7),
            client_name: Some("roxo-plugin-probe".to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn probes_are_counted_per_place_and_kept_apart_from_clients() {
        let registry = ClientRegistry::new();
        registry.record_probe(&probe_of(1), "127.0.0.1:1".to_owned());
        registry.record_probe(&probe_of(1), "127.0.0.1:1".to_owned());
        registry.record_probe(&probe_of(2), "127.0.0.1:2".to_owned());

        let probes = registry.recent_probes();
        assert_eq!(probes.len(), 2);
        let first = probes.iter().find(|p| p.place_id == Some(1)).unwrap();
        assert_eq!(first.probe_count, 2);
        assert!(first.verdict.is_none());

        assert!(registry.clients().is_empty());
    }

    #[test]
    fn a_verdict_attaches_to_the_place_that_probed() {
        let registry = ClientRegistry::new();
        registry.record_probe(&probe_of(1), "127.0.0.1:1".to_owned());
        registry.record_probe_verdict(
            ProbeReport {
                place_id: Some(1),
                game_id: Some(7),
                outcome: "rejected".to_owned(),
                code: Some("not-in-serve-place-ids".to_owned()),
                reason: "this place is not listed in servePlaceIds".to_owned(),
                conflicting_ports: Vec::new(),
            },
            "127.0.0.1:1".to_owned(),
        );

        let probes = registry.recent_probes();
        assert_eq!(probes.len(), 1);
        let verdict = probes[0].verdict.as_ref().unwrap();
        assert_eq!(verdict.code.as_deref(), Some("not-in-serve-place-ids"));
        assert_eq!(probes[0].probe_count, 1);
    }

    #[test]
    fn client_supplied_probe_text_is_bounded() {
        let registry = ClientRegistry::new();
        registry.record_probe_verdict(
            ProbeReport {
                place_id: Some(1),
                outcome: "x".repeat(10_000),
                reason: "y".repeat(10_000),
                conflicting_ports: (0..1000).collect(),
                ..Default::default()
            },
            "127.0.0.1:1".to_owned(),
        );

        for place in 0..100 {
            registry.record_probe(&probe_of(place), "127.0.0.1:1".to_owned());
        }

        let probes = registry.recent_probes();
        assert!(probes.len() <= ClientRegistry::MAX_PROBES);

        let all = registry.probes.lock().unwrap();
        for probe in all.values() {
            if let Some(verdict) = &probe.verdict {
                assert!(verdict.reason.chars().count() <= ClientRegistry::MAX_REASON_CHARS);
                assert!(verdict.outcome.chars().count() <= ClientRegistry::MAX_FIELD_CHARS);
                assert!(verdict.conflicting_ports.len() <= ClientRegistry::MAX_PROBES);
            }
        }
    }

    #[test]
    fn probe_reports_accept_ids_as_strings_or_numbers() {
        let as_text: ProbeReport = serde_json::from_str(
            r#"{"placeId":"99657347679962","gameId":"42","outcome":"rejected","reason":"r"}"#,
        )
        .unwrap();
        assert_eq!(as_text.place_id, Some(99657347679962));
        assert_eq!(as_text.game_id, Some(42));

        let as_number: ProbeReport =
            serde_json::from_str(r#"{"placeId":5,"outcome":"rejected","reason":"r"}"#).unwrap();
        assert_eq!(as_number.place_id, Some(5));
        assert_eq!(as_number.game_id, None);

        assert!(serde_json::from_str::<ProbeReport>(
            r#"{"placeId":"nope","outcome":"rejected","reason":"r"}"#
        )
        .is_err());
    }

    #[test]
    fn old_probes_expire() {
        let registry = ClientRegistry::new();
        registry.record_probe(&probe_of(1), "127.0.0.1:1".to_owned());

        for probe in registry.probes.lock().unwrap().values_mut() {
            probe.last_probe_at = now_secs() - (ClientRegistry::PROBE_TTL_SECS + 1);
        }

        assert!(registry.recent_probes().is_empty());
    }

    #[test]
    fn removing_a_client_drops_it_from_the_listing() {
        let registry = ClientRegistry::new();
        let first = registry.handshake(handshake_of(1), "127.0.0.1:1".to_owned());
        let second = registry.handshake(handshake_of(2), "127.0.0.1:2".to_owned());

        registry.remove(first);

        let clients = registry.clients();
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].id, second);
    }
}
