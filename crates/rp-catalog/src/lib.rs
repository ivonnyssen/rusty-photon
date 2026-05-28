#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Embedded Messier + NGC + IC deep-sky object catalog with
//! case- and whitespace-insensitive name resolution.
//!
//! Data is sourced from the OpenNGC project (CC-BY-SA-4.0; see
//! `src/data/LICENSE-DATA`) and pre-converted into per-catalog CSVs by
//! `scripts/openngc_to_catalog.py`. The CSVs are embedded via
//! `include_str!` and parsed once at first call to [`Catalog::embedded`].

#![deny(unsafe_code)]

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use tracing::{debug, error};

const MESSIER_CSV: &str = include_str!("data/messier.csv");
const NGC_CSV: &str = include_str!("data/ngc.csv");
const IC_CSV: &str = include_str!("data/ic.csv");
const ALIASES_CSV: &str = include_str!("data/aliases.csv");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedTarget {
    /// Canonical name as it appears in the source CSV
    /// (e.g. `"M 31"`, `"NGC 224"`, `"IC 1396"`).
    pub name: String,
    /// OpenNGC type code (`G` = galaxy, `OCl` = open cluster, `GCl`
    /// = globular, `Neb` = nebula, etc.). Documented at
    /// <https://github.com/mattiaverga/OpenNGC>.
    pub object_type: String,
    pub ra_hours: f64,
    pub dec_degrees: f64,
    /// V-Mag from OpenNGC, falling back to B-Mag when V is missing.
    /// `None` if the source row lacks both.
    pub magnitude: Option<f64>,
    /// Major axis in arcmin (OpenNGC `MajAx`). `None` for stellar /
    /// point-source entries.
    pub size_arcmin: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("failed to parse {file}: {source}")]
    CsvParse {
        file: &'static str,
        #[source]
        source: csv::Error,
    },
}

#[derive(Debug, Deserialize)]
struct CsvRow {
    name: String,
    #[serde(rename = "type")]
    object_type: String,
    ra_hours: f64,
    dec_degrees: f64,
    magnitude: String,
    size_arcmin: String,
}

#[derive(Debug, Deserialize)]
struct AliasRow {
    alias: String,
    canonical_name: String,
}

/// Look-up structure built once from the four embedded CSVs.
#[derive(Debug)]
pub struct Catalog {
    by_normalized_name: HashMap<String, ResolvedTarget>,
}

/// Normalize a query string for lookup: strip whitespace, lower-case,
/// and rewrite a leading `messier` prefix to `m` so `"Messier 41"`,
/// `"M 41"`, `"m41"`, and `"MESSIER41"` all collide on the same key.
fn normalize(name: &str) -> String {
    let buf: String = name
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    // Rewrite "messier" → "m" only when followed by a digit (or end);
    // otherwise we'd corrupt things like "messierr" or hypothetical
    // catalog names that happen to share the prefix.
    if let Some(rest) = buf.strip_prefix("messier") {
        if rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_digit()) {
            return format!("m{rest}");
        }
    }
    buf
}

