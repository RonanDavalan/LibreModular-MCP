//! A reference bridge that speaks the protocol, without any application.
//!
//! Why this module exists
//! The Hub must be tested end to end while no application is installed, and its
//! failure paths need a bridge that misbehaves on demand. This implementation is
//! written from the protocol description, independently of the Hub's client, so
//! that a mistake in the client cannot hide in a shared helper.

use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The fake bridge never buffers more than this in one line.
const LINE_CAP: usize = 64 * 1024 * 1024;
const POLL: Duration = Duration::from_millis(5);

/// What the bridge does with the `hello` it receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelloMode {
    /// Check the token in constant time and answer a valid hello.
    Normal,
    /// Defective on purpose: accept any token.
    AcceptAnyToken,
    /// Answer `ok: false`.
    Refuse,
    /// Answer a line that is not JSON.
    Malformed,
    /// Close the connection without answering.
    Close,
    /// Answer a hello for another protocol version.
    WrongProtocol,
    /// Answer a valid hello that announces `krita` instead of `gimp`.
    OtherApp,
}

/// What the bridge answers to one request.
pub enum Reply {
    /// This object, with the request's `id` added.
    Json(Value),
    /// These bytes, exactly; no newline is added.
    Raw(Vec<u8>),
    /// Nothing at all.
    Silent,
    /// The object, after a delay.
    Late(Duration, Value),
    /// Close the connection after reading the request.
    Close,
}

/// Decides the reply to each request (the parsed line, `id` included).
pub type Handler = Arc<dyn Fn(&Value) -> Reply + Send + Sync>;

/// What the bridge observed.
#[derive(Default)]
pub struct Stats {
    pub connections: AtomicUsize,
    /// Connections closed at once because the previous one closed too recently.
    pub turned_away: AtomicUsize,
    pub authenticated: AtomicUsize,
    pub requests: AtomicUsize,
    pub kinds: Mutex<Vec<String>>,
    pub scripts: Mutex<Vec<String>>,
}

pub struct FakeBridge {
    port: u16,
    stats: Arc<Stats>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Compare without stopping at the first difference, as a real bridge must.
fn same_secret(given: &[u8], expected: &[u8]) -> bool {
    let mut difference = given.len() ^ expected.len();
    for (index, byte) in expected.iter().enumerate() {
        difference |= usize::from(byte ^ given.get(index).copied().unwrap_or(0));
    }
    difference == 0
}

impl FakeBridge {
    /// Start listening on an ephemeral loopback port.
    pub fn start(token: &str, hello: HelloMode, handler: Handler) -> FakeBridge {
        FakeBridge::start_slow_to_release(token, hello, handler, Duration::ZERO)
    }

    /// Like [`FakeBridge::start`], but a connection that arrives less than
    /// `release` after the previous served connection closed is closed at once,
    /// as a real bridge does while it has not yet freed its single place.
    pub fn start_slow_to_release(
        token: &str,
        hello: HelloMode,
        handler: Handler,
        release: Duration,
    ) -> FakeBridge {
        let listener =
            TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind loopback");
        listener.set_nonblocking(true).expect("nonblocking");
        let port = listener.local_addr().expect("address").port();
        let stats = Arc::new(Stats::default());
        let stop = Arc::new(AtomicBool::new(false));
        let token = token.to_owned();
        let thread = {
            let stats = Arc::clone(&stats);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut last_closed: Option<Instant> = None;
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, peer)) => {
                            if !peer.ip().is_loopback() {
                                continue;
                            }
                            if last_closed.is_some_and(|closed| closed.elapsed() < release) {
                                stats.turned_away.fetch_add(1, Ordering::SeqCst);
                                drop(stream);
                                continue;
                            }
                            stats.connections.fetch_add(1, Ordering::SeqCst);
                            // One connection at a time, like a real bridge.
                            serve(stream, &token, hello, &handler, &stats, &stop);
                            last_closed = Some(Instant::now());
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            std::thread::sleep(POLL);
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        FakeBridge {
            port,
            stats,
            stop,
            thread: Some(thread),
        }
    }

