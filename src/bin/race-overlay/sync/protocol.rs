// Rust guideline compliant 2026-02-16

//! Wire types for team sync — see `plans/team-sync.md`.
//!
//! Everything crossing the network is one of two enums — [`FromClient`] and
//! [`FromRelay`] — encoded with `postcard` behind a leading version byte.
//! `postcard` is not self-describing, so the version byte is the whole
//! compatibility story: a relay and a client either share a version and
//! understand each other completely, or they refuse each other at the first
//! frame. That is the right trade for a team tool — every member updates from
//! the same repo — and it keeps a fuel figure at 4 bytes instead of a JSON
//! string.

use serde::{Deserialize, Serialize};

/// Bumped whenever any type in this module changes shape.
///
/// Both sides refuse frames from any other version outright; there is no
/// negotiation. Mixed versions inside one team should fail loudly at join
/// time, not quietly corrupt a fuel number mid-race.
///
/// v2 added the crew-chief write event ([`Event::PitWrite`]); v3 added the
/// armed tyre pressures to [`Event::DriverScalars`] and the latched-tyre
/// readings event ([`Event::TyreReadings`]); v4 added the shared fuel target
/// ([`Event::FuelTarget`]); v5 the standing tyre directive
/// ([`Event::TyrePolicySet`]).
pub const PROTOCOL_VERSION: u8 = 5;

/// One member's produced events, identified and ordered.
///
/// `seq` starts at 1 and increments per event from the same producer; the
/// pair `(producer, seq)` names an event globally, which is what makes
/// catch-up ("give me everything after seq N") and de-duplication possible.
/// `session_time` is iRacing's own session clock, shared by every member's
/// sim — consumers order and merge by it, so network latency and arrival
/// order cannot change what anything computes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// iRacing customer id of the member whose overlay produced this.
    pub producer: u32,
    /// 1-based, contiguous per producer.
    pub seq: u32,
    /// iRacing `SessionTime` at the moment the event happened, in seconds.
    pub session_time: f64,
    pub event: Event,
}

/// One measurement worth sharing — see the ledger section of the plan.
///
/// Everything here is either a rare event or the once-a-second scalars tick;
/// nothing is a stream. Phase 2 wires these to the telemetry that produces
/// and consumes them; the variants exist now so the protocol, ledger and
/// relay are exercised end to end by the loopback path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// The seated driver crossed the line: the lap just closed, what the tank
    /// read, and what the lap burned. The authoritative fuel record — the
    /// scalars tick only animates the number between these.
    LapClosed { lap: u16, fuel_litres: f32, used_litres: f32 },
    /// A car (any car) was seen stationary in its stall.
    PitStopObserved { car_idx: u8, stationary_secs: f32, took_tyres: bool },
    /// A car's off-track tally changed.
    OffTrack { car_idx: u8, tally: u16 },
    /// Who is in the team's car changed.
    StintBoundary { driver: String },
    /// The seated driver's player-only scalars, ~1 Hz while anyone listens.
    ///
    /// Carries the sim's armed pit-service state as well as the live tank:
    /// the service state is the echo that closes the crew-chief write loop —
    /// every screen shows what iRacing actually armed, not what anyone asked
    /// for.
    DriverScalars {
        fuel_litres: f32,
        /// Litres armed to add at the next stop; `None` when fuelling is
        /// unticked.
        service_fuel_litres: Option<i16>,
        /// Which corners are armed, in `telemetry::pit::corner_index` order.
        tyres_armed: [bool; 4],
        /// The armed cold pressures per corner, kPa — the base a spectator's
        /// pressure adjustment steps from, so a remote change lands on the
        /// right number rather than on zero.
        tyre_pressures_kpa: [f32; 4],
    },
    /// The seated driver's latched last-stop tyre life — wear, carcass temps,
    /// and the hot pressures the tyres came off at. Changes only at a stop, so
    /// it is sent on change rather than on a clock. This is what a spectator's
    /// Tyres page reads to decide whether to take tyres — see
    /// `plans/team-sync.md`.
    TyreReadings(crate::telemetry::snapshot::TyreInfo),
    /// A crew chief's one-off pit adjustment, applied on the seated driver's
    /// sim behind the consent gate — see `plans/team-sync.md`. Transient: only
    /// acted on live, never replayed from a backlog (a stale write from an
    /// hour ago must not re-arm the box). `requester` names it for the note.
    PitWrite { requester: String, request: crate::telemetry::pit::PitRequest },
    /// The shared fuel-per-lap target a spec sets and the driver's Relative
    /// header chases; `None` clears it. Session state, so it replays into a
    /// late joiner's store. See `plans/strategy-spec-mode.md`.
    FuelTarget { requester: String, litres_per_lap: Option<f32> },
    /// The team's standing tyre directive; `None` clears it back to "the
    /// driver decides". Unlike [`Event::PitWrite`] this is *policy*, not an
    /// action: session state that replays into late joiners and survives
    /// driver swaps, with the driver's overlay deciding each stop against it
    /// as the box approaches. See `plans/strategy-spec-mode.md`.
    TyrePolicySet { requester: String, policy: Option<TyrePolicy> },
}

