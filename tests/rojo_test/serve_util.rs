use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use hyper_tungstenite::tungstenite::{connect, Message};
use rbx_dom_weak::types::Ref;

use serde::{Deserialize, Serialize};
use tempfile::{tempdir, TempDir};

use libroxo::{
    web_api::{
        ReadResponse, SerializeRequest, SerializeResponse, ServerInfoResponse,
        SessionStatusResponse, SocketPacket, SocketPacketType,
    },
    SessionId,
};
use rojo_insta_ext::RedactionMap;

use crate::rojo_test::io_util::{
    copy_recursive, get_working_dir_path, KillOnDrop, ROJO_PATH, SERVE_TESTS_PATH,
};

/// Convenience method to run a `rojo serve` test.
///
/// Test projects should be defined in the `serve-tests` folder; their filename
/// should be given as the first parameter.
///
/// The passed in callback is where the actual test body should go. Setup and
/// cleanup happens automatically.
pub fn run_serve_test(test_name: &str, callback: impl FnOnce(TestServeSession, RedactionMap)) {
    let _ = env_logger::try_init();

    let mut redactions = RedactionMap::default();

    let mut session = TestServeSession::new(test_name);
    let info = session.wait_to_come_online();

    redactions.intern(info.session_id);
    redactions.intern(info.root_instance_id);

    let mut settings = insta::Settings::new();

    let snapshot_path = Path::new(SERVE_TESTS_PATH)
        .parent()
        .unwrap()
        .join("serve-test-snapshots");

    settings.set_snapshot_path(snapshot_path);
    settings.set_sort_maps(true);
    settings.add_redaction(".serverVersion", "[server-version]");

    // These two vary with where the test's temporary directory happens to be,
    // so they would otherwise make every serve snapshot machine-specific.
    settings.add_redaction(".projectPath", "[project-path]");
    settings.add_redaction(".projectId", "[project-id]");
    settings.bind(move || callback(session, redactions));
}

/// Represents a running Rojo serve session running in a temporary directory.
pub struct TestServeSession {
    // Drop order is important here: we want the process to be killed before the
    // directory it's operating on is destroyed.
    rojo_process: KillOnDrop,
    _dir: TempDir,

    port: usize,
    project_path: PathBuf,
    log_path: PathBuf,
}

impl TestServeSession {
    pub fn new(name: &str) -> Self {
        let working_dir = get_working_dir_path();

        let source_path = Path::new(SERVE_TESTS_PATH).join(name);
        let dir = tempdir().expect("Couldn't create temporary directory");
        let project_path = dir
            .path()
            .canonicalize()
            .expect("Couldn't canonicalize temporary directory path")
            .join(name);

        let source_is_file = fs::metadata(&source_path).unwrap().is_file();

        if source_is_file {
            fs::copy(&source_path, &project_path).expect("couldn't copy project file");
        } else {
            fs::create_dir(&project_path).expect("Couldn't create temporary project subdirectory");

            copy_recursive(&source_path, &project_path)
                .expect("Couldn't copy project to temporary directory");
        };

        // This is an ugly workaround for FSEvents sometimes reporting events
        // for the above copy operations, similar to this Stack Overflow question:
        // https://stackoverflow.com/questions/47679298/howto-avoid-receiving-old-events-in-fseventstream-callback-fsevents-framework-o
        // We'll hope that 100ms is enough for FSEvents to get whatever it is
        // out of its system.
        // TODO: find a better way to avoid processing these spurious events.
        #[cfg(target_os = "macos")]
        std::thread::sleep(Duration::from_millis(100));

        let port = get_port_number();
        let port_string = port.to_string();

        // Captured to a file rather than inherited, so that a server which
        // fails to come online can say why. Inheriting leaves the harness
        // reporting a bare timeout with the actual error swallowed by cargo's
        // output capture.
        let log_path = dir.path().join("serve.log");
        let log = fs::File::create(&log_path).expect("Couldn't create server log file");
        let log_err = log.try_clone().expect("Couldn't duplicate server log file");

        let rojo_process = Command::new(ROJO_PATH)
            .args([
                "serve",
                project_path.to_str().unwrap(),
                "--port",
                port_string.as_str(),
            ])
            .current_dir(working_dir)
            // Keep the session registry inside the test's own directory. Left
            // to itself the server advertises into the developer's real
            // ~/.roxo, so a test run litters their machine with beacons for
            // servers that no longer exist.
            .env("ROXO_HOME", dir.path())
            .stdout(log)
            .stderr(log_err)
            .spawn()
            .expect("Couldn't start Rojo");

        TestServeSession {
            rojo_process: KillOnDrop(rojo_process),
            _dir: dir,
            port,
            project_path,
            log_path,
        }
    }

