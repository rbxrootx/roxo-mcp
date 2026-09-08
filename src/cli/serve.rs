use std::{
    io::{self, Write},
    net::{IpAddr, Ipv4Addr},
    path::PathBuf,
    sync::Arc,
};

use clap::Parser;
use memofs::Vfs;
use termcolor::{BufferWriter, Color, ColorChoice, ColorSpec, WriteColor};

use crate::{
    auto_connect::AutoConnectPolicy,
    serve_session::ServeSession,
    session_registry::{self, SessionBeacon},
    web::LiveServer,
};

use super::{resolve_path, GlobalOptions};

const DEFAULT_BIND_ADDRESS: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);
const DEFAULT_PORT: u16 = 34872;

/// Expose a Rojo project to the Rojo Studio plugin.
#[derive(Debug, Parser)]
pub struct ServeCommand {
    /// Path to the project to serve. Defaults to the current directory.
    #[clap(default_value = "")]
    pub project: PathBuf,

    /// The IP address to listen on. Defaults to `127.0.0.1`.
    #[clap(long)]
    pub address: Option<IpAddr>,

    /// The port to listen on. Defaults to the project's preference, or `34872` if
    /// it has none.
    #[clap(long)]
    pub port: Option<u16>,

    /// Extra `Host`/`Origin` values the server will accept, beyond localhost and
    /// the bind address (for example a hostname like `mypc.lan`). Repeat the
    /// option or comma-separate to allow several. When given, this overrides the
    /// project's `serveAllowedHosts`. Listing any host also turns on Host/Origin
    /// validation for binds where it is otherwise off (such as `0.0.0.0`).
    #[clap(long, value_delimiter = ',')]
    pub allowed_hosts: Vec<String>,

    /// Override the project's auto-connect policy for this run. Valid values
    /// are `off`, `matching`, and `always`.
    ///
    /// `matching` lets the Studio plugin connect on its own, but only from a
    /// place that this project proves it belongs to. `always` drops that proof
    /// requirement and should only be used for unpublished places, which have
    /// no place ID to match against.
    #[clap(long, value_name = "POLICY")]
    pub auto_connect: Option<AutoConnectPolicy>,

    /// Do not advertise this session in the machine-local session registry.
    ///
    /// The registry is how `roxo sessions`, `roxo status`, and the MCP server
    /// find running servers without scanning ports.
    #[clap(long)]
    pub no_registry: bool,
}

impl ServeCommand {
    pub fn run(self, global: GlobalOptions) -> anyhow::Result<()> {
        let project_path = resolve_path(&self.project)?;

        let vfs = Vfs::new_default()?;

        let mut session = ServeSession::new(vfs, project_path)?;
        session.set_auto_connect_override(self.auto_connect);
        let session = Arc::new(session);

        let ip = self
            .address
            .or_else(|| session.serve_address())
            .unwrap_or(DEFAULT_BIND_ADDRESS.into());

        let port = self
            .port
            .or_else(|| session.project_port())
            .unwrap_or(DEFAULT_PORT);

        // The CLI flag, when given, replaces the project's list rather than
        // merging with it, matching how --address and --port override theirs.
        let allowed_hosts = if self.allowed_hosts.is_empty() {
            session.serve_allowed_hosts().to_vec()
        } else {
            self.allowed_hosts
        };

        // Held until the server stops so that the beacon is removed on exit
        // and agents are never pointed at a port nothing is listening on.
        let _beacon = if self.no_registry {
            None
        } else {
            match publish_beacon(&session, ip, port) {
                Ok(guard) => Some(guard),
                Err(err) => {
                    // Discovery is a convenience. A read-only home directory
                    // should degrade to "you must pass --port" rather than
                    // refuse to serve at all.
                    log::warn!("Could not advertise this session for discovery: {err:#}");
                    None
                }
            }
        };

        let auto_connect = session.auto_connect();
        let server = LiveServer::new(session);

        server.start((ip, port).into(), allowed_hosts, || {
            let _ = show_start_message(ip, port, auto_connect, global.color.into());
        })?;

        Ok(())
    }
}

fn publish_beacon(
    session: &ServeSession,
    address: IpAddr,
    port: u16,
) -> anyhow::Result<session_registry::BeaconGuard> {
    let beacon = SessionBeacon {
        session_id: session.session_id(),
        pid: std::process::id(),
        address: address.to_string(),
        port,
        project_name: session.project_name().to_owned(),
        project_id: session.project_id().to_owned(),
        project_path: session.project_path().to_path_buf(),
        place_id: session.place_id(),
        game_id: session.game_id(),
        serve_place_ids: session
            .serve_place_ids()
            .map(|ids| ids.iter().copied().collect()),
        auto_connect: session.auto_connect().as_str().to_owned(),
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        started_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0),
    };

    session_registry::publish(&beacon)
}

fn show_start_message(
    bind_address: IpAddr,
    port: u16,
    auto_connect: AutoConnectPolicy,
    color: ColorChoice,
) -> io::Result<()> {
    let mut green = ColorSpec::new();
    green.set_fg(Some(Color::Green)).set_bold(true);

    let writer = BufferWriter::stdout(color);
    let mut buffer = writer.buffer();

    let address_string = if bind_address.is_loopback() {
        "localhost".to_owned()
    } else {
        bind_address.to_string()
    };

    writeln!(&mut buffer, "Roxo server listening:")?;

    write!(&mut buffer, "  Address: ")?;
    buffer.set_color(&green)?;
    writeln!(&mut buffer, "{}", address_string)?;

    buffer.set_color(&ColorSpec::new())?;
    write!(&mut buffer, "  Port:    ")?;
    buffer.set_color(&green)?;
    writeln!(&mut buffer, "{}", port)?;

    buffer.set_color(&ColorSpec::new())?;
    write!(&mut buffer, "  Connect: ")?;
    buffer.set_color(&green)?;
    writeln!(&mut buffer, "{}", auto_connect_description(auto_connect))?;
    buffer.set_color(&ColorSpec::new())?;

    writeln!(&mut buffer)?;

    if !bind_address.is_loopback() {
        let mut warning = ColorSpec::new();
        warning.set_fg(Some(Color::Yellow)).set_bold(true);

        buffer.set_color(&warning)?;
        writeln!(
            &mut buffer,
            "WARNING: This server is bound to {address_string}, which is reachable from the \
             network.\n\
             The serve API is unauthenticated, so anyone who can reach {address_string}:{port} \
             can read\n\
             and modify your project's source. Prefer binding to localhost and tunneling (e.g. \
             SSH,\n\
             Tailscale, or WireGuard) when you need remote access."
        )?;
        buffer.set_color(&ColorSpec::new())?;
        writeln!(&mut buffer)?;
    }

    buffer.set_color(&ColorSpec::new())?;
    write!(&mut buffer, "Visit ")?;

    buffer.set_color(&green)?;
    write!(&mut buffer, "http://{}:{}/", address_string, port)?;

    buffer.set_color(&ColorSpec::new())?;
    writeln!(&mut buffer, " in your browser for more information.")?;

    writer.print(&buffer)?;

    Ok(())
}

fn auto_connect_description(policy: AutoConnectPolicy) -> &'static str {
    match policy {
        AutoConnectPolicy::Off => "manual (press Connect in Studio)",
        AutoConnectPolicy::Matching => "automatic for places this project matches",
        AutoConnectPolicy::Always => "automatic from any place",
    }
}
