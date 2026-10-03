//! Client side of the line-delimited JSON protocol spoken with the bridges.
//!
//! Why this module exists
//! A bridge runs code with the user's rights, so the Hub must only ever reach it
//! on the loopback address, prove it holds the token, and never let a slow or
//! hostile bridge fill its memory or receive a second script while the first one
//! is still running. This is the only module of the Hub that touches the network.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::config::Limits;
use crate::security::Token;

/// The only address the Hub ever connects to.
pub const BRIDGE_ADDRESS: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);
/// The protocol version the Hub speaks and requires from the bridge.
pub const PROTOCOL_VERSION: u64 = 1;
/// How long the Hub waits for the bridge to accept the connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long the Hub waits for the answer to its `hello`.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

const READ_CHUNK: usize = 64 * 1024;

/// Why a request to a bridge did not produce an answer.
///
/// No variant carries the token or a line received from the bridge.
#[derive(Debug)]
pub enum BridgeError {
    /// No bridge accepted the connection.
    Connect(io::Error),
    /// The bridge did not complete the handshake; the text says what was wrong.
    Handshake(&'static str),
    /// A request is already in progress, or a late answer is still awaited.
    Busy,
    /// The answer did not arrive in time; the client stays busy until it does.
    Timeout,
    /// The connection is gone. `sent` tells whether the request had been written.
    Disconnected { sent: bool },
    /// A line longer than the limit, in either direction.
    LineTooLong { limit: usize },
    /// The bridge answered something the protocol does not allow.
    Protocol(&'static str),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BridgeError::Connect(error) => write!(f, "cannot connect to the bridge: {error}"),
            BridgeError::Handshake(what) => write!(f, "bridge handshake failed: {what}"),
            BridgeError::Busy => f.write_str("the bridge is busy with a previous request"),
            BridgeError::Timeout => f.write_str("the bridge did not answer in time"),
            BridgeError::Disconnected { sent: true } => {
                f.write_str("the connection to the bridge closed after the request was sent")
            }
            BridgeError::Disconnected { sent: false } => {
                f.write_str("the connection to the bridge is closed")
            }
            BridgeError::LineTooLong { limit } => {
                write!(f, "a protocol line is longer than {limit} bytes")
            }
            BridgeError::Protocol(what) => write!(f, "protocol error: {what}"),
        }
    }
}

impl std::error::Error for BridgeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BridgeError::Connect(error) => Some(error),
            _ => None,
        }
    }
}

/// What the bridge said about itself in its `hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub app: String,
    pub app_version: String,
    pub bridge_version: String,
}

/// Everything needed to open a connection to one bridge.
#[derive(Clone)]
pub struct BridgeConnector {
    port: u16,
    token: Token,
    limits: Limits,
}

impl BridgeConnector {
    #[must_use]
    pub fn new(port: u16, token: Token, limits: Limits) -> Self {
        BridgeConnector {
            port,
            token,
            limits,
        }
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Open a new connection and complete the handshake.
    ///
    /// # Errors
    /// See [`BridgeClient::connect`].
    pub fn connect(&self) -> Result<BridgeClient, BridgeError> {
        BridgeClient::connect(self.port, &self.token, &self.limits)
    }
}

impl fmt::Debug for BridgeConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeConnector")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

struct Connection {
    stream: TcpStream,
    buffer: Vec<u8>,
    max_line_bytes: usize,
    /// The id of a request that timed out and whose answer has not arrived.
    late: Option<String>,
}

enum Poll {
    Line(Vec<u8>),
    Nothing,
}

impl Connection {
    /// Take one complete line out of the buffer, if there is one.
    fn take_line(&mut self) -> Result<Option<Vec<u8>>, BridgeError> {
        if let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let rest = self.buffer.split_off(end + 1);
            let mut line = std::mem::replace(&mut self.buffer, rest);
            line.pop();
            if line.len() > self.max_line_bytes {
                return Err(BridgeError::LineTooLong {
                    limit: self.max_line_bytes,
                });
            }
            return Ok(Some(line));
        }
        if self.buffer.len() > self.max_line_bytes {
            return Err(BridgeError::LineTooLong {
                limit: self.max_line_bytes,
            });
        }
        Ok(None)
    }

    /// Read more bytes; never more than one line's worth beyond the limit, so a
    /// line without an end cannot fill the memory.
    fn fill(&mut self) -> io::Result<usize> {
        let room = self
            .max_line_bytes
            .saturating_add(1)
            .saturating_sub(self.buffer.len())
            .clamp(1, READ_CHUNK);
        let mut chunk = vec![0_u8; room];
        let count = self.stream.read(&mut chunk)?;
        chunk.truncate(count);
        self.buffer.extend_from_slice(&chunk);
        Ok(count)
    }