/// The standing answer to "do we take tyres at the next stop?".
///
/// Set once by a crew chief and applied by the seated driver's overlay at
/// every stop until changed — the "double stint the rest of the race" call
/// made durable, rather than a spectator having to catch every pit entry
/// live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TyrePolicy {
    /// Always arm all four.
    EveryStop,
    /// Never arm tyres — the double-stint call.
    Never,
    /// Take tyres only when the lowest corner's remaining tread is at or
    /// under this percentage; hold onto them otherwise. The user-facing
    /// default is 80.
    BelowWear { threshold_pct: u8 },
}

impl TyrePolicy {
    /// The wear threshold a fresh [`TyrePolicy::BelowWear`] starts at.
    pub const DEFAULT_WEAR_THRESHOLD_PCT: u8 = 80;

    /// Whether this policy arms tyres at the coming stop.
    ///
    /// `lowest_remaining_pct` is the current set's worst reading, from
    /// `TyreInfo::lowest_remaining_pct` — remaining tread, so 100 is fresh.
    /// Under [`TyrePolicy::BelowWear`] a missing reading (no stop latched
    /// yet, so nothing measured) takes tyres: fresh rubber is the mistake
    /// that can't lose a race, worn rubber kept on a guess is.
    #[must_use]
    pub fn tyres_this_stop(self, lowest_remaining_pct: Option<f32>) -> bool {
        match self {
            Self::EveryStop => true,
            Self::Never => false,
            Self::BelowWear { threshold_pct } => {
                lowest_remaining_pct.is_none_or(|lowest| lowest <= f32::from(threshold_pct))
            }
        }
    }
}

/// A member as the relay introduces them to each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// iRacing customer id — also the producer id in every [`Envelope`].
    pub cust_id: u32,
    /// Display name, for "set by <name>" notes.
    pub name: String,
}

/// How far into one producer's sequence a party has seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProducerSeq {
    pub producer: u32,
    /// The highest contiguous `seq` held for that producer.
    pub seq: u32,
}

/// Everything a client ever sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FromClient {
    /// Must be the first frame on a fresh connection, and never repeated.
    Hello {
        /// iRacing `SubSessionID` — the room key. Members of the same
        /// subsession end up in the same room with zero configuration.
        subsession: u64,
        /// The invite code the host handed out, verified in constant time.
        invite: String,
        member: Member,
        /// What this client already holds, so the backlog is only the gap.
        /// Empty on a first join, which makes the gap everything.
        have: Vec<ProducerSeq>,
    },
    Publish(Envelope),
}

/// Everything the relay ever sends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum FromRelay {
    /// The `Hello` was accepted.
    Welcome {
        /// Everyone currently in the room, self included.
        members: Vec<Member>,
        /// The relay's own tips per producer. A reconnecting producer resumes
        /// its sequence from here, so a crash between produce and publish
        /// can't fork the numbering.
        have: Vec<ProducerSeq>,
    },
    /// Everything the `Hello`'s `have` list said was missing, oldest first.
    /// Sent once, straight after `Welcome`; replayed through the same
    /// handlers as live events.
    Backlog(Vec<Envelope>),
    /// One live event from another member.
    Relayed(Envelope),
    /// Someone joined or left; sent to the whole room.
    Roster(Vec<Member>),
    /// The `Hello` was rejected; the connection closes after this.
    Refused { reason: String },
}

