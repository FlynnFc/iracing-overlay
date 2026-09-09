// Rust guideline compliant 2026-02-16

//! The team-sync relay: rooms, the shared ledger, catch-up and fanout.
//!
//! Runs inside the hosting overlay (or the `--sync-host` diagnostic) and
//! listens on **localhost only** — the single route in from anywhere, LAN
//! included, is the Tailscale Funnel forwarding to this port, so there is no
//! interface to probe and no TLS to manage here; `tailscaled` terminates it
//! on this same machine. See `plans/team-sync.md`.
//!
//! Threading is one accept loop plus one thread per connection, matching the
//! rest of this app's thread-and-channel style. A connection's WebSocket is
//! owned by its own thread; other threads reach it only through its outbox
//! channel, so there is no socket sharing to get wrong. At a team's scale —
//! a handful of members sending a couple of kilobytes a second — threads are
//! not a cost worth architecture to avoid.

use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tungstenite::{Message, WebSocket};

use super::ledger::Ledger;
use super::protocol::{FromClient, FromRelay, Member, decode, encode};

/// How long a fresh connection gets to say `Hello` before it is dropped.
///
/// A real client sends it immediately; anything that connects and says
/// nothing is a scanner holding a socket open.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// The pump's read timeout: how often a connection thread checks its outbox.
///
/// Also the ceiling on fanout latency, which at 50 ms is far below anything
/// a fuel figure could care about.
const PUMP_INTERVAL: Duration = Duration::from_millis(50);

/// Failed invites allowed per [`FAIL_WINDOW`] before joins are refused blind.
///
/// An 8-character code from a 31-letter alphabet has ~40 bits of entropy;
/// five tries a minute makes guessing it a geological project. Legitimate
/// members are unaffected — a typo is one failure, not five.
const FAIL_LIMIT: usize = 5;

/// See [`FAIL_LIMIT`].
const FAIL_WINDOW: Duration = Duration::from_secs(60);

/// The invite code a host hands to the team.
///
/// Letters that survive being read over voice chat: no `I`/`L`/`O`/`0`/`1`.
/// Verification strips separators and case, so "abcd-efgh" spoken as
/// "ABCDEFGH" still matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteCode(String);

/// See [`InviteCode`].
const INVITE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// Length of the random part, excluding the display hyphen.
const INVITE_LEN: usize = 8;

impl InviteCode {
    /// Draws a fresh code from the operating system's entropy.
    ///
    /// # Errors
    /// Returns an error if the OS random source fails, which is not a
    /// condition to paper over with a weaker fallback.
    pub fn generate() -> anyhow::Result<Self> {
        let mut code = String::with_capacity(INVITE_LEN);
        while code.len() < INVITE_LEN {
            let mut bytes = [0_u8; 16];
            getrandom::fill(&mut bytes).map_err(|err| anyhow::anyhow!("no OS randomness for an invite: {err}"))?;
            // Rejection sampling: a plain modulo would prefer the alphabet's
            // first letters. The rejected fraction is small and the loop
            // draws more bytes if a whole batch is unlucky.
            for byte in bytes {
                let index = usize::from(byte);
                if index < INVITE_ALPHABET.len() * (u8::MAX as usize / INVITE_ALPHABET.len()) {
                    code.push(char::from(INVITE_ALPHABET[index % INVITE_ALPHABET.len()]));
                    if code.len() == INVITE_LEN {
                        break;
                    }
                }
            }
        }
        Ok(Self(code))
    }

    /// Rebuilds a code from its stored or typed text — case and separators
    /// forgiven, the way [`InviteCode::matches`] forgives them on the wire.
    ///
    /// This is how the settings page's saved invite becomes the code the
    /// embedded relay verifies against, so the one string in the config
    /// serves the host's relay and their own client alike.
    ///
    /// # Errors
    /// Refuses text that isn't exactly the code shape once canonicalised, so
    /// a mistyped invite fails loudly when hosting starts rather than
    /// quietly becoming a code nobody can ever match.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let canonical: String =
            text.chars().filter(|c| !c.is_whitespace() && *c != '-').map(|c| c.to_ascii_uppercase()).collect();
        anyhow::ensure!(
            canonical.len() == INVITE_LEN && canonical.bytes().all(|byte| INVITE_ALPHABET.contains(&byte)),
            "an invite code is {INVITE_LEN} letters/digits (no I, L, O, 0 or 1), like ABCD-EFGH"
        );
        Ok(Self(canonical))
    }

    /// Whether `presented` is this code, compared in constant time.
    ///
    /// Constant-time over the canonical bytes so response timing cannot leak
    /// how much of a guess was right. The length check before it leaks only
    /// the code's length, which is public anyway.
    #[must_use]
    pub fn matches(&self, presented: &str) -> bool {
        let canonical: Vec<u8> = presented
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace() && *byte != b'-')
            .map(|byte| byte.to_ascii_uppercase())
            .collect();
        if canonical.len() != self.0.len() {
            return false;
        }
        self.0.bytes().zip(canonical).fold(0_u8, |diff, (ours, theirs)| diff | (ours ^ theirs)) == 0
    }
}

