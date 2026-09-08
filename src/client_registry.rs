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

#[derive(Debug, Default)]
pub struct ClientRegistry {
    clients: Mutex<HashMap<ClientId, ClientInfo>>,
    next_id: AtomicU64,
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
