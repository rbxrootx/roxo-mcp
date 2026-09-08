use std::{
    io::{self, Write},
    path::PathBuf,
    time::Duration,
};

use anyhow::Context;
use clap::Parser;
use termcolor::{BufferWriter, Color, ColorSpec, WriteColor};

use crate::{session_registry, web_api::SessionStatusResponse};

use super::GlobalOptions;

/// How long to wait on a server that is running but not answering. Kept short
/// because the caller is usually a scripted poll, and a hung status check is
/// indistinguishable from a hung sync.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Report what a serve session is doing and whether Studio is attached to it.
#[derive(Debug, Parser)]
pub struct StatusCommand {
    /// The port of the session to inspect. Only needed when more than one
    /// session is running.
    #[clap(long)]
    pub port: Option<u16>,

    /// Inspect the session serving this project path.
    #[clap(long)]
    pub project: Option<PathBuf>,

    /// Print the full status as JSON.
    #[clap(long)]
    pub json: bool,

    /// Exit with a non-zero status when no Studio client is connected, so that
    /// a script can branch on it without parsing output.
    #[clap(long)]
    pub require_studio: bool,
}

impl StatusCommand {
    pub fn run(self, global: GlobalOptions) -> anyhow::Result<()> {
        let status = fetch_status(self.port, self.project.as_deref())?;

        if self.json {
            println!("{}", serde_json::to_string_pretty(&status)?);
        } else {
            show_status(&status, global.color.into())?;
        }

        if self.require_studio && !status.studio_connected {
            anyhow::bail!(
                "No Studio client is connected to '{}'. \
                 Open the place in Studio, or run `roxo wait --for studio` to block until it \
                 connects.",
                status.project_name
            );
        }

        Ok(())
    }
}

/// Fetches status for one session, resolving which session that is first.
pub fn fetch_status(
    port: Option<u16>,
    project: Option<&std::path::Path>,
) -> anyhow::Result<SessionStatusResponse> {
    let session = session_registry::resolve(port, project)?;

    let client = reqwest::blocking::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()?;

    let url = format!("{}/api/roxo/status", session.base_url());

    let response = client.get(&url).send().with_context(|| {
        format!(
            "Could not reach the Roxo server for '{}' at {}. \
             It may have stopped since it was last seen.",
            session.project_name,
            session.base_url()
        )
    })?;

    if !response.status().is_success() {
        anyhow::bail!(
            "The server at {} responded with {}. \
             It may be an upstream Rojo server, which does not implement /api/roxo/status.",
            session.base_url(),
            response.status()
        );
    }

    Ok(response.json()?)
}

fn show_status(status: &SessionStatusResponse, color: termcolor::ColorChoice) -> io::Result<()> {
    let writer = BufferWriter::stdout(color);
    let mut buffer = writer.buffer();

    let mut bold = ColorSpec::new();
    bold.set_bold(true);

    let mut green = ColorSpec::new();
    green.set_fg(Some(Color::Green)).set_bold(true);

    let mut yellow = ColorSpec::new();
    yellow.set_fg(Some(Color::Yellow)).set_bold(true);

    buffer.set_color(&bold)?;
    writeln!(&mut buffer, "{}", status.project_name)?;
    buffer.set_color(&ColorSpec::new())?;

    writeln!(&mut buffer, "  Project:  {}", status.project_path)?;
    writeln!(&mut buffer, "  Identity: {}", status.project_id)?;
    writeln!(&mut buffer, "  Session:  {}", status.session_id)?;
    writeln!(
        &mut buffer,
        "  Uptime:   {}",
        format_duration(status.uptime_seconds)
    )?;
    writeln!(&mut buffer, "  Connect:  {}", status.auto_connect)?;

    write!(&mut buffer, "  Studio:   ")?;
    if status.studio_connected {
        buffer.set_color(&green)?;
        writeln!(
            &mut buffer,
            "connected ({} client{})",
            status.client_count,
            if status.client_count == 1 { "" } else { "s" }
        )?;
    } else {
        buffer.set_color(&yellow)?;
        writeln!(&mut buffer, "not connected")?;
    }
    buffer.set_color(&ColorSpec::new())?;

    writeln!(&mut buffer, "  Patches:  {}", status.patches_sent)?;

    if !status.clients.is_empty() {
        writeln!(&mut buffer)?;
        buffer.set_color(&bold)?;
        writeln!(&mut buffer, "Clients")?;
        buffer.set_color(&ColorSpec::new())?;

        for client in &status.clients {
            let place = match client.place_id {
                // An unpublished place reports 0, which is worth spelling out
                // because it is also the reason strict matching cannot apply.
                Some(0) => "unpublished place".to_owned(),
                Some(id) => format!("place {id}"),
                None => "unknown place".to_owned(),
            };

            let how = if client.auto_connected {
                match &client.match_reason {
                    Some(reason) => format!("auto-connected via {reason}"),
                    None => "auto-connected".to_owned(),
                }
            } else {
                "connected manually".to_owned()
            };

            writeln!(
                &mut buffer,
                "  {} {} - {}, {}",
                if client.subscribed { "*" } else { "-" },
                client.client_name,
                place,
                how
            )?;
        }
    }

    writer.print(&buffer)
}

fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;

    if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn durations_scale_with_magnitude() {
        assert_eq!(format_duration(9), "9s");
        assert_eq!(format_duration(75), "1m 15s");
        assert_eq!(format_duration(3700), "1h 1m");
    }
}