impl std::fmt::Display for InviteCode {
    /// Renders as `XXXX-XXXX`, the shape it is read out to the team in.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (head, tail) = self.0.split_at(INVITE_LEN / 2);
        write!(f, "{head}-{tail}")
    }
}

/// One room: a subsession's ledger and whoever is connected to it.
#[derive(Debug, Default)]
struct Room {
    ledger: Ledger,
    /// Connected members by connection id; the outbox is how any thread
    /// hands a frame to the connection that owns the socket.
    clients: HashMap<u64, RoomSlot>,
}

#[derive(Debug)]
struct RoomSlot {
    member: Member,
    outbox: Sender<FromRelay>,
}

impl Room {
    /// Everyone connected, for `Welcome` and `Roster` frames.
    fn members(&self) -> Vec<Member> {
        self.clients.values().map(|slot| slot.member.clone()).collect()
    }

    /// Queues one frame to every connection except `from`.
    ///
    /// A send only fails when the receiving connection's thread is already
    /// gone, and that thread deregisters itself on the way out — so a failed
    /// send here is a race already being cleaned up, not an error.
    fn fanout(&self, from: u64, frame: &FromRelay) {
        for (&id, slot) in &self.clients {
            if id != from {
                let _ = slot.outbox.send(frame.clone());
            }
        }
    }
}

/// Recent failed invite attempts, shared across every connection.
#[derive(Debug, Default)]
struct FailedJoins(Vec<Instant>);

impl FailedJoins {
    /// Whether a new attempt may even be checked right now.
    fn throttled(&mut self, now: Instant) -> bool {
        self.0.retain(|at| now.duration_since(*at) < FAIL_WINDOW);
        self.0.len() >= FAIL_LIMIT
    }

    fn record(&mut self, now: Instant) {
        self.0.push(now);
    }
}

/// A running relay. Dropping it does not stop the threads; a relay lives as
/// long as the process that hosts it, which is the overlay itself.
#[derive(Debug)]
pub struct Relay {
    local_addr: SocketAddr,
}

impl Relay {
    /// Binds localhost and starts accepting members.
    ///
    /// `port` 0 asks the OS for any free port, which is what the loopback
    /// tests use; the real host picks a fixed one so the funnel command
    /// keeps working across restarts.
    ///
    /// # Errors
    /// Returns an error if the port cannot be bound.
    pub fn spawn(port: u16, invite: InviteCode) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .map_err(|err| anyhow::anyhow!("could not bind 127.0.0.1:{port} for team sync: {err}"))?;
        let local_addr = listener.local_addr()?;
        let invite = Arc::new(invite);
        let rooms: Arc<Mutex<HashMap<u64, Room>>> = Arc::new(Mutex::new(HashMap::new()));
        let failures = Arc::new(Mutex::new(FailedJoins::default()));
        let next_conn_id = Arc::new(AtomicU64::new(1));

        std::thread::spawn(move || {
            // The accept loop and every connection it spawns are socket pumps;
            // keep them off the sim's cores — see `crate::perf`.
            crate::perf::mark_background_thread();
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let invite = Arc::clone(&invite);
                let rooms = Arc::clone(&rooms);
                let failures = Arc::clone(&failures);
                let conn_id = next_conn_id.fetch_add(1, Ordering::Relaxed);
                std::thread::spawn(move || {
                    crate::perf::mark_background_thread();
                    serve_connection(stream, conn_id, &invite, &rooms, &failures);
                });
            }
        });
        Ok(Self { local_addr })
    }

    /// Where the relay is listening, for the host UI and the tests.
    #[must_use]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