    /// A bridge that answers every `execute` with `ok` and the script on stdout.
    pub fn echo(token: &str) -> FakeBridge {
        FakeBridge::start(
            token,
            HelloMode::Normal,
            handler(|request| {
                Reply::Json(json!({
                    "ok": true,
                    "stdout": request.get("script").and_then(Value::as_str).unwrap_or(""),
                    "stderr": "",
                    "result": null,
                    "error": null,
                    "duration_ms": 1,
                }))
            }),
        )
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    pub fn connections(&self) -> usize {
        self.stats.connections.load(Ordering::SeqCst)
    }

    pub fn requests(&self) -> usize {
        self.stats.requests.load(Ordering::SeqCst)
    }

    pub fn scripts(&self) -> Vec<String> {
        self.stats.scripts.lock().expect("scripts").clone()
    }

    pub fn kinds(&self) -> Vec<String> {
        self.stats.kinds.lock().expect("kinds").clone()
    }
}

impl Drop for FakeBridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Wrap a closure as a handler.
pub fn handler(f: impl Fn(&Value) -> Reply + Send + Sync + 'static) -> Handler {
    Arc::new(f)
}

/// Read one line; `None` on close, on stop, or on a line over the cap.
fn read_line(stream: &mut TcpStream, pending: &mut Vec<u8>, stop: &AtomicBool) -> Option<Vec<u8>> {
    loop {
        if let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let rest = pending.split_off(end + 1);
            let mut line = std::mem::replace(pending, rest);
            line.pop();
            return Some(line);
        }
        if pending.len() > LINE_CAP || stop.load(Ordering::SeqCst) {
            return None;
        }
        let mut chunk = [0_u8; 65536];
        match stream.read(&mut chunk) {
            Ok(0) => return None,
            Ok(count) => pending.extend_from_slice(&chunk[..count]),
            Err(error) if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => return None,
        }
    }
}

fn send(stream: &mut TcpStream, value: &Value) -> bool {
    let mut line = serde_json::to_vec(value).expect("encode");
    line.push(b'\n');
    stream.write_all(&line).is_ok()
}

fn hello_answer(protocol: u64, app: &str) -> Value {
    json!({
        "type": "hello",
        "ok": true,
        "app": app,
        "app_version": "3.0.4",
        "bridge_version": "1",
        "protocol": protocol,
    })
}

fn serve(
    mut stream: TcpStream,
    token: &str,
    hello: HelloMode,
    handler: &Handler,
    stats: &Stats,
    stop: &AtomicBool,
) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(20)));
    let mut pending = Vec::new();
    let Some(first) = read_line(&mut stream, &mut pending, stop) else {
        return;
    };
    let Ok(Value::Object(greeting)) = serde_json::from_slice::<Value>(&first) else {
        return; // not a hello: close without a word
    };
    let given = greeting.get("token").and_then(Value::as_str).unwrap_or("");
    let valid = greeting.get("type").and_then(Value::as_str) == Some("hello")
        && greeting.get("protocol").and_then(Value::as_u64) == Some(1)
        && (hello == HelloMode::AcceptAnyToken || same_secret(given.as_bytes(), token.as_bytes()));
    if !valid {
        return;
    }
    let answered = match hello {
        HelloMode::Normal | HelloMode::AcceptAnyToken => {
            send(&mut stream, &hello_answer(1, "gimp"))
        }
        HelloMode::Refuse => send(&mut stream, &json!({"type": "hello", "ok": false})),
        HelloMode::Malformed => stream.write_all(b"this is not json\n").is_ok(),
        HelloMode::WrongProtocol => send(&mut stream, &hello_answer(2, "gimp")),
        HelloMode::OtherApp => send(&mut stream, &hello_answer(1, "krita")),
        HelloMode::Close => false,
    };
    if !answered || !matches!(hello, HelloMode::Normal | HelloMode::AcceptAnyToken) {
        // Leave the client time to read the answer before the connection closes.
        std::thread::sleep(Duration::from_millis(50));
        return;
    }
    stats.authenticated.fetch_add(1, Ordering::SeqCst);
    while let Some(line) = read_line(&mut stream, &mut pending, stop) {
        let Ok(request) = serde_json::from_slice::<Value>(&line) else {
            return;
        };
        stats.requests.fetch_add(1, Ordering::SeqCst);
        let kind = request
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if kind == "execute" {
            let script = request.get("script").and_then(Value::as_str).unwrap_or("");
            stats
                .scripts
                .lock()
                .expect("scripts")
                .push(script.to_owned());
        }
        stats.kinds.lock().expect("kinds").push(kind.clone());
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let with_id = |mut value: Value| {
            if let Value::Object(entries) = &mut value {
                entries.insert("id".to_owned(), id.clone());
            }
            value
        };
        let reply = if kind == "ping" {
            Reply::Json(json!({"ok": true}))
        } else {
            handler(&request)
        };
        let delivered = match reply {
            Reply::Json(value) => send(&mut stream, &with_id(value)),
            Reply::Raw(bytes) => stream.write_all(&bytes).is_ok(),
            Reply::Silent => true,
            Reply::Late(delay, value) => {
                let until = Instant::now() + delay;
                while Instant::now() < until && !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(POLL);
                }
                send(&mut stream, &with_id(value))
            }
            Reply::Close => false,
        };
        if !delivered {
            return;
        }
    }
}

/// The smallest valid PNG: one transparent pixel.
pub const TINY_PNG: [u8; 67] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

/// Open a raw connection and send one line, as an arbitrary local client would.
pub fn raw_exchange(port: u16, line: &[u8]) -> Vec<u8> {
    let mut stream =
        TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    stream.write_all(line).expect("write");
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer);
    answer
}
