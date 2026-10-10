//! Côté client du protocole JSON par lignes parlé avec les ponts.
//!
//! Pourquoi ce module existe
//! Un pont exécute du code avec les droits de l'utilisateur : le Hub ne doit donc l'atteindre
//! que sur l'adresse de bouclage, prouver qu'il détient le jeton, et ne jamais laisser un pont
//! lent ou hostile lui remplir la mémoire ni recevoir un second script tant que le premier
//! tourne. C'est le seul module du Hub qui touche le réseau.

use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, SocketAddrV4, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, Instant};

use serde_json::{Map, Value, json};

use crate::config::Limits;
use crate::security::Token;

/// La seule adresse à laquelle le Hub se connecte jamais.
pub const BRIDGE_ADDRESS: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 1);
/// La version du protocole que le Hub parle et exige du pont.
pub const PROTOCOL_VERSION: u64 = 1;
/// Combien de temps le Hub attend que le pont accepte la connexion.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// Combien de temps le Hub attend la réponse à son `hello`.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

const READ_CHUNK: usize = 64 * 1024;

/// Pourquoi une requête à un pont n'a pas produit de réponse.
///
/// Aucune variante ne porte le jeton ni une ligne reçue du pont.
#[derive(Debug)]
pub enum BridgeError {
    /// Aucun pont n'a accepté la connexion.
    Connect(io::Error),
    /// Le pont n'a pas terminé la poignée de main ; le texte dit ce qui n'allait pas.
    Handshake(&'static str),
    /// Une requête est déjà en cours, ou une réponse tardive est encore attendue.
    Busy,
    /// La réponse n'est pas arrivée à temps ; le client reste occupé jusqu'à ce qu'elle arrive.
    Timeout,
    /// La connexion a disparu. `sent` dit si la requête avait été écrite.
    Disconnected { sent: bool },
    /// Une ligne plus longue que la limite, dans un sens ou dans l'autre.
    LineTooLong { limit: usize },
    /// Le pont a répondu quelque chose que le protocole ne permet pas.
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

/// Ce que le pont a dit de lui-même dans son `hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hello {
    pub app: String,
    pub app_version: String,
    pub bridge_version: String,
}

/// Tout ce qu'il faut pour ouvrir une connexion vers un pont.
#[derive(Clone)]
pub struct BridgeConnector {
    app: &'static str,
    port: u16,
    token: Token,
    limits: Limits,
}

impl BridgeConnector {
    /// `app` est l'application configurée sur `port` : un pont qui en annonce une autre est refusé,
    /// pour qu'un script n'atteigne jamais le mauvais programme.
    #[must_use]
    pub fn new(app: &'static str, port: u16, token: Token, limits: Limits) -> Self {
        BridgeConnector {
            app,
            port,
            token,
            limits,
        }
    }

    #[must_use]
    pub fn app(&self) -> &'static str {
        self.app
    }

    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Ouvre une nouvelle connexion et termine la poignée de main.
    ///
    /// # Erreurs
    /// Voir [`BridgeClient::connect`].
    pub fn connect(&self) -> Result<BridgeClient, BridgeError> {
        BridgeClient::connect(self.port, self.app, &self.token, &self.limits)
    }
}

impl fmt::Debug for BridgeConnector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BridgeConnector")
            .field("app", &self.app)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

struct Connection {
    stream: TcpStream,
    buffer: Vec<u8>,
    max_line_bytes: usize,
    /// L'identifiant d'une requête qui a expiré et dont la réponse n'est pas arrivée.
    late: Option<String>,
}

enum Poll {
    Line(Vec<u8>),
    Nothing,
}

impl Connection {
    /// Sort une ligne complète du tampon, s'il y en a une.
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

    /// Lit plus d'octets ; jamais plus d'une ligne au-delà de la limite, pour qu'une ligne sans fin
    /// ne puisse pas remplir la mémoire.
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

    /// Attend une ligne jusqu'à `deadline`.
    fn read_line(&mut self, deadline: Instant, sent: bool) -> Result<Vec<u8>, BridgeError> {
        loop {
            if let Some(line) = self.take_line()? {
                return Ok(line);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(BridgeError::Timeout);
            }
            // Un délai de lecture nul est rejeté par le système : attendre au moins une milliseconde.
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

    /// Cherche une ligne sans attendre.
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
        // Quoi qu'il se passe à l'écriture, une partie de la requête a pu partir : elle est signalée
        // comme envoyée pour qu'elle ne soit jamais écrite une seconde fois.
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

fn check_hello(object: &Map<String, Value>, expected_app: &str) -> Result<Hello, BridgeError> {
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
    let hello = match (
        text_field(object, "app"),
        text_field(object, "app_version"),
        text_field(object, "bridge_version"),
    ) {
        (Some(app), Some(app_version), Some(bridge_version)) => Hello {
            app,
            app_version,
            bridge_version,
        },
        _ => {
            return Err(BridgeError::Handshake(
                "the hello lacks the application or a version",
            ));
        }
    };
    // Le nom annoncé n'est pas répété : c'est un texte venu de l'autre bout.
    if hello.app != expected_app {
        return Err(BridgeError::Handshake(
            "the bridge announced another application than the one configured on this port",
        ));
    }
    Ok(hello)
}

/// Une connexion authentifiée vers un pont.
///
/// Au plus une requête est en vol : une seconde, ou toute requête tant que la réponse à une
/// requête expirée est attendue, reçoit aussitôt [`BridgeError::Busy`] et rien n'est écrit au pont.
pub struct BridgeClient {
    connection: Mutex<Connection>,
    /// Un second descripteur sur la socket, pour que `close` n'attende jamais une requête.
    control: TcpStream,
    hello: Hello,
    next_id: AtomicU64,
    broken: AtomicBool,
}

impl BridgeClient {
    /// Se connecte au pont qui écoute sur `port` de l'adresse de bouclage, envoie le `hello` avec le
    /// jeton et vérifie la réponse, qui doit annoncer `expected_app`.
    ///
    /// # Erreurs
    /// [`BridgeError::Connect`] quand rien n'écoute, [`BridgeError::Handshake`] pour toute réponse
    /// autre qu'un `hello` valide de `expected_app` (une connexion fermée comprise).
    pub fn connect(
        port: u16,
        expected_app: &str,
        token: &Token,
        limits: &Limits,
    ) -> Result<BridgeClient, BridgeError> {
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
        let hello = check_hello(&parse_object(&answer).map_err(handshake)?, expected_app)?;
        Ok(BridgeClient {
            connection: Mutex::new(connection),
            control,
            hello,
            next_id: AtomicU64::new(1),
            broken: AtomicBool::new(false),
        })
    }

    /// Ce que le pont a annoncé dans son `hello`.
    #[must_use]
    pub fn hello(&self) -> &Hello {
        &self.hello
    }

    /// Faux une fois que la connexion a échoué ou a été fermée ; un nouveau client est nécessaire.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        !self.broken.load(Ordering::SeqCst)
    }

    /// Envoie une requête et attend au plus `timeout` la réponse portant le même identifiant.
    ///
    /// `payload` est un objet JSON sans `id` ; le client l'ajoute.
    ///
    /// # Erreurs
    /// [`BridgeError::Busy`] sans rien envoyer quand une requête est en cours ou qu'une réponse
    /// tardive est encore attendue ; [`BridgeError::Timeout`], après quoi le client reste occupé ;
    /// toute autre variante rend le client inutilisable.
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
                    // La réponse tardive appartient à un appel qui a déjà rendu.
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

    /// Ferme la connexion. Rien n'est envoyé au pont ni à l'application.
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
