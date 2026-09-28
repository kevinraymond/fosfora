use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::Sender;

use super::client;
use super::types::WsInMessage;

/// Embedded HTML control surface.
/// In debug mode, try to read from filesystem for hot-reload; fall back to embedded.
fn get_html_content() -> String {
    #[cfg(debug_assertions)]
    {
        let path = crate::effect::loader::assets_dir().join("web/control.html");
        if let Ok(content) = std::fs::read_to_string(&path) {
            return content;
        }
    }
    include_str!("../../../../assets/web/control.html").to_string()
}

/// Spawn the accept loop thread. Returns (shutdown_flag, thread_handle).
pub fn spawn_accept_loop(
    port: u16,
    lan: bool,
    inbound_tx: Sender<WsInMessage>,
    clients: Arc<Mutex<Vec<Sender<String>>>>,
    latest_state: Arc<Mutex<String>>,
    shutdown: Arc<AtomicBool>,
) -> anyhow::Result<JoinHandle<()>> {
    let listener = bind_listener(port, lan)?;
    log::info!(
        "Web control server listening on http://{}",
        listener.local_addr()?
    );

    let client_counter = Arc::new(AtomicUsize::new(0));

    let handle = thread::Builder::new()
        .name("fosfora-web-accept".into())
        .spawn(move || {
            // Set the listener to have a timeout for accept
            let _ = listener.set_nonblocking(true);

            while !shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, addr)) => {
                        log::debug!("Web connection from {addr}");
                        let _ = stream.set_nonblocking(false);
                        handle_connection(
                            stream,
                            lan,
                            &inbound_tx,
                            &clients,
                            &latest_state,
                            &shutdown,
                            &client_counter,
                        );
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        // No pending connection — sleep briefly before retrying
                        thread::sleep(Duration::from_millis(50));
                    }
                    Err(e) => {
                        if !shutdown.load(Ordering::Relaxed) {
                            log::error!("Web accept error: {e}");
                        }
                        break;
                    }
                }
            }
            log::info!("Web accept thread shutting down");
        })?;

    Ok(handle)
}

/// Loopback only unless LAN access is on (#43).
fn bind_listener(port: u16, lan: bool) -> std::io::Result<TcpListener> {
    TcpListener::bind((if lan { "0.0.0.0" } else { "127.0.0.1" }, port))
}

fn handle_connection(
    mut stream: TcpStream,
    lan: bool,
    inbound_tx: &Sender<WsInMessage>,
    clients: &Arc<Mutex<Vec<Sender<String>>>>,
    latest_state: &Arc<Mutex<String>>,
    shutdown: &Arc<AtomicBool>,
    client_counter: &Arc<AtomicUsize>,
) {
    // Peek at the first bytes to determine if this is a WebSocket upgrade or plain HTTP
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));

    let mut buf = [0u8; 4096];
    let n = match stream.read(&mut buf) {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    let request = String::from_utf8_lossy(&buf[..n]);

    if is_websocket_upgrade(&request) {
        if let Err(why) = upgrade_allowed(&request, lan) {
            log::warn!("Refused WebSocket connection: {why}");
            let _ = stream.write_all(
                b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            return;
        }
        // Set read timeout for interleaved read/write in client handler
        let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
        // Replay already-read bytes then continue from the stream
        let replay = ReplayStream::new(buf[..n].to_vec(), stream);
        match tungstenite::accept(replay) {
            Ok(ws) => {
                let client_id = client_counter.fetch_add(1, Ordering::Relaxed);
                let (outbound_tx, outbound_rx) = crossbeam_channel::bounded(256);

                // Get latest state for initial sync — recover from poisoned mutex
                // rather than panicking the accept thread during a live performance.
                let state = latest_state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();

                // Register client
                clients
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(outbound_tx);

                let tx = inbound_tx.clone();
                let flag = shutdown.clone();

                thread::Builder::new()
                    .name(format!("fosfora-web-client-{client_id}"))
                    .spawn(move || {
                        client::run_client(ws, tx, outbound_rx, state, flag, client_id);
                    })
                    .ok();
            }
            Err(e) => {
                log::debug!("WebSocket handshake failed: {e}");
            }
        }
    } else {
        // Plain HTTP — serve the control surface HTML
        serve_http(&mut stream, &request);
    }
}

/// Value of the first `name:` header in a raw request, trimmed; case-insensitive.
fn header<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    request.lines().skip(1).find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// Hostname of a `Host` header value (`localhost:9002`, `[::1]:9002`), port dropped.
fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    host.rsplit_once(':').map_or(host, |(h, _)| h)
}

/// Whether a WebSocket upgrade may proceed.
///
/// - A browser always sends `Origin`, so any web page the operator has open
///   could otherwise open `ws://127.0.0.1:9002` and drive the show. Only the
///   control page this server serves itself is let in: its `Origin` names the
///   same host:port as the request's `Host`. `Origin: null` (a file:// page,
///   a sandboxed frame) is refused. Bridges and other non-browser clients send
///   no `Origin` and are unaffected.
/// - With LAN access off the `Host` must be a loopback name, which stops a DNS
///   rebinding page from reaching the loopback-only server under its own name.
fn upgrade_allowed(request: &str, lan: bool) -> Result<(), String> {
    let host = header(request, "host");
    if !lan
        && let Some(h) = host
        && !matches!(
            host_name(h).to_ascii_lowercase().as_str(),
            "localhost" | "127.0.0.1" | "::1"
        )
    {
        return Err(format!("Host {h} is not this computer (LAN access is off)"));
    }
    match header(request, "origin") {
        None => Ok(()),
        Some(origin) => {
            let origin_host = origin
                .split_once("://")
                .map(|(_, rest)| rest.trim_end_matches('/'));
            match (origin_host, host) {
                (Some(o), Some(h)) if o.eq_ignore_ascii_case(h) => Ok(()),
                _ => Err(format!("page origin {origin} is not this server")),
            }
        }
    }
}

