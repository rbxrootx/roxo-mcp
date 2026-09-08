use std::{
    path::PathBuf,
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use clap::Parser;

use super::{status::fetch_status, GlobalOptions};

/// How often to re-check. Fast enough that an agent does not sit idle after
/// Studio attaches, slow enough not to spin a core while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// What a caller is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitCondition {
    /// A serve session exists at all.
    Server,

    /// A Studio client holds a live subscription to it.
    Studio,
}

impl FromStr for WaitCondition {
    type Err = String;

    fn from_str(source: &str) -> Result<Self, Self::Err> {
        match source.to_ascii_lowercase().as_str() {
            "server" => Ok(WaitCondition::Server),
            "studio" | "client" => Ok(WaitCondition::Studio),
            other => Err(format!(
                "Invalid wait condition '{other}'. Valid values are: server, studio"
            )),
        }
    }
}

/// Block until a serve session is ready, then exit.
///
/// This exists because starting a server and syncing to Studio are separate
/// events, and an agent that treats them as one will write files into a
/// session nothing is listening to and report success.
#[derive(Debug, Parser)]
pub struct WaitCommand {
    /// What to wait for: `server` (a session is running) or `studio` (a Studio
    /// client is attached and receiving changes).
    #[clap(long = "for", value_name = "CONDITION", default_value = "studio")]
    pub condition: WaitCondition,

    /// The port of the session to wait on.
    #[clap(long)]
    pub port: Option<u16>,

    /// Wait on the session serving this project path.
    #[clap(long)]
    pub project: Option<PathBuf>,

    /// Give up after this many seconds.
    #[clap(long, default_value = "60")]
    pub timeout: u64,

    /// Print the status as JSON once the condition is met.
    #[clap(long)]
    pub json: bool,
}

impl WaitCommand {
    pub fn run(self, _global: GlobalOptions) -> anyhow::Result<()> {
        let deadline = Instant::now() + Duration::from_secs(self.timeout);
        let mut last_error = None;

        loop {
            match fetch_status(self.port, self.project.as_deref()) {
                Ok(status) => {
                    let satisfied = match self.condition {
                        WaitCondition::Server => true,
                        WaitCondition::Studio => status.studio_connected,
                    };

                    if satisfied {
                        if self.json {
                            println!("{}", serde_json::to_string_pretty(&status)?);
                        } else {
                            match self.condition {
                                WaitCondition::Server => println!(
                                    "Server ready: '{}' on {}",
                                    status.project_name, status.project_path
                                ),
                                WaitCondition::Studio => println!(
                                    "Studio connected to '{}' ({} client{})",
                                    status.project_name,
                                    status.client_count,
                                    if status.client_count == 1 { "" } else { "s" }
                                ),
                            }
                        }

                        return Ok(());
                    }
                }
                // A missing session is an expected intermediate state when
                // waiting, not a failure: the caller may have started the
                // server a moment ago. Remember the reason so the timeout can
                // explain what was actually wrong.
                Err(err) => last_error = Some(err),
            }

            if Instant::now() >= deadline {
                break;
            }

            thread::sleep(POLL_INTERVAL);
        }

        match self.condition {
            WaitCondition::Server => anyhow::bail!(
                "Timed out after {}s waiting for a Roxo serve session.{}",
                self.timeout,
                last_error
                    .map(|err| format!("\nLast attempt: {err:#}"))
                    .unwrap_or_default()
            ),
            WaitCondition::Studio => anyhow::bail!(
                "Timed out after {}s waiting for Studio to connect.\n\
                 Check that the place is open in Studio, that the Roxo plugin is installed, \
                 and that auto-connect is not set to 'off' for this project.{}",
                self.timeout,
                last_error
                    .map(|err| format!("\nLast attempt: {err:#}"))
                    .unwrap_or_default()
            ),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn conditions_parse_from_command_line_values() {
        assert_eq!("studio".parse(), Ok(WaitCondition::Studio));
        assert_eq!("Server".parse(), Ok(WaitCondition::Server));
        assert!("forever".parse::<WaitCondition>().is_err());
    }
}
