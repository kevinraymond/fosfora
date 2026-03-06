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
    inbound_tx: Sender<WsInMessage>,
    clients: Arc<Mutex<Vec<Sender<String>>>>,
    latest_state: Arc<Mutex<String>>,
    shutdown: Arc<AtomicBool>,
) -> anyhow::Result<JoinHandle<()>> {
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr)?;
    listener.set_nonblocking(false)?;
    // Short accept timeout so we can check shutdown flag
    let _ = listener.set_nonblocking(false);
    log::info!("Web control server listening on http://0.0.0.0:{port}");

    let client_counter = Arc::new(AtomicUsize::new(0));

    let handle = thread::Builder::new()
        .name("phosphor-web-accept".into())
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

fn handle_connection(
    mut stream: TcpStream,
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
        // Set read timeout for interleaved read/write in client handler
        let _ = stream.set_read_timeout(Some(Duration::from_millis(50)));
        // Replay already-read bytes then continue from the stream
        let replay = ReplayStream::new(buf[..n].to_vec(), stream);
        match tungstenite::accept(replay) {
            Ok(ws) => {
                let client_id = client_counter.fetch_add(1, Ordering::Relaxed);
                let (outbound_tx, outbound_rx) = crossbeam_channel::bounded(256);

                // Get latest state for initial sync
                let state = latest_state.lock().unwrap().clone();

                // Register client
                clients.lock().unwrap().push(outbound_tx);

                let tx = inbound_tx.clone();
                let flag = shutdown.clone();

                thread::Builder::new()
                    .name(format!("phosphor-web-client-{client_id}"))
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