/// The largest message either side will accept, in bytes.
///
/// The one big legitimate message is a full-session backlog — a few hundred
/// kilobytes for a 24-hour race at the ledger's rates — so four megabytes is
/// generous headroom while still refusing the 64 MB tungstenite would
/// otherwise buffer for a single hostile frame from inside the room.
const MAX_MESSAGE_BYTES: usize = 4 << 20;

/// The socket limits both the relay and the client run with.
#[must_use]
pub fn socket_config() -> tungstenite::protocol::WebSocketConfig {
    tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES))
}

/// Encodes one message as a version byte followed by its `postcard` body.
///
/// Infallible in practice — these types contain nothing `postcard` can
/// refuse — so a failure is a programming error and panics rather than
/// making every call site thread an error it can do nothing about.
///
/// # Panics
/// Panics if serialization fails, which no value of these types can cause.
#[must_use]
pub fn encode<T: Serialize>(message: &T) -> Vec<u8> {
    let mut frame = vec![PROTOCOL_VERSION];
    frame.extend(postcard::to_stdvec(message).expect("sync protocol types always serialize"));
    frame
}

/// Decodes a frame produced by [`encode`] at the same version.
///
/// # Errors
/// Returns an error naming the mismatch if the frame is empty, from another
/// protocol version, or not a valid body for `T`.
pub fn decode<T: for<'de> Deserialize<'de>>(frame: &[u8]) -> anyhow::Result<T> {
    let (&version, body) = frame.split_first().ok_or_else(|| anyhow::anyhow!("empty sync frame"))?;
    anyhow::ensure!(
        version == PROTOCOL_VERSION,
        "sync protocol version {version} but this build speaks {PROTOCOL_VERSION}; update the older side"
    );
    postcard::from_bytes(body).map_err(|err| anyhow::anyhow!("undecodable sync frame: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_round_trips() {
        let envelope = Envelope {
            producer: 123_456,
            seq: 7,
            session_time: 3210.5,
            event: Event::LapClosed { lap: 42, fuel_litres: 61.2, used_litres: 2.84 },
        };
        let messages = [
            FromRelay::Welcome {
                members: vec![Member { cust_id: 1, name: "Flynn".to_owned() }],
                have: vec![ProducerSeq { producer: 1, seq: 9 }],
            },
            FromRelay::Backlog(vec![envelope.clone()]),
            FromRelay::Relayed(envelope),
            FromRelay::Refused { reason: "bad invite".to_owned() },
        ];
        for message in messages {
            let decoded: FromRelay = decode(&encode(&message)).expect("must round-trip");
            assert_eq!(decoded, message);
        }
    }

    #[test]
    fn a_frame_from_another_version_is_refused_with_advice() {
        let mut frame = encode(&FromClient::Publish(Envelope {
            producer: 1,
            seq: 1,
            session_time: 0.0,
            event: Event::OffTrack { car_idx: 3, tally: 2 },
        }));
        frame[0] = PROTOCOL_VERSION + 1;

        let err = decode::<FromClient>(&frame).expect_err("a version bump must not decode");
        assert!(err.to_string().contains("update"), "the error should say what to do: {err}");
    }

    #[test]
    fn an_empty_frame_is_an_error_not_a_panic() {
        decode::<FromClient>(&[]).expect_err("an empty frame has no version byte");
    }

    #[test]
    fn tyre_events_round_trip_with_their_readings_and_pressures() {
        use crate::telemetry::snapshot::{TyreInfo, TyreState};
        let corners = [
            TyreState { temps_c: [78.0, 82.0, 85.0], wear: [0.95, 0.93, 0.90], pressure_kpa: 168.0 },
            TyreState { temps_c: [79.0, 83.0, 88.0], wear: [0.96, 0.92, 0.88], pressure_kpa: 169.0 },
            TyreState { temps_c: [70.0, 74.0, 77.0], wear: [0.98, 0.97, 0.95], pressure_kpa: 165.0 },
            TyreState { temps_c: [71.0, 75.0, 79.0], wear: [0.97, 0.96, 0.94], pressure_kpa: 166.0 },
        ];
        let readings =
            Envelope { producer: 3, seq: 9, session_time: 900.0, event: Event::TyreReadings(TyreInfo { corners }) };
        assert_eq!(decode::<Envelope>(&encode(&readings)).expect("must round-trip"), readings);

        let scalars = Envelope {
            producer: 3,
            seq: 10,
            session_time: 901.0,
            event: Event::DriverScalars {
                fuel_litres: 33.3,
                service_fuel_litres: Some(28),
                tyres_armed: [true, false, true, false],
                tyre_pressures_kpa: [165.0, 166.0, 167.0, 168.0],
            },
        };
        assert_eq!(decode::<Envelope>(&encode(&scalars)).expect("must round-trip"), scalars);
    }

    #[test]
    fn a_fuel_target_round_trips_set_and_cleared() {
        for litres_per_lap in [Some(2.55_f32), None] {
            let envelope = Envelope {
                producer: 5,
                seq: 2,
                session_time: 60.0,
                event: Event::FuelTarget { requester: "Ben".to_owned(), litres_per_lap },
            };
            assert_eq!(decode::<Envelope>(&encode(&envelope)).expect("must round-trip"), envelope);
        }
    }

    #[test]
    fn the_wear_policy_takes_tyres_at_or_under_the_threshold() {
        let policy = TyrePolicy::BelowWear { threshold_pct: 80 };
        assert!(!policy.tyres_this_stop(Some(92.0)), "barely worn: keep them, this is the double-stint call");
        assert!(policy.tyres_this_stop(Some(80.0)), "at the threshold counts as worn");
        assert!(policy.tyres_this_stop(Some(64.5)));
        assert!(policy.tyres_this_stop(None), "nothing measured yet: fresh rubber is the safe call");
    }

    #[test]
    fn the_fixed_policies_ignore_the_wear_reading() {
        assert!(TyrePolicy::EveryStop.tyres_this_stop(Some(99.0)));
        assert!(TyrePolicy::EveryStop.tyres_this_stop(None));
        assert!(!TyrePolicy::Never.tyres_this_stop(Some(10.0)));
        assert!(!TyrePolicy::Never.tyres_this_stop(None));
    }

    #[test]
    fn a_tyre_policy_round_trips_in_every_shape() {
        for policy in [
            Some(TyrePolicy::EveryStop),
            Some(TyrePolicy::Never),
            Some(TyrePolicy::BelowWear { threshold_pct: TyrePolicy::DEFAULT_WEAR_THRESHOLD_PCT }),
            None,
        ] {
            let envelope = Envelope {
                producer: 5,
                seq: 4,
                session_time: 120.0,
                event: Event::TyrePolicySet { requester: "Ben".to_owned(), policy },
            };
            assert_eq!(decode::<Envelope>(&encode(&envelope)).expect("must round-trip"), envelope);
        }
    }

    #[test]
    fn a_pit_write_round_trips_with_its_request() {
        use crate::telemetry::pit::{Corner, PitRequest};
        // A spread of request shapes, including the per-corner ones, to catch
        // a field dropped or reordered in the wire encoding.
        for request in [
            PitRequest::SetFuel(42),
            PitRequest::ClearFuel,
            PitRequest::SetAllTyres(true),
            PitRequest::SetTyre { corner: Corner::LeftRear, armed: false },
            PitRequest::SetTyrePressure { corner: Corner::RightFront, kpa: 165 },
            PitRequest::SetFastRepair(true),
        ] {
            let envelope = Envelope {
                producer: 7,
                seq: 3,
                session_time: 12.5,
                event: Event::PitWrite { requester: "Flynn".to_owned(), request },
            };
            let decoded: Envelope = decode(&encode(&envelope)).expect("a pit write must round-trip");
            assert_eq!(decoded, envelope);
        }
    }
}
