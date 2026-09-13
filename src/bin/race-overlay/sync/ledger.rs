// Rust guideline compliant 2026-02-16

//! The append-only event log both sides of team sync hold.
//!
//! The relay keeps one per room so late joiners can be caught up; every
//! client keeps a replica so a reconnect can say exactly what it missed.
//! Same type both places, because "what do I have" and "what does the other
//! side lack" are the same question asked in both directions.
//!
//! A ledger lives as long as its session. The relay limits each producer to
//! 150,000 events, comfortably above a driver's normal 24-hour rate. History
//! stays in memory; a surviving replica can repair a restarted relay, but a
//! simultaneous shutdown of every holder loses it.

use std::collections::BTreeMap;

use super::protocol::{Envelope, ProducerSeq};

/// A single producer's hard event budget for one race. At the driver's
/// maximum normal rate (one scalar per second plus laps and stops), this is
/// comfortably beyond 24 hours. It prevents a corrupt or misbehaving client
/// from making a relay room grow forever.
pub const MAX_EVENTS_PER_PRODUCER: usize = 150_000;

/// Every event seen so far, per producer, in sequence order.
#[derive(Debug, Default)]
pub struct Ledger {
    /// Keyed by producer; each `Vec` holds that producer's events sorted by
    /// `seq`. A `BTreeMap` so iteration — and therefore catch-up — is in a
    /// stable order regardless of join order.
    by_producer: BTreeMap<u32, Vec<Envelope>>,
}

impl Ledger {
    /// Records one event; reports whether it was new.
    ///
    /// Duplicates — same `(producer, seq)` — are dropped, which is what makes
    /// redelivery after a reconnect harmless everywhere. An out-of-order
    /// arrival is inserted in place, so the log stays sorted without
    /// requiring the network to be.
    pub fn insert(&mut self, envelope: Envelope) -> bool {
        let events = self.by_producer.entry(envelope.producer).or_default();
        match events.binary_search_by_key(&envelope.seq, |held| held.seq) {
            Ok(_) => false,
            Err(at) => {
                events.insert(at, envelope);
                true
            }
        }
    }

    /// Whether a relay may accept this envelope without introducing a hole or
    /// exceeding the per-race storage budget. Clients can retain an
    /// out-of-order replica while reconnecting, but a relay sees one ordered
    /// TCP stream per producer and should never create a hole itself: a hole
    /// would make its contiguous catch-up tip permanently misleading.
    #[must_use]
    pub fn accepts_relayed(&self, envelope: &Envelope) -> bool {
        if envelope.seq == 0 {
            return false;
        }
        let Some(events) = self.by_producer.get(&envelope.producer) else {
            return envelope.seq == 1;
        };
        match events.binary_search_by_key(&envelope.seq, |held| held.seq) {
            // Re-offers and reconnect redelivery are harmless.
            Ok(_) => true,
            Err(_) => {
                events.len() < MAX_EVENTS_PER_PRODUCER
                    && events.last().is_some_and(|last| envelope.seq == last.seq.saturating_add(1))
            }
        }
    }

    /// The highest contiguous sequence held per producer.
    ///
    /// Contiguous, not merely highest: a gap means an event is still missing,
    /// and claiming the ones beyond it would stop the other side from ever
    /// resending the hole. Events past a gap are held and simply counted
    /// again once the hole fills.
    #[must_use]
    pub fn tips(&self) -> Vec<ProducerSeq> {
        self.by_producer
            .iter()
            .map(|(&producer, events)| {
                let mut seq = 0;
                for event in events {
                    if event.seq != seq + 1 {
                        break;
                    }
                    seq = event.seq;
                }
                ProducerSeq { producer, seq }
            })
            .collect()
    }

    /// Everything the holder of `have` is missing, oldest first per producer.
    #[must_use]
    pub fn after(&self, have: &[ProducerSeq]) -> Vec<Envelope> {
        let held = |producer: u32| have.iter().find(|tip| tip.producer == producer).map_or(0, |tip| tip.seq);
        self.by_producer
            .iter()
            .flat_map(|(&producer, events)| {
                let from = held(producer);
                events.iter().filter(move |event| event.seq > from).cloned()
            })
            .collect()
    }

    /// The next sequence number a producer should stamp, given what this
    /// ledger already holds for it.
    #[must_use]
    pub fn next_seq(&self, producer: u32) -> u32 {
        self.by_producer.get(&producer).and_then(|events| events.last()).map_or(1, |event| event.seq.saturating_add(1))
    }
}

#[cfg(test)]
mod tests {
    use super::super::protocol::Event;
    use super::*;

    fn envelope(producer: u32, seq: u32) -> Envelope {
        let tally = u16::try_from(seq).unwrap_or(u16::MAX);
        Envelope { producer, seq, session_time: f64::from(seq), event: Event::OffTrack { car_idx: 1, tally } }
    }

    #[test]
    fn duplicates_are_dropped_and_gaps_are_not_claimed() {
        let mut ledger = Ledger::default();
        assert!(ledger.insert(envelope(7, 1)));
        assert!(!ledger.insert(envelope(7, 1)), "redelivery must be harmless");
        assert!(ledger.insert(envelope(7, 3)), "an event past a gap is still kept");

        assert_eq!(ledger.tips(), vec![ProducerSeq { producer: 7, seq: 1 }], "the gap at 2 caps the tip");
        assert!(ledger.insert(envelope(7, 2)));
        assert_eq!(ledger.tips(), vec![ProducerSeq { producer: 7, seq: 3 }], "filling the hole frees the rest");
    }

    #[test]
    fn catch_up_returns_exactly_the_gap() {
        let mut ledger = Ledger::default();
        for seq in 1..=4 {
            ledger.insert(envelope(7, seq));
        }
        ledger.insert(envelope(9, 1));

        let missing = ledger.after(&[ProducerSeq { producer: 7, seq: 2 }]);
        let seqs: Vec<(u32, u32)> = missing.iter().map(|held| (held.producer, held.seq)).collect();
        assert_eq!(seqs, vec![(7, 3), (7, 4), (9, 1)], "past the tip for 7, everything for unseen 9");

        assert!(ledger.after(&ledger.tips()).is_empty(), "a caught-up member gets nothing");
    }

    #[test]
    fn producers_resume_their_numbering_from_the_ledger() {
        let mut ledger = Ledger::default();
        assert_eq!(ledger.next_seq(7), 1, "a fresh producer starts at 1");
        ledger.insert(envelope(7, 1));
        ledger.insert(envelope(7, 2));
        assert_eq!(ledger.next_seq(7), 3);
    }

    #[test]
    fn a_relay_accepts_only_contiguous_nonzero_history() {
        let mut ledger = Ledger::default();
        assert!(!ledger.accepts_relayed(&envelope(7, 0)));
        assert!(!ledger.accepts_relayed(&envelope(7, 2)), "a relay must not create its first gap");
        assert!(ledger.accepts_relayed(&envelope(7, 1)));
        ledger.insert(envelope(7, 1));
        assert!(ledger.accepts_relayed(&envelope(7, 1)), "a reconnect may redeliver");
        assert!(!ledger.accepts_relayed(&envelope(7, 3)), "the missing seq 2 must be repaired first");
        assert!(ledger.accepts_relayed(&envelope(7, 2)));
    }
}
