//! Identity and policy used by Roxo's auto-connect.
//!
//! Rojo requires a human to press "Connect" in Studio before a serve session
//! does anything. That makes it unusable from an AI agent, which can start
//! `roxo serve` but cannot click. Roxo lets the plugin connect on its own, but
//! only when the server can *prove* it belongs to the place that is asking.
//! This module defines that proof: a stable project identity, and the policy
//! that says how strong a match has to be before a connection happens without
//! a human in the loop.

use std::{fmt, path::Path, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// How willing a serve session is to be connected to without human
/// confirmation.
///
/// The default is deliberately *not* `Always`. An agent-friendly default that
/// let any place attach to any server would make it trivial to sync a project
/// into the wrong place, which is far more destructive than the inconvenience
/// of a manual click.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoConnectPolicy {
    /// Never auto-connect. A human must press Connect every time.
    Off,

    /// Auto-connect only when the place proves it belongs to this project,
    /// either because the project lists the place in `servePlaceIds`, because
    /// `gameId` matches, or because this place was previously paired with this
    /// project by a human.
    #[default]
    Matching,

    /// Auto-connect from any place. This exists for unpublished places, which
    /// report a `PlaceId` of 0 and therefore have no identity to match against.
    /// Opting in is a statement that the machine is trusted.
    Always,
}

impl AutoConnectPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            AutoConnectPolicy::Off => "off",
            AutoConnectPolicy::Matching => "matching",
            AutoConnectPolicy::Always => "always",
        }
    }
}

impl fmt::Display for AutoConnectPolicy {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Lets the policy be used directly as a command line value.
impl FromStr for AutoConnectPolicy {
    type Err = String;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        match source.to_ascii_lowercase().as_str() {
            "off" | "never" | "false" => Ok(AutoConnectPolicy::Off),
            "matching" | "match" | "true" => Ok(AutoConnectPolicy::Matching),
            "always" => Ok(AutoConnectPolicy::Always),
            other => Err(format!(
                "Invalid auto-connect policy '{other}'. Valid values are: off, matching, always"
            )),
        }
    }
}

/// Accepts either the policy names or a bare boolean, since `"autoConnect":
/// true` is the spelling most people reach for first.
impl<'de> Deserialize<'de> for AutoConnectPolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Bool(bool),
            Name(String),
        }

        match Repr::deserialize(deserializer)? {
            Repr::Bool(true) => Ok(AutoConnectPolicy::Matching),
            Repr::Bool(false) => Ok(AutoConnectPolicy::Off),
            Repr::Name(name) => match name.to_ascii_lowercase().as_str() {
                "off" | "never" => Ok(AutoConnectPolicy::Off),
                "matching" | "match" => Ok(AutoConnectPolicy::Matching),
                "always" => Ok(AutoConnectPolicy::Always),
                other => Err(serde::de::Error::custom(format!(
                    "invalid autoConnect value '{other}'. \
                     Valid values are 'off', 'matching', 'always', true, or false"
                ))),
            },
        }
    }
}

impl Serialize for AutoConnectPolicy {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// Derives a stable identifier for a project that has no explicit `projectId`.
///
/// The identifier has to survive server restarts so that a place paired today
/// still recognizes the same project tomorrow, but it must not collide between
/// two different checkouts on the same machine. Hashing the canonical path of
/// the project file gives us both. It is intentionally *not* stable across
/// machines: pairing is a local trust decision, and a shared identifier would
/// let a project file in a repository claim someone else's pairing.
pub fn derive_project_id(project_file_location: &Path) -> String {
    let canonical = dunce::canonicalize(project_file_location)
        .unwrap_or_else(|_| project_file_location.to_path_buf());

    let hash = blake3::hash(canonical.to_string_lossy().as_bytes());

    // 16 hex characters is far more than enough to keep local checkouts apart
    // and keeps the value readable in logs and settings.
    format!("roxo-{}", &hash.to_hex()[..16])
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn policy_accepts_booleans_and_names() {
        let parse = |json: &str| serde_json::from_str::<AutoConnectPolicy>(json).unwrap();

        assert_eq!(parse("true"), AutoConnectPolicy::Matching);
        assert_eq!(parse("false"), AutoConnectPolicy::Off);
        assert_eq!(parse("\"off\""), AutoConnectPolicy::Off);
        assert_eq!(parse("\"MATCHING\""), AutoConnectPolicy::Matching);
        assert_eq!(parse("\"always\""), AutoConnectPolicy::Always);
    }

    #[test]
    fn policy_rejects_unknown_names() {
        assert!(serde_json::from_str::<AutoConnectPolicy>("\"sometimes\"").is_err());
    }

    #[test]
    fn policy_parses_from_command_line_values() {
        assert_eq!("off".parse(), Ok(AutoConnectPolicy::Off));
        assert_eq!("Matching".parse(), Ok(AutoConnectPolicy::Matching));
        assert_eq!("always".parse(), Ok(AutoConnectPolicy::Always));
        assert!("sometimes".parse::<AutoConnectPolicy>().is_err());
    }

    #[test]
    fn derived_project_ids_differ_between_paths() {
        let a = derive_project_id(Path::new("/tmp/a/default.project.json"));
        let b = derive_project_id(Path::new("/tmp/b/default.project.json"));

        assert_ne!(a, b);
        assert!(a.starts_with("roxo-"));
    }

    #[test]
    fn derived_project_ids_are_stable() {
        let path = Path::new("/tmp/a/default.project.json");

        assert_eq!(derive_project_id(path), derive_project_id(path));
    }
}
