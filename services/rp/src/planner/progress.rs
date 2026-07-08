//! Per-target / per-filter exposure counters — the state behind the
//! `record_exposure` / `get_session_progress` MCP tools and rp.md
//! §"Dynamic Planner" decision-logic bullets 3–4 plus the
//! exhausted-targets half of bullet 6.
//!
//! The store is plain data with pure methods so `decision::next_target`
//! stays a pure function of its arguments; the shared
//! `Arc<Mutex<SessionProgress>>` lives on `McpHandler` (one per rp
//! process — counters survive across MCP connections and orchestrator
//! re-invocations, and `SessionManager::start` clears them when a
//! fresh session begins). The rp.md §"Session Persistence" state-file
//! write and startup read-back are still unimplemented — see the
//! status callout there.
//!
//! Counters are keyed by target name and *filter key*: the filter
//! name, with `None` / `""` (an unfiltered rig) normalised to the
//! empty string. Goals come from the target's `exposures[]` plan
//! (`count` per entry); the store itself never sees the plan except
//! through the methods that take a [`PlannerTarget`].

use std::collections::HashMap;

use super::decision::{ExposureSpec, PlannerTarget};

/// Normalise an optional filter name to the map key: the unfiltered
/// slot (absent, `null`, or `""` in tool parameters and target plans)
/// is the empty string.
pub fn filter_key(filter: Option<&str>) -> String {
    filter.unwrap_or("").to_string()
}

/// The in-memory `record_exposure` counters plus the last recorded
/// filter (rp.md §"Dynamic Planner" bullet 4's "previous frame").
#[derive(Debug, Default)]
pub struct SessionProgress {
    /// target name → filter key → completed frame count.
    completed: HashMap<String, HashMap<String, u32>>,
    /// Filter key of the most recently recorded frame, `None` before
    /// the first `record_exposure` of the session.
    last_filter_key: Option<String>,
}

impl SessionProgress {
    /// Record one completed frame; returns the updated count for that
    /// (target, filter) slot.
    pub fn record(&mut self, target: &str, filter: Option<&str>) -> u32 {
        let key = filter_key(filter);
        let slot = self
            .completed
            .entry(target.to_string())
            .or_default()
            .entry(key.clone())
            .or_insert(0);
        *slot = slot.saturating_add(1);
        self.last_filter_key = Some(key);
        *slot
    }

    /// Completed frames recorded for a (target, filter) slot.
    pub fn completed_for(&self, target: &str, filter: Option<&str>) -> u32 {
        self.completed
            .get(target)
            .and_then(|by_filter| by_filter.get(&filter_key(filter)))
            .copied()
            .unwrap_or(0)
    }

    /// All filter keys recorded for a target (for the progress views;
    /// plans contribute their own keys separately).
    pub fn recorded_filter_keys(&self, target: &str) -> Vec<&str> {
        self.completed
            .get(target)
            .map(|by_filter| by_filter.keys().map(String::as_str).collect())
            .unwrap_or_default()
    }

    /// Filter key of the most recently recorded frame.
    pub fn last_filter_key(&self) -> Option<&str> {
        self.last_filter_key.as_deref()
    }

    /// Reset every counter — a fresh session is a fresh night.
    pub fn clear(&mut self) {
        self.completed.clear();
        self.last_filter_key = None;
    }

