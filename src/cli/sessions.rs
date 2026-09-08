use std::io::{self, Write};

use clap::Parser;
use termcolor::{BufferWriter, Color, ColorSpec, WriteColor};

use crate::session_registry::{self, SessionBeacon};

use super::GlobalOptions;

/// List the Roxo serve sessions running on this machine.
///
/// This is how an agent finds a server to talk to without scanning ports or
/// being told a port up front.
#[derive(Debug, Parser)]
pub struct SessionsCommand {
    /// Print the session list as JSON.
    #[clap(long)]
    pub json: bool,
}

impl SessionsCommand {
    pub fn run(self, global: GlobalOptions) -> anyhow::Result<()> {
        let sessions = session_registry::list()?;

        if self.json {
            println!("{}", serde_json::to_string_pretty(&sessions)?);
            return Ok(());
        }

        show_sessions(&sessions, global.color.into())?;

        Ok(())
    }
}

fn show_sessions(sessions: &[SessionBeacon], color: termcolor::ColorChoice) -> io::Result<()> {
    let writer = BufferWriter::stdout(color);
    let mut buffer = writer.buffer();

    if sessions.is_empty() {
        writeln!(&mut buffer, "No Roxo serve sessions are running.")?;
        writeln!(&mut buffer)?;
        writeln!(&mut buffer, "Start one with `roxo serve`.")?;

        return writer.print(&buffer);
    }

    let mut bold = ColorSpec::new();
    bold.set_bold(true);

    let mut green = ColorSpec::new();
    green.set_fg(Some(Color::Green));

    buffer.set_color(&bold)?;
    writeln!(
        &mut buffer,
        "{:<7} {:<24} {:<12} PATH",
        "PORT", "PROJECT", "CONNECT"
    )?;
    buffer.set_color(&ColorSpec::new())?;

    for session in sessions {
        write!(&mut buffer, "{:<7} ", session.port)?;

        buffer.set_color(&green)?;
        write!(&mut buffer, "{:<24} ", truncate(&session.project_name, 24))?;
        buffer.set_color(&ColorSpec::new())?;

        writeln!(
            &mut buffer,
            "{:<12} {}",
            session.auto_connect,
            session.project_path.display()
        )?;
    }

    writer.print(&buffer)
}

/// Keeps the table aligned when a project name is longer than its column.
fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_owned();
    }

    let kept: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn short_names_are_left_alone() {
        assert_eq!(truncate("MyGame", 24), "MyGame");
    }

    #[test]
    fn long_names_are_shortened_to_the_column_width() {
        let truncated = truncate("AVeryLongProjectNameIndeed", 10);

        assert_eq!(truncated.chars().count(), 10);
        assert!(truncated.ends_with('…'));
    }
}
