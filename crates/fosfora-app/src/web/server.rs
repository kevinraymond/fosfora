use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::Sender;

use super::client;
use super::types::WsInMessage;
use crate::inbound::DropOldestSender;

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

/// Most connections served at once: WebSocket clients plus HTTP requests and
/// handshakes in flight. Each holds one thread; beyond this a connection gets a
/// 503 and is closed, so a flood of sockets cannot exhaust threads.
const MAX_CONNECTIONS: usize = 32;

/// One slot of [`MAX_CONNECTIONS`], released when the connection's thread ends.
struct ConnectionSlot(Arc<AtomicUsize>);

impl ConnectionSlot {
    fn acquire(open: &Arc<AtomicUsize>) -> Option<Self> {
        open.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < MAX_CONNECTIONS).then_some(n + 1)
        })
        .ok()
        .map(|_| Self(Arc::clone(open)))
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Spawn the accept loop thread. Returns (shutdown_flag, thread_handle).
///
/// The accept thread only accepts: each connection's first read, handshake and
/// client loop run on that connection's own thread, so an idle socket that never
/// sends a request cannot hold up anyone else.
pub(crate) fn spawn_accept_loop(
    port: u16,
    lan: bool,
    access_key: String,
    inbound_tx: DropOldestSender<WsInMessage>,
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
    let open_connections = Arc::new(AtomicUsize::new(0));
    let access_key: Arc<str> = access_key.into();

    let handle = thread::Builder::new()
        .name("fosfora-web-accept".into())
        .spawn(move || {
            // Set the listener to have a timeout for accept
            let _ = listener.set_nonblocking(true);

            while !shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, addr)) => {
                        log::debug!("Web connection from {addr}");
                        let _ = stream.set_nonblocking(false);
                        let Some(slot) = ConnectionSlot::acquire(&open_connections) else {
                            log::warn!(
                                "Refused web connection from {addr}: {MAX_CONNECTIONS} already open"
                            );
                            let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
                            let _ = stream.write_all(
                                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                            );
                            continue;
                        };
                        let inbound_tx = inbound_tx.clone();
                        let clients = Arc::clone(&clients);
                        let latest_state = Arc::clone(&latest_state);
                        let shutdown = Arc::clone(&shutdown);
                        let client_counter = Arc::clone(&client_counter);
                        let access_key = Arc::clone(&access_key);
                        let from_loopback = addr.ip().is_loopback();
                        let spawned = thread::Builder::new()
                            .name("fosfora-web-conn".into())
                            .spawn(move || {
                                let _slot = slot;
                                handle_connection(
                                    stream,
                                    Gate {
                                        lan,
                                        from_loopback,
                                        access_key: &access_key,
                                    },
                                    &inbound_tx,
                                    &clients,
                                    &latest_state,
                                    &shutdown,
                                    &client_counter,
                                );
                            });
                        if let Err(e) = spawned {
                            log::warn!("Web connection thread failed to start: {e}");
                        }
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

/// What a WebSocket upgrade is checked against: the server's settings and
/// where the connection came from.
#[derive(Clone, Copy)]
struct Gate<'a> {
    /// LAN access is on.
    lan: bool,
    /// The peer address is loopback: the connection comes from this computer.
    from_loopback: bool,
    /// The access key another device must present (#43); empty refuses all.
    access_key: &'a str,
}

fn handle_connection(
    mut stream: TcpStream,
    gate: Gate<'_>,
    inbound_tx: &DropOldestSender<WsInMessage>,
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
        if let Err(why) = upgrade_allowed(&request, gate) {
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

                // Already on this connection's own thread (it holds the slot).
                client::run_client(
                    ws,
                    inbound_tx.clone(),
                    outbound_rx,
                    state,
                    shutdown.clone(),
                    client_id,
                );
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

/// Whether a `Host` header value names this computer's loopback interface.
fn is_loopback_host(host: &str) -> bool {
    matches!(
        host_name(host).to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

/// Value of query parameter `name` in the request line's target
/// (`GET /ws?key=… HTTP/1.1`).
fn query_param<'a>(request: &'a str, name: &str) -> Option<&'a str> {
    let target = request.lines().next()?.split_whitespace().nth(1)?;
    let (_, query) = target.split_once('?')?;
    query
        .split('&')
        .find_map(|pair| pair.split_once('=').filter(|(k, _)| *k == name))
        .map(|(_, v)| v)
}

/// `given == expected` without an early exit on the first differing byte, so
/// response timing does not reveal how much of a guessed key was right. An
/// empty `expected` matches nothing.
fn key_matches(given: &str, expected: &str) -> bool {
    !expected.is_empty()
        && given.len() == expected.len()
        && given
            .bytes()
            .zip(expected.bytes())
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
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
/// - Anything but this computer addressing itself as localhost must present
///   the access key as `?key=`: another device, and — with LAN access on — a
///   DNS rebinding page, which reaches the server from this computer but under
///   its own name. The key is never served over HTTP, so neither can learn it
///   from the control page.
fn upgrade_allowed(request: &str, gate: Gate<'_>) -> Result<(), String> {
    let host = header(request, "host");
    let local_host = host.is_none_or(is_loopback_host);
    if !gate.lan && !local_host {
        let h = host.unwrap_or_default();
        return Err(format!("Host {h} is not this computer (LAN access is off)"));
    }
    if let Some(origin) = header(request, "origin") {
        let origin_host = origin
            .split_once("://")
            .map(|(_, rest)| rest.trim_end_matches('/'));
        match (origin_host, host) {
            (Some(o), Some(h)) if o.eq_ignore_ascii_case(h) => {}
            _ => return Err(format!("page origin {origin} is not this server")),
        }
    }
    let exempt = gate.from_loopback && local_host;
    if !exempt && !query_param(request, "key").is_some_and(|k| key_matches(k, gate.access_key)) {
        return Err(
            "no valid access key (another device needs the network link from \
                    Setup > Control > Web remote)"
                .to_string(),
        );
    }
    Ok(())
}

fn is_websocket_upgrade(request: &str) -> bool {
    // Check for WebSocket upgrade headers (case-insensitive)
    let lower = request.to_lowercase();
    lower.contains("upgrade: websocket") || lower.contains("upgrade:websocket")
}

fn serve_http(stream: &mut TcpStream, request: &str) {
    // Parse the request path; the query (the network link's `?key=`) is for
    // the page's script, not for routing.
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .map_or("/", |target| target.split('?').next().unwrap_or(target));

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
        upgrade_to("/", headers)
    }

    fn upgrade_to(target: &str, headers: &[&str]) -> String {
        let mut r =
            format!("GET {target} HTTP/1.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n");
        for h in headers {
            r.push_str(h);
            r.push_str("\r\n");
        }
        r + "\r\n"
    }

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    /// This computer, LAN access off.
    const LOCAL: Gate<'static> = Gate {
        lan: false,
        from_loopback: true,
        access_key: KEY,
    };
    /// This computer, LAN access on.
    const LOCAL_LAN: Gate<'static> = Gate { lan: true, ..LOCAL };
    /// Another device (LAN access is necessarily on).
    const DEVICE: Gate<'static> = Gate {
        from_loopback: false,
        ..LOCAL_LAN
    };

    #[test]
    fn the_served_control_page_and_bridges_are_let_in() {
        // The control page, loaded from this server.
        let page = upgrade(&["Host: localhost:9002", "Origin: http://localhost:9002"]);
        assert!(upgrade_allowed(&page, LOCAL).is_ok());
        assert!(upgrade_allowed(&page, LOCAL_LAN).is_ok());
        // A bridge: no Origin.
        assert!(upgrade_allowed(&upgrade(&["Host: 127.0.0.1:9002"]), LOCAL).is_ok());
        // A phone on the LAN, with the key from the network link.
        let phone = upgrade_to(
            &format!("/ws?key={KEY}"),
            &["Host: 192.168.1.5:9002", "Origin: http://192.168.1.5:9002"],
        );
        assert!(upgrade_allowed(&phone, DEVICE).is_ok());
        // A bridge in Docker, key among other parameters.
        let docker = upgrade_to(&format!("/bind?x=1&key={KEY}"), &["Host: 172.17.0.1:9002"]);
        assert!(upgrade_allowed(&docker, DEVICE).is_ok());
    }

    #[test]
    fn another_device_needs_the_access_key() {
        let headers = ["Host: 192.168.1.5:9002", "Origin: http://192.168.1.5:9002"];
        assert!(upgrade_allowed(&upgrade_to("/ws", &headers), DEVICE).is_err());
        let wrong = upgrade_to("/ws?key=0123456789abcdef0123456789abcdee", &headers);
        assert!(upgrade_allowed(&wrong, DEVICE).is_err());
        let prefix = upgrade_to("/ws?key=0123", &headers);
        assert!(upgrade_allowed(&prefix, DEVICE).is_err());
        let empty = upgrade_to("/ws?key=", &headers);
        assert!(upgrade_allowed(&empty, DEVICE).is_err());
        // The key only counts in the query, not as some other parameter's value.
        let elsewhere = upgrade_to(&format!("/ws?notkey={KEY}"), &headers);
        assert!(upgrade_allowed(&elsewhere, DEVICE).is_err());
        // A bridge's localhost Host header does not stand in for the key.
        let spoofed = upgrade(&["Host: localhost:9002"]);
        assert!(upgrade_allowed(&spoofed, DEVICE).is_err());
    }

    #[test]
    fn an_empty_configured_key_refuses_every_other_device() {
        let none = Gate {
            access_key: "",
            ..DEVICE
        };
        let phone = upgrade_to("/ws?key=", &["Host: 192.168.1.5:9002"]);
        assert!(upgrade_allowed(&phone, none).is_err());
        // This computer is unaffected.
        let local = Gate {
            access_key: "",
            ..LOCAL_LAN
        };
        assert!(upgrade_allowed(&upgrade(&["Host: localhost:9002"]), local).is_ok());
    }

    /// With LAN access on the Host check is off, so a rebinding page on this
    /// computer gets through the Origin check under its own name. The key
    /// stops it: the page cannot read it from anything this server serves.
    #[test]
    fn a_rebinding_page_needs_the_key_even_from_this_computer() {
        let rebind = upgrade(&[
            "Host: evil.example:9002",
            "Origin: http://evil.example:9002",
        ]);
        assert!(upgrade_allowed(&rebind, LOCAL_LAN).is_err());
        // By this machine's LAN address from its own browser: same rule.
        let by_ip = upgrade(&["Host: 192.168.1.5:9002", "Origin: http://192.168.1.5:9002"]);
        assert!(upgrade_allowed(&by_ip, LOCAL_LAN).is_err());
        let with_key = upgrade_to(
            &format!("/ws?key={KEY}"),
            &["Host: 192.168.1.5:9002", "Origin: http://192.168.1.5:9002"],
        );
        assert!(upgrade_allowed(&with_key, LOCAL_LAN).is_ok());
    }

    #[test]
    fn keys_compare_whole() {
        assert!(key_matches(KEY, KEY));
        assert!(!key_matches("", ""));
        assert!(!key_matches(&KEY[..31], KEY));
        assert!(!key_matches(&format!("{KEY}0"), KEY));
    }

    #[test]
    fn a_foreign_web_page_is_refused() {
        let evil = upgrade(&["Host: 127.0.0.1:9002", "Origin: https://evil.example"]);
        assert!(upgrade_allowed(&evil, LOCAL).is_err());
        assert!(upgrade_allowed(&evil, LOCAL_LAN).is_err());
        // The key does not excuse a foreign page.
        let keyed = upgrade_to(
            &format!("/ws?key={KEY}"),
            &["Host: 127.0.0.1:9002", "Origin: https://evil.example"],
        );
        assert!(upgrade_allowed(&keyed, DEVICE).is_err());
        let file = upgrade(&["Host: localhost:9002", "Origin: null"]);
        assert!(upgrade_allowed(&file, LOCAL).is_err());
        // Same host, different port: another local web app.
        let other = upgrade(&["Host: localhost:9002", "Origin: http://localhost:3000"]);
        assert!(upgrade_allowed(&other, LOCAL).is_err());
    }

    #[test]
    fn loopback_only_refuses_foreign_host_names() {
        // DNS rebinding: the attacker's name resolves to 127.0.0.1, same origin.
        let rebind = upgrade(&[
            "Host: evil.example:9002",
            "Origin: http://evil.example:9002",
        ]);
        assert!(upgrade_allowed(&rebind, LOCAL).is_err());
        assert!(upgrade_allowed(&upgrade(&["Host: [::1]:9002"]), LOCAL).is_ok());
        assert!(upgrade_allowed(&upgrade(&["host: LOCALHOST:9002"]), LOCAL).is_ok());
    }

    #[test]
    fn server_binds_loopback_unless_lan_is_on() {
        let local = bind_listener(0, false).unwrap().local_addr().unwrap();
        assert!(local.ip().is_loopback(), "{local}");
        let lan = bind_listener(0, true).unwrap().local_addr().unwrap();
        assert!(lan.ip().is_unspecified(), "{lan}");
    }

    /// A running loopback server on a free port, plus its shutdown flag.
    fn running_server() -> (u16, Arc<AtomicBool>, JoinHandle<()>) {
        let port = bind_listener(0, false)
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let (tx, _rx) = crate::inbound::bounded(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = spawn_accept_loop(
            port,
            false,
            KEY.to_string(),
            tx,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(String::new())),
            shutdown.clone(),
        )
        .unwrap();
        (port, shutdown, handle)
    }

    /// Response head of one plain HTTP request.
    fn http_get(port: u16, path: &str) -> String {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        write!(s, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").unwrap();
        let mut buf = [0u8; 256];
        let n = s.read(&mut buf).unwrap_or(0);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }

    /// An idle socket used to hold the accept thread for its 5 s read timeout,
    /// stalling every other client (#44).
    #[test]
    fn an_idle_connection_does_not_stall_others() {
        let (port, shutdown, handle) = running_server();
        let _idle = TcpStream::connect(("127.0.0.1", port)).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        let start = std::time::Instant::now();
        let resp = http_get(port, "/health");
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "took {:?}",
            start.elapsed()
        );

        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    #[test]
    fn connections_beyond_the_cap_get_a_503() {
        let (port, shutdown, handle) = running_server();
        let idle: Vec<_> = (0..MAX_CONNECTIONS)
            .map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap())
            .collect();
        std::thread::sleep(Duration::from_millis(300));
        let resp = http_get(port, "/health");
        assert!(resp.starts_with("HTTP/1.1 503"), "{resp}");

        // Closing them frees the slots.
        drop(idle);
        std::thread::sleep(Duration::from_millis(300));
        let resp = http_get(port, "/health");
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");

        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap();
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
        let (tx, _rx) = crate::inbound::bounded(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = spawn_accept_loop(
            port,
            false,
            KEY.to_string(),
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
    /// The network link (`/?key=…`) serves the page instead of redirecting to
    /// `/`, which would drop the key before the page's script could read it.
    #[test]
    fn the_network_link_serves_the_page_with_its_key_intact() {
        let (port, shutdown, handle) = running_server();
        let resp = http_get(port, &format!("/?key={KEY}"));
        assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");
        assert!(http_get(port, "/nope?key=x").starts_with("HTTP/1.1 302"));
        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    /// Through the real accept loop with LAN access on: a connection that is
    /// not this computer addressing itself as localhost gets a 403 without the
    /// key and completes the handshake with it.
    #[test]
    fn accept_loop_checks_the_access_key() {
        use tungstenite::client::IntoClientRequest;

        let port = bind_listener(0, true).unwrap().local_addr().unwrap().port();
        let (tx, _rx) = crate::inbound::bounded(1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let handle = spawn_accept_loop(
            port,
            true,
            KEY.to_string(),
            tx,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(String::new())),
            shutdown.clone(),
        )
        .unwrap();
        let request = |query: &str| {
            let mut req = format!("ws://127.0.0.1:{port}/bind{query}")
                .into_client_request()
                .unwrap();
            req.headers_mut()
                .insert("Host", format!("fosfora.lan:{port}").parse().unwrap());
            req
        };

        match tungstenite::connect(request("")) {
            Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status(), 403),
            other => panic!("expected a 403, got {:?}", other.map(|_| ())),
        }
        let keyed = tungstenite::connect(request(&format!("?key={KEY}")));
        assert!(keyed.is_ok(), "keyed bridge refused: {:?}", keyed.err());
        // This computer as localhost still needs none.
        let local = tungstenite::connect(format!("ws://127.0.0.1:{port}/bind"));
        assert!(local.is_ok(), "local bridge refused: {:?}", local.err());

        drop((keyed, local));
        shutdown.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }
}