impl Catalog {
    /// Process-wide singleton catalog, lazily initialized on first call.
    /// The embedded data is committed and proven-parseable by the
    /// `loads_with_expected_size` test, so the error branch below is
    /// defensive: it logs the parse failure and yields an empty catalog
    /// (every `resolve()` returns `NotFound`) rather than panicking at
    /// first use of the service.
    pub fn embedded() -> &'static Catalog {
        static SINGLETON: OnceLock<Catalog> = OnceLock::new();
        SINGLETON.get_or_init(|| {
            Self::load_embedded().unwrap_or_else(|e| {
                error!("embedded catalog failed to parse: {e}; serving empty catalog");
                Catalog {
                    by_normalized_name: HashMap::new(),
                }
            })
        })
    }

    /// Build a catalog from the embedded CSVs without using the
    /// process-wide singleton. Useful for tests that want to construct
    /// independent instances.
    pub fn load_embedded() -> Result<Self, CatalogError> {
        let mut by_normalized_name: HashMap<String, ResolvedTarget> =
            HashMap::with_capacity(15_000);

        for (label, body) in [
            ("messier.csv", MESSIER_CSV),
            ("ngc.csv", NGC_CSV),
            ("ic.csv", IC_CSV),
        ] {
            let mut rdr = csv::Reader::from_reader(body.as_bytes());
            for record in rdr.deserialize::<CsvRow>() {
                let r = record.map_err(|e| CatalogError::CsvParse {
                    file: label,
                    source: e,
                })?;
                let target = ResolvedTarget {
                    name: r.name.clone(),
                    object_type: r.object_type,
                    ra_hours: r.ra_hours,
                    dec_degrees: r.dec_degrees,
                    magnitude: r.magnitude.trim().parse().ok(),
                    size_arcmin: r.size_arcmin.trim().parse().ok(),
                };
                by_normalized_name.insert(normalize(&r.name), target);
            }
        }

        // Aliases: human-readable names → canonical NGC/IC entries.
        // We collect first, then resolve in a second pass — order of
        // appearance in the file should not matter.
        let mut alias_rdr = csv::Reader::from_reader(ALIASES_CSV.as_bytes());
        let mut alias_pairs: Vec<(String, String)> = Vec::new();
        for record in alias_rdr.deserialize::<AliasRow>() {
            let r = record.map_err(|e| CatalogError::CsvParse {
                file: "aliases.csv",
                source: e,
            })?;
            alias_pairs.push((normalize(&r.alias), normalize(&r.canonical_name)));
        }
        // Track aliases we've already inserted from aliases.csv so a
        // genuinely ambiguous common name (e.g. "Antennae Galaxies"
        // covers a pair of interacting NGCs) doesn't silently
        // overwrite the first-seen target. First-wins, with a debug
        // log for forensic audits — predictable behaviour beats
        // alphabetical lottery.
        let mut alias_origins: HashMap<String, String> = HashMap::new();
        for (alias_key, canon_key) in alias_pairs {
            if by_normalized_name.contains_key(&alias_key) {
                // alias collides with a first-class catalogue entry
                // (M / NGC / IC); the canonical row always wins.
                continue;
            }
            if let Some(prior) = alias_origins.get(&alias_key) {
                if prior != &canon_key {
                    debug!(
                        alias = %alias_key,
                        first = %prior,
                        skipped = %canon_key,
                        "ambiguous common-name alias maps to multiple targets; keeping first"
                    );
                }
                continue;
            }
            if let Some(target) = by_normalized_name.get(&canon_key).cloned() {
                by_normalized_name.insert(alias_key.clone(), target);
                alias_origins.insert(alias_key, canon_key);
            }
        }

        Ok(Self { by_normalized_name })
    }

    /// Resolve a name to a target. Case- and whitespace-insensitive;
    /// `"M 41"`, `"M41"`, `"m 41"`, and `"Messier 41"` are equivalent.
    /// Common-name aliases (`"Andromeda Galaxy"` → `NGC 224`) are
    /// honoured.
    pub fn resolve(&self, name: &str) -> Option<&ResolvedTarget> {
        self.by_normalized_name.get(&normalize(name))
    }

    pub fn len(&self) -> usize {
        self.by_normalized_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_normalized_name.is_empty()
    }

    /// Up to `limit` canonical names with the smallest Levenshtein
    /// distance from the query (≤ 3). Used to populate "did you
    /// mean…?" suggestions on a lookup miss in the MCP wrapper.
    /// Each returned string is the human-facing canonical name
    /// (e.g. `"M 41"`, `"NGC 224"`) — never the internal lookup key
    /// or an alias. Multiple lookup keys mapping to the same
    /// canonical name (the alias case) are deduped so suggestions
    /// don't fill up with the same target under different spellings.
    pub fn fuzzy_suggestions(&self, query: &str, limit: usize) -> Vec<String> {
        let q = normalize(query);
        let mut scored: Vec<(usize, &str)> = self
            .by_normalized_name
            .iter()
            .map(|(k, t)| (levenshtein(k, &q, 4), t.name.as_str()))
            .filter(|(d, _)| *d <= 3)
            .collect();
        scored.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
        let mut out: Vec<String> = Vec::with_capacity(limit);
        for (_, name) in scored {
            let s = name.to_string();
            if !out.contains(&s) {
                out.push(s);
                if out.len() == limit {
                    break;
                }
            }
        }
        out
    }
}