    /// Wait for one line until `deadline`.
    fn read_line(&mut self, deadline: Instant, sent: bool) -> Result<Vec<u8>, BridgeError> {
        loop {
            if let Some(line) = self.take_line()? {
                return Ok(line);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(BridgeError::Timeout);
            }
            // A zero read timeout is rejected by the system: wait at least a millisecond.
            self.stream
                .set_read_timeout(Some(remaining.max(Duration::from_millis(1))))
                .map_err(|_| BridgeError::Disconnected { sent })?;
            match self.fill() {
                Ok(0) => return Err(BridgeError::Disconnected { sent }),
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return Err(BridgeError::Disconnected { sent }),
            }
        }
    }

    /// Look for a line without waiting.
    fn poll_line(&mut self) -> Result<Poll, BridgeError> {
        let closed = BridgeError::Disconnected { sent: false };
        if let Some(line) = self.take_line()? {
            return Ok(Poll::Line(line));
        }
        self.stream.set_nonblocking(true).map_err(|_| closed)?;
        let outcome = loop {
            match self.fill() {
                Ok(0) => break Err(BridgeError::Disconnected { sent: false }),
                Ok(_) => {
                    if let Some(line) = self.take_line()? {
                        break Ok(Poll::Line(line));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break Ok(Poll::Nothing),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break Err(BridgeError::Disconnected { sent: false }),
            }
        };
        self.stream
            .set_nonblocking(false)
            .map_err(|_| BridgeError::Disconnected { sent: false })?;
        outcome
    }

    fn write_line(&mut self, line: &[u8], deadline: Instant) -> Result<(), BridgeError> {
        if line.len() > self.max_line_bytes {
            return Err(BridgeError::LineTooLong {
                limit: self.max_line_bytes,
            });
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let wait = if remaining.is_zero() {
            Duration::from_millis(1)
        } else {
            remaining
        };
        // Whatever goes wrong while writing, part of the request may have left:
        // it is reported as sent so that it is never written a second time.
        let gone = || BridgeError::Disconnected { sent: true };
        self.stream
            .set_write_timeout(Some(wait))
            .map_err(|_| gone())?;
        self.stream.write_all(line).map_err(|_| gone())?;
        self.stream.write_all(b"\n").map_err(|_| gone())?;
        self.stream.flush().map_err(|_| gone())
    }
}

fn parse_object(line: &[u8]) -> Result<Map<String, Value>, BridgeError> {
    match serde_json::from_slice::<Value>(line) {
        Ok(Value::Object(object)) => Ok(object),
        Ok(_) => Err(BridgeError::Protocol("a line is not a JSON object")),
        Err(_) => Err(BridgeError::Protocol("a line is not valid JSON")),
    }
}

fn text_field(object: &Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn check_hello(object: &Map<String, Value>) -> Result<Hello, BridgeError> {
    if object.get("type").and_then(Value::as_str) != Some("hello") {
        return Err(BridgeError::Handshake("the answer is not a hello"));
    }
    if object.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(BridgeError::Handshake("the bridge refused the hello"));
    }
    if object.get("protocol").and_then(Value::as_u64) != Some(PROTOCOL_VERSION) {
        return Err(BridgeError::Handshake(
            "the bridge speaks another protocol version",
        ));
    }
    match (
        text_field(object, "app"),
        text_field(object, "app_version"),
        text_field(object, "bridge_version"),
    ) {
        (Some(app), Some(app_version), Some(bridge_version)) => Ok(Hello {
            app,
            app_version,
            bridge_version,
        }),
        _ => Err(BridgeError::Handshake(
            "the hello lacks the application or a version",
        )),
    }
}

/// One authenticated connection to a bridge.
///
/// At most one request is in flight: a second one, or any request while the
/// answer to a timed-out one is still awaited, gets [`BridgeError::Busy`] at once
/// and nothing is written to the bridge.
pub struct BridgeClient {
    connection: Mutex<Connection>,
    /// A second handle on the socket, so that `close` never waits for a request.
    control: TcpStream,
    hello: Hello,
    next_id: AtomicU64,
    broken: AtomicBool,
}

impl BridgeClient {
    /// Connect to the bridge listening on `port` of the loopback address, send the
    /// `hello` with the token and check the answer.
    ///
    /// # Errors
    /// [`BridgeError::Connect`] when nothing listens, [`BridgeError::Handshake`]
    /// for any answer other than a valid `hello` (including a closed connection).
    pub fn connect(port: u16, token: &Token, limits: &Limits) -> Result<BridgeClient, BridgeError> {
        let address = SocketAddr::V4(SocketAddrV4::new(BRIDGE_ADDRESS, port));
        let stream =
            TcpStream::connect_timeout(&address, CONNECT_TIMEOUT).map_err(BridgeError::Connect)?;
        let _ = stream.set_nodelay(true);
        let control = stream.try_clone().map_err(BridgeError::Connect)?;
        let mut connection = Connection {
            stream,
            buffer: Vec::new(),
            max_line_bytes: limits.max_line_bytes,
            late: None,
        };
        let greeting =
            json!({"type": "hello", "token": token.as_str(), "protocol": PROTOCOL_VERSION});
        let line = serde_json::to_vec(&greeting)
            .map_err(|_| BridgeError::Protocol("cannot encode the hello"))?;
        let deadline = Instant::now() + HELLO_TIMEOUT;
        let handshake = |error: BridgeError| match error {
            BridgeError::Timeout => BridgeError::Handshake("no answer to the hello"),
            BridgeError::Disconnected { .. } => {
                BridgeError::Handshake("the bridge closed the connection during the handshake")
            }
            BridgeError::LineTooLong { .. } => {
                BridgeError::Handshake("the answer to the hello is too long")
            }
            BridgeError::Protocol(_) => {
                BridgeError::Handshake("the answer to the hello is not a JSON object")
            }
            other => other,
        };
        connection.write_line(&line, deadline).map_err(handshake)?;
        let answer = connection.read_line(deadline, true).map_err(handshake)?;
        let hello = check_hello(&parse_object(&answer).map_err(handshake)?)?;
        Ok(BridgeClient {
            connection: Mutex::new(connection),
            control,
            hello,
            next_id: AtomicU64::new(1),
            broken: AtomicBool::new(false),
        })
    }

    /// What the bridge announced in its `hello`.
    #[must_use]
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// False once the connection failed or was closed; a new client is needed.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.broken.load(Ordering::SeqCst)
    }

    /// Send one request and wait at most `timeout` for the answer with the same id.
    ///
    /// `payload` is a JSON object without `id`; the client adds it.
    ///
    /// # Errors
    /// [`BridgeError::Busy`] without sending anything when a request is in
    /// progress or a late answer is still awaited; [`BridgeError::Timeout`] after
    /// which the client stays busy; any other variant leaves the client unusable.
    pub fn request(&self, payload: Value, timeout: Duration) -> Result<Value, BridgeError> {
        let mut connection = match self.connection.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::WouldBlock) => return Err(BridgeError::Busy),
            Err(TryLockError::Poisoned(_)) => {
                self.broken.store(true, Ordering::SeqCst);
                return Err(BridgeError::Disconnected { sent: false });
            }
        };
        if !self.is_usable() {
            return Err(BridgeError::Disconnected { sent: false });
        }
        let outcome = self.exchange(&mut connection, payload, timeout);
        if let Err(error) = &outcome
            && !matches!(error, BridgeError::Busy | BridgeError::Timeout)
        {
            self.broken.store(true, Ordering::SeqCst);
            let _ = connection.stream.shutdown(Shutdown::Both);
        }
        outcome
    }