    /// The first `exposures[]` entry that has not met its `count`, in
    /// plan order — the entry `get_next_target` recommends. An entry
    /// without a `count` has no finite goal and is never complete, so
    /// the walk stops there. Duplicate-filter plans share one counter
    /// per filter, so completed frames are allocated to entries in
    /// plan order. `None` means the plan is empty or fully complete.
    pub fn next_incomplete_entry<'a>(&self, target: &'a PlannerTarget) -> Option<&'a ExposureSpec> {
        let mut remaining: HashMap<String, u32> = self
            .completed
            .get(&target.name)
            .cloned()
            .unwrap_or_default();
        for entry in &target.exposures {
            let Some(goal) = entry.count else {
                return Some(entry);
            };
            let have = remaining
                .entry(filter_key(entry.filter.as_deref()))
                .or_insert(0);
            let used = (*have).min(goal);
            *have -= used;
            if used < goal {
                return Some(entry);
            }
        }
        None
    }

    /// Whether every entry of the target's plan has met its `count`.
    /// A target with no plan (or any uncounted entry) has no finite
    /// integration goal and is never exhausted.
    pub fn is_exhausted(&self, target: &PlannerTarget) -> bool {
        !target.exposures.is_empty() && self.next_incomplete_entry(target).is_none()
    }

    /// Completed-to-goal fraction across the target's counted entries
    /// (clamped per entry, so over-recorded frames don't inflate it).
    /// A target with no counted entries reports 0.0 — it has no goal
    /// to progress toward, so bullet 3 keeps preferring it.
    pub fn fraction(&self, target: &PlannerTarget) -> f64 {
        let mut remaining: HashMap<String, u32> = self
            .completed
            .get(&target.name)
            .cloned()
            .unwrap_or_default();
        let mut done: u32 = 0;
        let mut goal_total: u32 = 0;
        for entry in &target.exposures {
            let Some(goal) = entry.count else { continue };
            goal_total = goal_total.saturating_add(goal);
            let have = remaining
                .entry(filter_key(entry.filter.as_deref()))
                .or_insert(0);
            let used = (*have).min(goal);
            *have -= used;
            done = done.saturating_add(used);
        }
        if goal_total == 0 {
            0.0
        } else {
            f64::from(done) / f64::from(goal_total)
        }
    }

    /// The summed `count` of a target's plan entries matching a filter
    /// key — the `goal` field of `record_exposure` /
    /// `get_session_progress`. `None` when the filter is not in the
    /// plan, or when any matching entry is uncounted (no finite goal).
    pub fn goal_for(target: &PlannerTarget, key: &str) -> Option<u32> {
        let mut total: u32 = 0;
        let mut matched = false;
        for entry in &target.exposures {
            if filter_key(entry.filter.as_deref()) != key {
                continue;
            }
            matched = true;
            total = total.saturating_add(entry.count?);
        }
        matched.then_some(total)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    fn target(name: &str, exposures: Vec<ExposureSpec>) -> PlannerTarget {
        PlannerTarget {
            name: name.to_string(),
            ra_hours: 0.0,
            dec_degrees: 0.0,
            min_altitude_degrees: None,
            exposures,
        }
    }

    fn entry(filter: Option<&str>, count: Option<u32>) -> ExposureSpec {
        ExposureSpec {
            filter: filter.map(String::from),
            duration_secs: 60.0,
            count,
        }
    }

    #[test]
    fn filter_key_normalises_the_unfiltered_slot() {
        assert_eq!(filter_key(None), "");
        assert_eq!(filter_key(Some("")), "");
        assert_eq!(filter_key(Some("Red")), "Red");
    }

    #[test]
    fn record_increments_and_reads_back() {
        let mut p = SessionProgress::default();
        assert_eq!(p.completed_for("M31", Some("Red")), 0);
        assert_eq!(p.record("M31", Some("Red")), 1);
        assert_eq!(p.record("M31", Some("Red")), 2);
        assert_eq!(p.record("M31", None), 1);
        assert_eq!(p.completed_for("M31", Some("Red")), 2);
        assert_eq!(
            p.completed_for("M31", Some("")),
            1,
            "None and \"\" share the slot"
        );
        assert_eq!(p.last_filter_key(), Some(""));
    }

    #[test]
    fn clear_resets_counters_and_last_filter() {
        let mut p = SessionProgress::default();
        p.record("M31", Some("Red"));
        p.clear();
        assert_eq!(p.completed_for("M31", Some("Red")), 0);
        assert_eq!(p.last_filter_key(), None);
    }

    #[test]
    fn next_incomplete_entry_walks_the_plan_in_order() {
        let t = target(
            "M31",
            vec![entry(Some("L"), Some(2)), entry(Some("R"), Some(1))],
        );
        let mut p = SessionProgress::default();
        assert_eq!(
            p.next_incomplete_entry(&t).unwrap().filter.as_deref(),
            Some("L")
        );
        p.record("M31", Some("L"));
        assert_eq!(
            p.next_incomplete_entry(&t).unwrap().filter.as_deref(),
            Some("L")
        );
        p.record("M31", Some("L"));
        assert_eq!(
            p.next_incomplete_entry(&t).unwrap().filter.as_deref(),
            Some("R")
        );
        p.record("M31", Some("R"));
        assert!(p.next_incomplete_entry(&t).is_none());
        assert!(p.is_exhausted(&t));
    }

    #[test]
    fn an_uncounted_entry_pins_the_walk_and_blocks_exhaustion() {
        let t = target(
            "M31",
            vec![entry(Some("L"), Some(1)), entry(Some("R"), None)],
        );
        let mut p = SessionProgress::default();
        p.record("M31", Some("L"));
        for _ in 0..3 {
            assert_eq!(
                p.next_incomplete_entry(&t).unwrap().filter.as_deref(),
                Some("R"),
                "an uncounted entry recommends forever"
            );
            p.record("M31", Some("R"));
        }
        assert!(!p.is_exhausted(&t));
    }

    #[test]
    fn duplicate_filter_entries_allocate_completed_frames_in_plan_order() {
        // Two Luminance entries with different durations share the L
        // counter; 3 recorded frames complete the first (goal 2) and
        // half-fill the second.
        let mut first = entry(Some("L"), Some(2));
        first.duration_secs = 300.0;
        let mut second = entry(Some("L"), Some(2));
        second.duration_secs = 60.0;
        let t = target("M31", vec![first, second]);
        let mut p = SessionProgress::default();
        for _ in 0..3 {
            p.record("M31", Some("L"));
        }
        let next = p.next_incomplete_entry(&t).unwrap();
        assert_eq!(next.duration_secs, 60.0, "the first entry is complete");
        assert!(!p.is_exhausted(&t));
        p.record("M31", Some("L"));
        assert!(p.is_exhausted(&t));
    }

    #[test]
    fn a_planless_target_is_never_exhausted() {
        let t = target("M31", Vec::new());
        let mut p = SessionProgress::default();
        p.record("M31", Some("L"));
        assert!(p.next_incomplete_entry(&t).is_none());
        assert!(!p.is_exhausted(&t), "no plan means no goal to meet");
    }

    #[test]
    fn fraction_counts_allocated_frames_against_the_summed_goals() {
        let t = target(
            "M31",
            vec![entry(Some("L"), Some(3)), entry(Some("R"), Some(1))],
        );
        let mut p = SessionProgress::default();
        assert_eq!(p.fraction(&t), 0.0);
        p.record("M31", Some("L"));
        assert!((p.fraction(&t) - 0.25).abs() < 1e-12);
        // Over-recording beyond the goal clamps: 5 L frames still
        // count as 3 of the 4-frame total.
        for _ in 0..4 {
            p.record("M31", Some("L"));
        }
        assert!((p.fraction(&t) - 0.75).abs() < 1e-12);
        // Frames on a filter outside the plan don't move the fraction.
        p.record("M31", Some("Ha"));
        assert!((p.fraction(&t) - 0.75).abs() < 1e-12);
    }

    #[test]
    fn fraction_is_zero_when_the_plan_has_no_counted_entries() {
        let t = target("M31", vec![entry(Some("L"), None)]);
        let mut p = SessionProgress::default();
        p.record("M31", Some("L"));
        assert_eq!(p.fraction(&t), 0.0);
    }

    #[test]
    fn goal_for_sums_matching_counted_entries() {
        let t = target(
            "M31",
            vec![
                entry(Some("L"), Some(2)),
                entry(Some("L"), Some(3)),
                entry(Some("R"), Some(1)),
                entry(None, Some(4)),
            ],
        );
        assert_eq!(SessionProgress::goal_for(&t, "L"), Some(5));
        assert_eq!(SessionProgress::goal_for(&t, "R"), Some(1));
        assert_eq!(SessionProgress::goal_for(&t, ""), Some(4));
        assert_eq!(SessionProgress::goal_for(&t, "Ha"), None, "not in the plan");
    }

    #[test]
    fn goal_for_is_none_when_any_matching_entry_is_uncounted() {
        let t = target(
            "M31",
            vec![entry(Some("L"), Some(2)), entry(Some("L"), None)],
        );
        assert_eq!(
            SessionProgress::goal_for(&t, "L"),
            None,
            "an uncounted entry makes the filter's goal infinite"
        );
    }
}