/// Runs one member's connection from handshake to disconnect.
fn serve_connection(
    stream: TcpStream,
    conn_id: u64,
    invite: &InviteCode,
    rooms: &Mutex<HashMap<u64, Room>>,
    failures: &Mutex<FailedJoins>,
) {
    // The timeout covers the WebSocket handshake and the Hello both; a
    // connection that stalls in either is dropped when the read times out.
    // The size limits are the memory guard — see `protocol::socket_config`.
    let _ = stream.set_read_timeout(Some(HELLO_TIMEOUT));
    let Ok(mut ws) = tungstenite::accept_with_config(stream, Some(super::protocol::socket_config())) else {
        return;
    };

    let hello = match read_frame(&mut ws) {
        Some(FromClient::Hello { subsession, invite, member, have }) => Some((subsession, invite, member, have)),
        _ => None,
    };
    let Some((subsession, invite_presented, member, have)) = hello else {
        return;
    };

    // The throttle is consulted — and a failure recorded — before the code is
    // compared, so a guessing loop locks itself out rather than getting
    // [`FAIL_LIMIT`] fresh tries against each new connection.
    let accepted = {
        let mut failures = failures.lock().expect("no thread panics while holding the failure log");
        let now = Instant::now();
        if failures.throttled(now) {
            false
        } else {
            let ok = invite.matches(&invite_presented);
            if !ok {
                failures.record(now);
            }
            ok
        }
    };
    if !accepted {
        let refused = FromRelay::Refused { reason: "invite code not accepted; check it and try again".to_owned() };
        let _ = ws.send(Message::Binary(encode(&refused).into()));
        let _ = ws.close(None);
        return;
    }

    // Registration, welcome and backlog under one lock hold, so no event can
    // slip between the backlog being cut and the outbox existing — an event
    // arriving during the hold lands in the outbox and follows the backlog.
    let (outbox_tx, outbox) = std::sync::mpsc::channel();
    let mut rooms_guard = rooms.lock().expect("no thread panics while holding the rooms");
    let room = rooms_guard.entry(subsession).or_default();
    room.clients.insert(conn_id, RoomSlot { member: member.clone(), outbox: outbox_tx.clone() });
    let welcome = FromRelay::Welcome { members: room.members(), have: room.ledger.tips() };
    let backlog = FromRelay::Backlog(room.ledger.after(&have));
    let _ = outbox_tx.send(welcome);
    let _ = outbox_tx.send(backlog);
    room.fanout(conn_id, &FromRelay::Roster(room.members()));
    drop(rooms_guard);
    println!("note: team sync: {} joined subsession {subsession}", member.name);

    let _ = ws.get_ref().set_read_timeout(Some(PUMP_INTERVAL));
    pump(&mut ws, conn_id, subsession, rooms, &outbox);

    let mut rooms = rooms.lock().expect("no thread panics while holding the rooms");
    if let Some(room) = rooms.get_mut(&subsession) {
        room.clients.remove(&conn_id);
        room.fanout(conn_id, &FromRelay::Roster(room.members()));
        // The room itself — and its ledger — outlives its members on
        // purpose: an empty room is a team that all disconnected, and the
        // ledger is exactly what they need back.
    }
    println!("note: team sync: {} left subsession {subsession}", member.name);
}

/// Shuttles frames both ways until the connection dies.
fn pump(
    ws: &mut WebSocket<TcpStream>,
    conn_id: u64,
    subsession: u64,
    rooms: &Mutex<HashMap<u64, Room>>,
    outbox: &Receiver<FromRelay>,
) {
    loop {
        loop {
            match outbox.try_recv() {
                Ok(frame) => {
                    if ws.send(Message::Binary(encode(&frame).into())).is_err() {
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        match ws.read() {
            Ok(Message::Binary(data)) => {
                if let Ok(FromClient::Publish(envelope)) = decode::<FromClient>(&data) {
                    let mut rooms = rooms.lock().expect("no thread panics while holding the rooms");
                    if let Some(room) = rooms.get_mut(&subsession)
                        && room.ledger.insert(envelope.clone())
                    {
                        room.fanout(conn_id, &FromRelay::Relayed(envelope));
                    }
                }
            }
            // Pings are answered by tungstenite itself; the flush in the
            // timeout arm below is what actually puts the pong on the wire
            // when nothing else is being sent.
            Ok(Message::Close(_)) => return,
            Ok(_) => {}
            Err(tungstenite::Error::Io(err))
                if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                let _ = ws.flush();
            }
            Err(_) => return,
        }
    }
}

/// Reads and decodes one client frame, if one arrives in time.
fn read_frame(ws: &mut WebSocket<TcpStream>) -> Option<FromClient> {
    loop {
        match ws.read() {
            Ok(Message::Binary(data)) => return decode(&data).ok(),
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_codes_match_however_the_team_types_them() {
        let code = InviteCode("ABCD2345".to_owned());
        assert!(code.matches("ABCD-2345"));
        assert!(code.matches("abcd2345"));
        assert!(code.matches(" abcd 2345 "));
        assert!(!code.matches("ABCD-2346"));
        assert!(!code.matches("ABCD"));
        assert_eq!(code.to_string(), "ABCD-2345");
    }

    #[test]
    fn a_stored_invite_parses_back_however_it_was_typed() {
        // The settings page saves whatever the host typed; parsing must land
        // on the same canonical code the display form shows.
        let parsed = InviteCode::parse(" abcd-2345 ").expect("a readable code parses");
        assert_eq!(parsed.to_string(), "ABCD-2345");
        assert!(parsed.matches("ABCD2345"));

        InviteCode::parse("").expect_err("empty is not a code");
        InviteCode::parse("ABCD").expect_err("half a code is not a code");
        InviteCode::parse("ABCD-234O").expect_err("O is outside the confusable-free alphabet");
    }

    #[test]
    fn generated_codes_use_the_readable_alphabet() {
        let code = InviteCode::generate().expect("the OS has entropy");
        assert_eq!(code.0.len(), INVITE_LEN);
        assert!(code.0.bytes().all(|byte| INVITE_ALPHABET.contains(&byte)), "unexpected letter in {}", code.0);
        assert!(code.matches(&code.to_string()), "a code must match its own display form");
    }

    #[test]
    fn five_failures_inside_the_window_throttle_the_sixth() {
        let mut failures = FailedJoins::default();
        let start = Instant::now();
        for _ in 0..FAIL_LIMIT {
            assert!(!failures.throttled(start));
            failures.record(start);
        }
        assert!(failures.throttled(start), "the limit itself must throttle");
        assert!(!failures.throttled(start + FAIL_WINDOW), "old failures age out");
    }
}