fn is_websocket_upgrade(request: &str) -> bool {
    // Check for WebSocket upgrade headers (case-insensitive)
    let lower = request.to_lowercase();
    lower.contains("upgrade: websocket") || lower.contains("upgrade:websocket")
}

fn serve_http(stream: &mut TcpStream, request: &str) {
    // Parse the request path
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let (status, content_type, body) = match path {
        "/" | "/index.html" | "/control" => {
            let html = get_html_content();
            ("200 OK", "text/html; charset=utf-8", html)
        }
        "/health" => (
            "200 OK",
            "application/json",
            r#"{"status":"ok"}"#.to_string(),
        ),
        _ => {
            // Redirect everything else to /
            let response =
                "HTTP/1.1 302 Found\r\nLocation: /\r\nContent-Length: 0\r\n\r\n".to_string();
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            return;
        }
    };

    let response = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Cache-Control: no-cache\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}

/// A wrapper that replays buffered data before reading from the underlying stream.
struct ReplayStream {
    buffer: Vec<u8>,
    pos: usize,
    stream: TcpStream,
}

impl ReplayStream {
    fn new(buffer: Vec<u8>, stream: TcpStream) -> Self {
        Self {
            buffer,
            pos: 0,
            stream,
        }
    }
}

impl Read for ReplayStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos < self.buffer.len() {
            let remaining = &self.buffer[self.pos..];
            let n = remaining.len().min(buf.len());
            buf[..n].copy_from_slice(&remaining[..n]);
            self.pos += n;
            Ok(n)
        } else {
            self.stream.read(buf)
        }
    }
}

impl Write for ReplayStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.stream.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upgrade(headers: &[&str]) -> String {
        let mut r = String::from("GET / HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n");
        for h in headers {
            r.push_str(h);
            r.push_str("\r\n");
        }
        r + "\r\n"
    }

    #[test]
    fn the_served_control_page_and_bridges_are_let_in() {
        // The control page, loaded from this server.
        let page = upgrade(&["Host: localhost:9002", "Origin: http://localhost:9002"]);
        assert!(upgrade_allowed(&page, false).is_ok());
        // A bridge: no Origin.
        assert!(upgrade_allowed(&upgrade(&["Host: 127.0.0.1:9002"]), false).is_ok());
        // A phone on the LAN loading the page by IP, with LAN access on.
        let phone = upgrade(&["Host: 192.168.1.5:9002", "Origin: http://192.168.1.5:9002"]);
        assert!(upgrade_allowed(&phone, true).is_ok());
    }

    #[test]
    fn a_foreign_web_page_is_refused() {
        let evil = upgrade(&["Host: 127.0.0.1:9002", "Origin: https://evil.example"]);
        assert!(upgrade_allowed(&evil, false).is_err());
        assert!(upgrade_allowed(&evil, true).is_err());
        let file = upgrade(&["Host: localhost:9002", "Origin: null"]);
        assert!(upgrade_allowed(&file, false).is_err());
        // Same host, different port: another local web app.
        let other = upgrade(&["Host: localhost:9002", "Origin: http://localhost:3000"]);
        assert!(upgrade_allowed(&other, false).is_err());
    }

    #[test]
    fn loopback_only_refuses_foreign_host_names() {
        // DNS rebinding: the attacker's name resolves to 127.0.0.1, same origin.
        let rebind = upgrade(&[
            "Host: evil.example:9002",
            "Origin: http://evil.example:9002",
        ]);
        assert!(upgrade_allowed(&rebind, false).is_err());
        assert!(upgrade_allowed(&upgrade(&["Host: [::1]:9002"]), false).is_ok());
        assert!(upgrade_allowed(&upgrade(&["host: LOCALHOST:9002"]), false).is_ok());
    }

    #[test]
    fn server_binds_loopback_unless_lan_is_on() {
        let local = bind_listener(0, false).unwrap().local_addr().unwrap();
        assert!(local.ip().is_loopback(), "{local}");
        let lan = bind_listener(0, true).unwrap().local_addr().unwrap();
        assert!(lan.ip().is_unspecified(), "{lan}");
    }

    /// Through the real accept loop: a bridge (no Origin) completes the
    /// handshake, a foreign page's Origin gets a 403.
    #[test]
    fn accept_loop_refuses_a_foreign_origin() {
        use tungstenite::client::IntoClientRequest;

        let port = bind_listener(0, false)
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let (tx, _rx) = crossbeam_channel::bounded(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = spawn_accept_loop(
            port,
            false,
            tx,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(String::new())),
            shutdown.clone(),
        )
        .unwrap();
        let url = format!("ws://127.0.0.1:{port}/");

        let bridge = tungstenite::connect(url.as_str());
        assert!(bridge.is_ok(), "bridge refused: {:?}", bridge.err());

        let mut req = url.as_str().into_client_request().unwrap();
        req.headers_mut()
            .insert("Origin", "https://evil.example".parse().unwrap());
        match tungstenite::connect(req) {
            Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 403),
            other => panic!("expected a 403, got {:?}", other.map(|_| ())),
        }

        drop(bridge);
        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }
}
