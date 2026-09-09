// Rust guideline compliant 2026-02-16

//! The member side of team sync: connect, catch up, publish, reconnect.
//!
//! One background thread owns the socket and a replica [`Ledger`], exactly
//! the way the telemetry thread owns the sim connection: the rest of the app
//! talks to it through two channels. The replica is what makes reconnection
//! honest — the `Hello` after a drop says precisely what this member holds,
//! so the backlog is only the gap, and it is also where this member's own
//! sequence numbers come from, so a reconnect can never fork its numbering.
//!
//! Connections are retried forever with backoff. A dropped link is not an
//! error state to surface and manage; the ledger makes missed time a
//! non-event, so the only job is to be connected again soon.

use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use tungstenite::Message;
use tungstenite::stream::MaybeTlsStream;

use super::ledger::Ledger;
use super::protocol::{Envelope, Event, FromClient, FromRelay, Member, decode, encode};

/// How often the pump checks the publish channel while the socket is quiet.
const PUMP_INTERVAL: Duration = Duration::from_millis(50);

/// Reconnect backoff bounds: patient enough not to hammer a host mid-restart,
/// fast enough that a blip costs seconds of live data (and no ledger data).
const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

/// An event handed to the client thread for stamping and publishing.
///
/// The producer and sequence number are the thread's to assign — call sites
/// know what happened and when in session time, not where the numbering
/// stands after a reconnect.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub session_time: f64,
    pub event: Event,
}

/// A running connection to the team's relay.
///
/// Both channels outlive any number of reconnections. When the receiver
/// reports disconnection the background thread has given up permanently,
/// which it only does if this handle was dropped first.
#[derive(Debug)]
pub struct SyncClient {
    /// Frames from the relay, backlog and live alike, in arrival order.
    pub incoming: Receiver<FromRelay>,
    /// Events this member wants published.
    pub publish: Sender<Outgoing>,
    /// Keeps retries alive only while the owning client still exists.
    _shutdown: Sender<()>,
}

impl SyncClient {
    /// A channel-only client for runtime tests, without a network thread.
    #[cfg(test)]
    pub(super) fn from_test_channels(incoming: Receiver<FromRelay>, publish: Sender<Outgoing>) -> Self {
        let (shutdown, _) = std::sync::mpsc::channel();
        Self { incoming, publish, _shutdown: shutdown }
    }

    /// Starts the background thread and returns its channels.
    ///
    /// `url` is the relay as the member reaches it — `ws://127.0.0.1:port`
    /// for the host's own overlay, the funnel's `wss://…` for everyone else.
    /// Nothing is validated here; a bad URL simply fails every connection
    /// attempt, with a note each time the backoff resets.
    #[must_use]
    pub fn start(url: String, subsession: u64, invite: String, member: Member) -> Self {
        let (incoming_tx, incoming) = std::sync::mpsc::channel();
        let (publish, publish_rx) = std::sync::mpsc::channel();
        let (shutdown_tx, shutdown) = std::sync::mpsc::channel();
        std::thread::spawn(move || run(&url, subsession, &invite, &member, &incoming_tx, &publish_rx, &shutdown));
        Self { incoming, publish, _shutdown: shutdown_tx }
    }
}

/// The connection loop: one iteration per (re)connection.
fn run(
    url: &str,
    subsession: u64,
    invite: &str,
    member: &Member,
    incoming: &Sender<FromRelay>,
    publish: &Receiver<Outgoing>,
    shutdown: &Receiver<()>,
) {
    // Socket pump; keep it off the sim's cores — see `crate::perf`.
    crate::perf::mark_background_thread();
    let mut replica = Ledger::default();
    let mut backoff = BACKOFF_START;
    loop {
        match connect_once(url, subsession, invite, member, &mut replica, incoming, publish) {
            // A session ended cleanly: the app dropped its handles.
            ConnectionEnd::HandlesDropped => return,
            ConnectionEnd::Refused(reason) => {
                println!("note: team sync refused: {reason}");
                return;
            }
            ConnectionEnd::Dropped(reason) => {
                println!("note: team sync disconnected ({reason}); retrying in {backoff:?}");
            }
            ConnectionEnd::NeverConnected(reason) => {
                println!("note: team sync could not connect ({reason}); retrying in {backoff:?}");
            }
        }
        // Connection failures never reach the publish-channel pump. The
        // owning handle must still be able to stop this thread, including
        // while it waits through a long reconnect backoff.
        if shutdown.recv_timeout(backoff) != Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
            return;
        }
        backoff = (backoff * 2).min(BACKOFF_CAP);
    }
}

