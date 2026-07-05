//! Duration-string checking shared by document validation (layer 1) and
//! parameter binding (layer 3).
//!
//! A document duration must satisfy **both**:
//!
//! 1. the surface form published in `schema/workflow-v1.schema.json`
//!    (`^([0-9]+(\.[0-9]+)?(ns|us|µs|ms|s|m|h|d|w|y)\s*)+$`), and
//! 2. `humantime::parse_duration` (the semantic parse).
//!
//! Neither alone is sufficient: humantime is *looser* than the published
//! pattern (it accepts `1day`, `1 h`, `1month`), so checking only
//! humantime would accept documents the published schema rejects —
//! breaking the schema's contract; and the pattern alone would accept
//! out-of-range values only humantime can catch (e.g. an overflowing
//! number of seconds).

use std::time::Duration;

/// Parses a document duration string, enforcing the published surface
/// form. The error is a human-readable message (no position — callers
/// attach the JSON Pointer).
pub(crate) fn parse_duration(s: &str) -> Result<Duration, String> {
    let parsed =
        humantime::parse_duration(s).map_err(|e| format!("`{s}` is not a valid duration: {e}"))?;
    if !surface_ok(s) {
        return Err(format!(
            "`{s}` is not in the document duration format — write durations with \
             the units ns/us/µs/ms/s/m/h/d/w/y and no space before the unit \
             (e.g. \"90m\", \"1h30m\")"
        ));
    }
    Ok(parsed)
}

/// Whether `s` matches the schema's duration pattern:
/// one or more `<digits>[.<digits>]<unit>` components, each optionally
/// followed by whitespace. Longest-match on units so `ms` is not read as
/// `m` + stray `s`.
fn surface_ok(s: &str) -> bool {
    const UNITS: [&str; 10] = ["ns", "us", "µs", "ms", "s", "m", "h", "d", "w", "y"];
    let mut rest = s;
    let mut components = 0usize;
    while !rest.is_empty() {
        let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return false;
        }
        rest = &rest[digits..];
        if let Some(after) = rest.strip_prefix('.') {
            let frac = after.len() - after.trim_start_matches(|c: char| c.is_ascii_digit()).len();
            if frac == 0 {
                return false;
            }
            rest = &after[frac..];
        }
        match UNITS.iter().find(|u| rest.starts_with(**u)) {
            Some(u) => rest = &rest[u.len()..],
            None => return false,
        }
        rest = rest.trim_start();
        components += 1;
    }
    components > 0
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn test_accepts_the_published_surface() {
        for (src, secs) in [
            ("300s", 300.0),
            ("1h30m", 5400.0),
            ("1h 30m", 5400.0),
            ("1.5h", 5400.0),
            ("10ms", 0.010),
            ("5µs", 0.000_005),
            ("1d", 86_400.0),
            ("2w", 1_209_600.0),
        ] {
            assert_eq!(parse_duration(src).unwrap().as_secs_f64(), secs, "{src}");
        }
    }

    #[test]
    fn test_rejects_humantime_extensions_outside_the_published_pattern() {
        // humantime itself accepts all of these; the document format does
        // not, because the published schema pattern must stay authoritative
        // (everything the validator accepts must pass the schema).
        for src in ["1day", "2 days", "1 h", "1min", "1month", "1M", " 1s"] {
            let err = parse_duration(src).unwrap_err();
            assert!(err.contains("document duration format"), "{src}: {err}");
        }
    }

    #[test]
    fn test_rejects_what_humantime_rejects() {
        for src in ["", "90", "1h30", "abc", "99999999999999999999s"] {
            let err = parse_duration(src).unwrap_err();
            assert!(err.contains("not a valid duration"), "{src}: {err}");
        }
    }

    #[test]
    fn test_trailing_whitespace_is_tolerated_between_and_after_components() {
        // The pattern's `\s*` sits after each component: trailing space is
        // fine, leading space is not.
        assert!(parse_duration("1s ").is_ok());
        assert!(parse_duration("1h  30m").is_ok());
        assert!(parse_duration(" 1s").is_err());
    }
}