    pub fn path(&self) -> &Path {
        &self.project_path
    }

    pub fn port(&self) -> usize {
        self.port
    }

    /// Waits for the `rojo serve` server to come online.
    ///
    /// Bounded by wall time rather than a try count. A fixed number of retries
    /// with short backoff gives the process well under a second to start, which
    /// is enough on a warm machine and not enough on a cold one or a loaded CI
    /// runner, producing failures that look like server bugs but are not.
    pub fn wait_to_come_online(&mut self) -> ServerInfoResponse {
        const POLL_INTERVAL: Duration = Duration::from_millis(50);
        const TIMEOUT: Duration = Duration::from_secs(30);

        let deadline = Instant::now() + TIMEOUT;
        let mut last_error = None;

        while Instant::now() < deadline {
            match self.rojo_process.0.try_wait() {
                Ok(Some(status)) => {
                    let log = fs::read_to_string(&self.log_path).unwrap_or_default();

                    panic!("Rojo process exited with status {status}\nServer output:\n{log}");
                }
                Ok(None) => { /* The process is still running, as expected */ }
                Err(err) => panic!("Failed to wait on Rojo process: {}", err),
            }

            match self.get_api_rojo() {
                Ok(info) => {
                    log::info!("Got session info: {:?}", info);

                    return info;
                }
                Err(err) => {
                    log::debug!("Server not up yet, retrying: {}", err);
                    last_error = Some(err);
                    thread::sleep(POLL_INTERVAL);
                }
            }
        }

        let log = fs::read_to_string(&self.log_path).unwrap_or_default();

        panic!(
            "Rojo server did not respond within {:?}.\nLast error: {}\nServer output:\n{}",
            TIMEOUT,
            last_error
                .map(|err| err.to_string())
                .unwrap_or_else(|| "<none>".to_owned()),
            if log.is_empty() { "<nothing>" } else { &log }
        );
    }

    pub fn get_api_rojo(&self) -> Result<ServerInfoResponse, reqwest::Error> {
        let url = format!("http://localhost:{}/api/rojo", self.port);
        let body = reqwest::blocking::get(url)?.bytes()?;

        Ok(deserialize_msgpack(&body).expect("Server returned malformed response"))
    }

    /// Performs a handshake carrying the query parameters a Roxo client uses to
    /// identify itself.
    pub fn get_api_rojo_as_client(
        &self,
        query: &str,
    ) -> Result<ServerInfoResponse, reqwest::Error> {
        let url = format!("http://localhost:{}/api/rojo?{}", self.port, query);
        let body = reqwest::blocking::get(url)?.bytes()?;

        Ok(deserialize_msgpack(&body).expect("Server returned malformed response"))
    }

    pub fn get_api_status(&self) -> Result<SessionStatusResponse, reqwest::Error> {
        let url = format!("http://localhost:{}/api/roxo/status", self.port);

        reqwest::blocking::get(url)?.json()
    }

    /// Opens a change subscription and returns it, so the caller controls when
    /// the connection closes.
    pub fn open_subscription(
        &self,
        client_id: Option<u64>,
    ) -> Result<
        hyper_tungstenite::tungstenite::WebSocket<
            hyper_tungstenite::tungstenite::stream::MaybeTlsStream<std::net::TcpStream>,
        >,
        Box<dyn std::error::Error>,
    > {
        let url = match client_id {
            Some(id) => format!("ws://localhost:{}/api/socket/0?clientId={}", self.port, id),
            None => format!("ws://localhost:{}/api/socket/0", self.port),
        };

        let (socket, _response) = connect(url)?;

        Ok(socket)
    }

    pub fn get_api_read(&self, id: Ref) -> Result<ReadResponse<'_>, reqwest::Error> {
        let url = format!("http://localhost:{}/api/read/{}", self.port, id);
        let body = reqwest::blocking::get(url)?.bytes()?;

