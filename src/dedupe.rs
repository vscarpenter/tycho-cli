//! Collapsing duplicate streaming records.
//!
//! Claude Code writes multiple records for one API message while streaming
//! (observed worst case: 7). For each identity exactly one record survives:
//! the one with the greatest `output_tokens`, tie-broken by latest
//! timestamp. See `docs/adr/0002-dedup-keep-max-output.md`.

use std::collections::HashMap;

use crate::record::{DedupKey, UsageEvent};

/// Accumulates events across all scanned files, keeping one survivor per
/// [`DedupKey`].
#[derive(Debug, Default)]
pub struct Deduper {
    survivors: HashMap<DedupKey, UsageEvent>,
    collapsed: u64,
}

impl Deduper {
    /// Create an empty deduper.
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold one event in. If its key was already seen, the record with the
    /// greater `output_tokens` wins (latest timestamp on ties) and the
    /// duplicate is counted.
    pub fn insert(&mut self, event: UsageEvent) {
        match self.survivors.entry(event.dedup_key.clone()) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(event);
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                self.collapsed += 1;
                let kept = slot.get();
                let challenger_wins =
                    (event.usage.output, event.timestamp) > (kept.usage.output, kept.timestamp);
                if challenger_wins {
                    slot.insert(event);
                }
            }
        }
    }

    /// How many duplicate records were collapsed (reported by `doctor`).
    pub fn collapsed(&self) -> u64 {
        self.collapsed
    }

    /// Consume the deduper, yielding the surviving events.
    pub fn into_events(self) -> impl Iterator<Item = UsageEvent> {
        self.survivors.into_values()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::Provider;
    use crate::record::TokenUsage;
    use chrono::{TimeZone, Utc};

    fn event(key: DedupKey, output: u64, secs: i64) -> UsageEvent {
        UsageEvent {
            timestamp: Utc.timestamp_opt(secs, 0).unwrap(),
            session_id: Some("sess-1".into()),
            project: String::new(),
            model: "m".into(),
            usage: TokenUsage {
                input: 10,
                output,
                ..TokenUsage::default()
            },
            cost_usd: None,
            cost: rust_decimal::Decimal::ZERO,
            dedup_key: key,
            provider: Provider::Claude,
        }
    }

    fn msg_key(id: &str) -> DedupKey {
        DedupKey::MessageRequest(id.into(), "req_A".into())
    }

    #[test]
    fn distinct_keys_all_survive() {
        let mut deduper = Deduper::new();
        deduper.insert(event(msg_key("a"), 1, 0));
        deduper.insert(event(msg_key("b"), 2, 0));
        deduper.insert(event(DedupKey::Uuid("u".into()), 3, 0));
        assert_eq!(deduper.into_events().count(), 3);
    }

    #[test]
    fn duplicates_collapse_to_the_record_with_max_output_tokens() {
        let mut deduper = Deduper::new();
        deduper.insert(event(msg_key("a"), 100, 0));
        deduper.insert(event(msg_key("a"), 5301, 1));
        deduper.insert(event(msg_key("a"), 900, 2));
        let survivors: Vec<_> = deduper.into_events().collect();
        assert_eq!(survivors.len(), 1);
        assert_eq!(survivors[0].usage.output, 5301);
    }

    #[test]
    fn equal_output_ties_break_to_the_latest_timestamp() {
        let mut deduper = Deduper::new();
        deduper.insert(event(msg_key("a"), 5301, 100));
        deduper.insert(event(msg_key("a"), 5301, 200));
        deduper.insert(event(msg_key("a"), 5301, 150));
        let survivors: Vec<_> = deduper.into_events().collect();
        assert_eq!(survivors[0].timestamp.timestamp(), 200);
    }

    #[test]
    fn collapsed_counts_only_true_duplicates() {
        let mut deduper = Deduper::new();
        for _ in 0..7 {
            deduper.insert(event(msg_key("a"), 5301, 0));
        }
        deduper.insert(event(msg_key("b"), 1, 0));
        assert_eq!(deduper.collapsed(), 6);
    }

    #[test]
    fn same_message_id_under_different_request_ids_is_not_a_duplicate() {
        let mut deduper = Deduper::new();
        deduper.insert(event(
            DedupKey::MessageRequest("msg".into(), "req_1".into()),
            1,
            0,
        ));
        deduper.insert(event(
            DedupKey::MessageRequest("msg".into(), "req_2".into()),
            2,
            0,
        ));
        assert_eq!(deduper.into_events().count(), 2);
    }
}