/// Why one connection attempt is over.
enum ConnectionEnd {
    NeverConnected(String),
    Dropped(String),
    /// The relay rejected the invite; retrying cannot help.
    Refused(String),
    HandlesDropped,
}

/// Dials, joins and pumps one connection until it dies.
fn connect_once(
    url: &str,
    subsession: u64,
    invite: &str,
    member: &Member,
    replica: &mut Ledger,
    incoming: &Sender<FromRelay>,
    publish: &Receiver<Outgoing>,
) -> ConnectionEnd {
    // The size limits mirror the relay's — see `protocol::socket_config`.
    let (mut ws, _response) =
        match tungstenite::client::connect_with_config(url, Some(super::protocol::socket_config()), 3) {
            Ok(connected) => connected,
            Err(err) => return ConnectionEnd::NeverConnected(err.to_string()),
        };
    // The read timeout is what turns the blocking socket into a pump that
    // also drains the publish channel. `MaybeTlsStream` is non-exhaustive;
    // an unknown variant would leave a blocking read, which only stalls
    // publishing until the relay next speaks — degraded, not broken.
    match ws.get_mut() {
        MaybeTlsStream::Plain(stream) => drop(stream.set_read_timeout(Some(PUMP_INTERVAL))),
        MaybeTlsStream::NativeTls(stream) => drop(stream.get_ref().set_read_timeout(Some(PUMP_INTERVAL))),
        _ => {}
    }

    let hello =
        FromClient::Hello { subsession, invite: invite.to_owned(), member: member.clone(), have: replica.tips() };
    if let Err(err) = ws.send(Message::Binary(encode(&hello).into())) {
        return ConnectionEnd::NeverConnected(err.to_string());
    }

    // This member's next sequence number: wherever its own replica ends.
    // The relay's `Welcome` can push it further — a previous run of this
    // process may have published events this one never saw.
    let mut next_seq = replica.next_seq(member.cust_id);
    // A restarted process has no replica yet. Wait for both the relay's
    // contiguous tips and backlog (which can include events past a gap)
    // before assigning identities to queued events.
    let mut welcomed = false;
    let mut caught_up = false;

    loop {
        while caught_up {
            match publish.try_recv() {
                Ok(outgoing) => {
                    let envelope = Envelope {
                        producer: member.cust_id,
                        seq: next_seq,
                        session_time: outgoing.session_time,
                        event: outgoing.event,
                    };
                    next_seq = next_seq.saturating_add(1);
                    // Into the replica first: if the send fails, the event
                    // survives there, and the re-offer pass after the next
                    // `Welcome` delivers it — the relay cannot back-fill
                    // what it never received.
                    replica.insert(envelope.clone());
                    if ws.send(Message::Binary(encode(&FromClient::Publish(envelope)).into())).is_err() {
                        return ConnectionEnd::Dropped("publish failed".to_owned());
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return ConnectionEnd::HandlesDropped,
            }
        }
        match ws.read() {
            Ok(Message::Binary(data)) => {
                let Ok(frame) = decode::<FromRelay>(&data) else {
                    return ConnectionEnd::Dropped("undecodable frame; version mismatch?".to_owned());
                };
                if let FromRelay::Refused { reason } = &frame {
                    // Retrying a refused invite would only feed the relay's
                    // failure throttle; this needs a human to fix the code.
                    let _ = incoming.send(frame.clone());
                    return ConnectionEnd::Refused(reason.clone());
                }
                absorb(&frame, replica, &mut next_seq, member.cust_id);
                match &frame {
                    FromRelay::Welcome { .. } => welcomed = true,
                    FromRelay::Backlog(_) if welcomed => caught_up = true,
                    _ => {}
                }
                // Anything of this member's own that the relay lacks — laps
                // published while the link was down — is re-offered here.
                // The relay cannot back-fill what it never received, so the
                // producer is the only party that can close that gap; room
                // duplicates are dropped by every ledger anyway.
                if let FromRelay::Welcome { have, .. } = &frame {
                    for envelope in replica.after(have) {
                        if envelope.producer == member.cust_id
                            && ws.send(Message::Binary(encode(&FromClient::Publish(envelope)).into())).is_err()
                        {
                            return ConnectionEnd::Dropped("re-offer failed".to_owned());
                        }
                    }
                }
                if incoming.send(frame).is_err() {
                    return ConnectionEnd::HandlesDropped;
                }
            }
            Ok(Message::Close(_)) => return ConnectionEnd::Dropped("closed by relay".to_owned()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(err))
                if err.kind() == std::io::ErrorKind::WouldBlock || err.kind() == std::io::ErrorKind::TimedOut =>
            {
                let _ = ws.flush();
            }
            Err(err) => return ConnectionEnd::Dropped(err.to_string()),
        }
    }
}

/// Feeds one relay frame into the replica and the sequence counter.
fn absorb(frame: &FromRelay, replica: &mut Ledger, next_seq: &mut u32, cust_id: u32) {
    match frame {
        FromRelay::Welcome { have, .. } => {
            // Resume numbering past anything the relay holds from a previous
            // run of this member, or a fork would collide and be dropped as
            // duplicates everywhere.
            if let Some(tip) = have.iter().find(|tip| tip.producer == cust_id) {
                *next_seq = (*next_seq).max(tip.seq.saturating_add(1));
            }
        }
        FromRelay::Backlog(envelopes) => {
            for envelope in envelopes {
                replica.insert(envelope.clone());
            }
            *next_seq = (*next_seq).max(replica.next_seq(cust_id));
        }
        FromRelay::Relayed(envelope) => {
            replica.insert(envelope.clone());
        }
        FromRelay::Roster(_) | FromRelay::Refused { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_handles_stops_retries_when_the_url_cannot_connect() {
        let (incoming_tx, incoming) = std::sync::mpsc::channel();
        let (publish, publish_rx) = std::sync::mpsc::channel();
        let (shutdown_tx, shutdown) = std::sync::mpsc::channel();
        let client = SyncClient { incoming, publish, _shutdown: shutdown_tx };
        drop(client);
        let (finished_tx, finished) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            run(
                "not a websocket URL",
                1,
                "invite",
                &Member { cust_id: 7, name: "Driver".to_owned() },
                &incoming_tx,
                &publish_rx,
                &shutdown,
            );
            finished_tx.send(()).expect("signal worker exit");
        });
        finished.recv_timeout(Duration::from_secs(2)).expect("dropped client must end failed retries");
        worker.join().expect("worker finished");
    }

    #[test]
    fn queued_publish_after_restart_follows_all_existing_producer_history() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind relay");
        let url = format!("ws://{}", listener.local_addr().expect("relay address"));
        let history: Vec<_> = [1, 3]
            .into_iter()
            .map(|seq| Envelope {
                producer: 7,
                seq,
                session_time: f64::from(seq),
                event: Event::OffTrack { car_idx: 1, tally: 1 },
            })
            .collect();
        let old_events = history.clone();
        let relay = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept client");
            stream.set_read_timeout(Some(Duration::from_secs(5))).expect("set timeout");
            let mut ws = tungstenite::accept(stream).expect("handshake");
            let Message::Binary(hello) = ws.read().expect("hello") else { panic!("expected hello") };
            assert!(matches!(decode::<FromClient>(&hello).expect("decode hello"), FromClient::Hello { .. }));
            let mut ledger = Ledger::default();
            for event in history {
                ledger.insert(event);
            }
            for frame in
                [FromRelay::Welcome { members: vec![], have: ledger.tips() }, FromRelay::Backlog(ledger.after(&[]))]
            {
                ws.send(Message::Binary(encode(&frame).into())).expect("send catch-up");
            }
            let Message::Binary(data) = ws.read().expect("queued publish") else { panic!("expected publish") };
            let FromClient::Publish(envelope) = decode(&data).expect("decode publish") else {
                panic!("expected publish")
            };
            let inserted = ledger.insert(envelope.clone());
            ws.close(None).expect("close relay");
            (envelope, inserted)
        });
        let (incoming_tx, _incoming) = std::sync::mpsc::channel();
        let (publish, publish_rx) = std::sync::mpsc::channel();
        let event = Event::OffTrack { car_idx: 1, tally: 9 };
        publish.send(Outgoing { session_time: 10.0, event: event.clone() }).expect("queue before connect");
        let mut replica = Ledger::default();
        let _ = connect_once(
            &url,
            1,
            "invite",
            &Member { cust_id: 7, name: "Driver".to_owned() },
            &mut replica,
            &incoming_tx,
            &publish_rx,
        );
        let (published, inserted) = relay.join().expect("relay finished");
        assert_eq!(published.seq, 4, "must follow backlog events beyond the contiguous tip");
        assert_eq!(published.event, event);
        assert!(inserted, "the relay must retain the queued event rather than deduplicating it");
        let mut expected = old_events;
        expected.push(published);
        assert_eq!(replica.after(&[]), expected, "catch-up must preserve both old and new events");
    }
}