/// Truncated Levenshtein distance: returns `cap` if the true distance
/// is `≥ cap`. Cheap enough for a one-shot suggestion list across the
/// full catalog (~14k entries).
fn levenshtein(a: &str, b: &str, cap: usize) -> usize {
    if a.len().abs_diff(b.len()) > cap {
        return cap;
    }
    let n = b.len();
    if a.is_empty() {
        return n.min(cap);
    }
    if b.is_empty() {
        return a.len().min(cap);
    }
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];
    for (i, ca) in a.bytes().enumerate() {
        curr[0] = i + 1;
        let mut row_min = curr[0];
        for (j, cb) in b.bytes().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (curr[j] + 1).min(prev[j + 1] + 1).min(prev[j] + cost);
            row_min = row_min.min(curr[j + 1]);
        }
        if row_min >= cap {
            return cap;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[n].min(cap)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    fn cat() -> &'static Catalog {
        Catalog::embedded()
    }

    #[test]
    fn loads_with_expected_size() {
        let c = cat();
        // ~13.7k canonical entries plus 100+ aliases. Lower bound: M+NGC+IC
        // canonicals only.
        assert!(
            c.len() > 13_500,
            "catalog smaller than expected: {}",
            c.len()
        );
    }

    #[test]
    fn resolves_messier_objects_with_canonical_format() {
        let m31 = cat().resolve("M 31").expect("M 31 must resolve");
        assert_eq!(m31.name, "M 31");
        assert!(m31.object_type.starts_with('G')); // galaxy
        assert!((m31.ra_hours - 0.7123).abs() < 0.001);
        assert!((m31.dec_degrees - 41.269).abs() < 0.001);
    }

    #[test]
    fn resolves_messier_with_alternate_spellings() {
        let canon = cat().resolve("M 41").unwrap();
        for variant in ["m41", "M41", "m 41", "M  41", "Messier 41", "messier 41"] {
            let v = cat().resolve(variant).unwrap_or_else(|| {
                panic!("variant {:?} did not resolve", variant);
            });
            assert_eq!(
                v, canon,
                "variant {:?} resolved to a different target",
                variant
            );
        }
    }

    #[test]
    fn ngc_alias_resolves_same_object_as_messier_for_orion_nebula() {
        let m42 = cat().resolve("M 42").unwrap();
        let ngc = cat().resolve("NGC 1976").unwrap();
        assert!((m42.ra_hours - ngc.ra_hours).abs() < 1e-4);
        assert!((m42.dec_degrees - ngc.dec_degrees).abs() < 1e-4);
    }

    #[test]
    fn ngc_resolution_is_whitespace_insensitive() {
        let by_canon = cat().resolve("NGC 224").unwrap();
        let by_squashed = cat().resolve("ngc224").unwrap();
        assert_eq!(by_canon, by_squashed);
    }

    #[test]
    fn ic_resolves() {
        let ic1396 = cat().resolve("IC 1396").expect("IC 1396 must resolve");
        assert_eq!(ic1396.name, "IC 1396");
        // Cepheus, ~21.6h RA, ~+57.5° Dec
        assert!((ic1396.ra_hours - 21.6).abs() < 0.5);
        assert!((ic1396.dec_degrees - 57.5).abs() < 1.0);
    }

    #[test]
    fn common_name_alias_resolves_to_canonical_ngc() {
        let andromeda = cat()
            .resolve("Andromeda Galaxy")
            .expect("alias must resolve");
        // openNGC maps "Andromeda Galaxy" to NGC 224 (= M31).
        assert_eq!(andromeda.name, "NGC 224");
        let crab = cat().resolve("Crab Nebula").expect("alias must resolve");
        assert_eq!(crab.name, "NGC 1952");
    }

    #[test]
    fn missing_object_returns_none() {
        assert!(cat().resolve("M 999").is_none());
        assert!(cat().resolve("not a thing at all").is_none());
    }

    #[test]
    fn fuzzy_suggestions_finds_close_neighbours() {
        let suggestions = cat().fuzzy_suggestions("M 41", 5);
        assert!(
            suggestions.iter().any(|s| s == "M 41"),
            "exact match should appear in fuzzy list as canonical name: {:?}",
            suggestions
        );
        let typo = cat().fuzzy_suggestions("M 411", 5);
        assert!(
            typo.iter().any(|s| s == "M 41"),
            "typo M 411 should suggest M 41 by canonical name: {:?}",
            typo
        );
    }

    #[test]
    fn fuzzy_suggestions_dedup_canonical_names() {
        // "Andromeda Galaxy" is an alias of NGC 224; a query close to
        // it should yield the canonical "NGC 224" once, not twice.
        let suggestions = cat().fuzzy_suggestions("NGC 224", 10);
        let copies = suggestions.iter().filter(|s| *s == "NGC 224").count();
        assert_eq!(
            copies, 1,
            "expected canonical name once, got {copies}: {suggestions:?}"
        );
    }

    #[test]
    fn no_panics_on_empty_or_garbage_query() {
        assert!(cat().resolve("").is_none());
        assert!(cat().resolve("   \t  ").is_none());
        assert!(cat().resolve("!!!").is_none());
    }

    #[test]
    fn levenshtein_capped() {
        assert_eq!(levenshtein("kitten", "sitting", 100), 3);
        assert_eq!(levenshtein("kitten", "sitting", 2), 2);
        assert_eq!(levenshtein("", "abc", 4), 3);
        assert_eq!(levenshtein("abc", "", 4), 3);
        assert_eq!(levenshtein("abc", "abc", 4), 0);
    }

    #[test]
    fn normalize_handles_messier_prefix_variants() {
        assert_eq!(normalize("Messier 41"), "m41");
        assert_eq!(normalize("MESSIER41"), "m41");
        assert_eq!(normalize("M 41"), "m41");
        assert_eq!(normalize("m41"), "m41");
        // Don't rewrite mid-string occurrences.
        assert_eq!(normalize("messieRR"), "messierr");
    }
}