    fn exchange(
        &self,
        connection: &mut Connection,
        payload: Value,
        timeout: Duration,
    ) -> Result<Value, BridgeError> {
        if let Some(awaited) = connection.late.clone() {
            match connection.poll_line()? {
                Poll::Nothing => return Err(BridgeError::Busy),
                Poll::Line(line) => {
                    let late = parse_object(&line)?;
                    if late.get("id").and_then(Value::as_str) != Some(awaited.as_str()) {
                        return Err(BridgeError::Protocol("an answer carries an unexpected id"));
                    }
                    // The late answer belongs to a call that already returned.
                    connection.late = None;
                }
            }
        }
        let Value::Object(mut object) = payload else {
            return Err(BridgeError::Protocol("a request must be a JSON object"));
        };
        let id = format!("r{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        object.insert("id".to_owned(), Value::String(id.clone()));
        let line = serde_json::to_vec(&Value::Object(object))
            .map_err(|_| BridgeError::Protocol("cannot encode the request"))?;
        let deadline = Instant::now() + timeout;
        connection.write_line(&line, deadline)?;
        let answer = match connection.read_line(deadline, true) {
            Ok(answer) => answer,
            Err(BridgeError::Timeout) => {
                connection.late = Some(id);
                return Err(BridgeError::Timeout);
            }
            Err(other) => return Err(other),
        };
        let object = parse_object(&answer)?;
        if object.get("id").and_then(Value::as_str) != Some(id.as_str()) {
            return Err(BridgeError::Protocol("an answer carries an unexpected id"));
        }
        Ok(Value::Object(object))
    }

    /// Close the connection. Nothing is sent to the bridge or the application.
    pub fn close(&self) {
        self.broken.store(true, Ordering::SeqCst);
        let _ = self.control.shutdown(Shutdown::Both);
    }
}

impl fmt::Debug for BridgeClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeClient")
            .field("hello", &self.hello)
            .field("usable", &self.is_usable())
            .finish_non_exhaustive()
    }
}