        Ok(deserialize_msgpack(&body).expect("Server returned malformed response"))
    }

    pub fn get_api_socket_packet(
        &self,
        packet_type: SocketPacketType,
        cursor: u32,
    ) -> Result<SocketPacket<'static>, Box<dyn std::error::Error>> {
        let url = format!("ws://localhost:{}/api/socket/{}", self.port, cursor);

        let (mut socket, _response) = connect(url)?;

        // Wait for messages with a timeout
        let timeout = Duration::from_secs(10);
        let start = std::time::Instant::now();

        loop {
            if start.elapsed() > timeout {
                return Err("Timeout waiting for packet from WebSocket".into());
            }

            match socket.read() {
                Ok(Message::Binary(binary)) => {
                    let packet: SocketPacket = deserialize_msgpack(&binary)?;
                    if packet.packet_type != packet_type {
                        continue;
                    }

                    // Close the WebSocket connection now that we got what we were waiting for
                    let _ = socket.close(None);
                    return Ok(packet);
                }
                Ok(Message::Close(_)) => {
                    return Err("WebSocket closed before receiving messages".into());
                }
                Ok(_) => {
                    // Ignore other message types (ping, pong, text)
                    continue;
                }
                Err(hyper_tungstenite::tungstenite::Error::Io(e))
                    if e.kind() == std::io::ErrorKind::WouldBlock =>
                {
                    // No data available yet, sleep a bit and try again
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(e) => {
                    return Err(e.into());
                }
            }
        }
    }

    pub fn post_api_serialize(
        &self,
        ids: &[Ref],
        session_id: SessionId,
    ) -> Result<reqwest::blocking::Response, reqwest::Error> {
        let client = reqwest::blocking::Client::new();
        let url = format!("http://localhost:{}/api/serialize", self.port);
        let body = serialize_msgpack(&SerializeRequest {
            session_id,
            ids: ids.to_vec(),
        })
        .unwrap();

        client.post(url).body(body).send()
    }

    /// Sends a GET to `/api/rojo` with the given extra request headers and
    /// returns the full response. Used to exercise the Host/Origin allowlist that
    /// guards against DNS rebinding, including asserting that a rejection reveals
    /// nothing about the server.
    pub fn api_rojo_response_with_headers(
        &self,
        headers: &[(&str, &str)],
    ) -> reqwest::blocking::Response {
        let client = reqwest::blocking::Client::new();
        let url = format!("http://localhost:{}/api/rojo", self.port);

        let mut request = client.get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }

        request.send().expect("Failed to send request")
    }

    /// Sends a POST to `/api/open/<id>` and returns the response status code.
    /// Used to verify that the local-only gate on `/api/open` admits loopback
    /// peers (the test harness always connects over loopback).
    pub fn api_open_status(&self, id: &str) -> reqwest::StatusCode {
        let client = reqwest::blocking::Client::new();
        let url = format!("http://localhost:{}/api/open/{}", self.port, id);

        client
            .post(url)
            .send()
            .expect("Failed to send request")
            .status()
    }
}

fn serialize_msgpack<T: Serialize>(value: T) -> Result<Vec<u8>, rmp_serde::encode::Error> {
    let mut serialized = Vec::new();
    let mut serializer = rmp_serde::Serializer::new(&mut serialized)
        .with_human_readable()
        .with_struct_map();

    value.serialize(&mut serializer)?;

    Ok(serialized)
}

pub fn deserialize_msgpack<'a, T: Deserialize<'a>>(
    input: &'a [u8],
) -> Result<T, rmp_serde::decode::Error> {
    let mut deserializer = rmp_serde::Deserializer::new(input).with_human_readable();

    T::deserialize(&mut deserializer)
}

/// Probably-okay way to generate random enough port numbers for running the
/// Rojo live server.
///
/// If this method ends up having problems, we should add an option for Rojo to
/// use a random port chosen by the operating system and figure out a good way
/// to get that port back to the test CLI.
fn get_port_number() -> usize {
    static NEXT_PORT_NUMBER: AtomicUsize = AtomicUsize::new(35103);

    NEXT_PORT_NUMBER.fetch_add(1, Ordering::SeqCst)
}

/// Takes a SerializeResponse and creates an XML model out of the response.
///
/// Since the provided structure intentionally includes unredacted referents,
/// some post-processing is done to ensure they don't show up in the model.
pub fn serialize_to_xml_model(response: &SerializeResponse, redactions: &RedactionMap) -> String {
    let mut dom = rbx_binary::from_reader(response.model_contents.as_slice()).unwrap();
    // This makes me realize that maybe we need a `descendants_mut` iter.
    let ref_list: Vec<Ref> = dom.descendants().map(|inst| inst.referent()).collect();
    for referent in ref_list {
        let inst = dom.get_by_ref_mut(referent).unwrap();
        if let Some(id) = redactions.get_id_for_value(&inst.name) {
            inst.name = format!("id-{id}");
        }
    }

    let mut data = Vec::new();
    rbx_xml::to_writer_default(&mut data, &dom, dom.root().children()).unwrap();
    String::from_utf8(data).expect("rbx_xml should never produce invalid utf-8")
}
