#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
//! Embedded deep-sky + star catalog with case-, whitespace-, and
//! dash-insensitive name resolution and a class-tiered nearest-neighbor
//! query for planetarium-import naming.
//!
//! One logical catalog spans two entry classes:
//!
//! - **Deep-sky objects** (~19k): Messier + NGC + IC (`OpenNGC`) plus the
//!   astrophoto catalogs — Sharpless, Abell planetaries, vdB, RCW, Gum,
//!   Cederblad, Barnard, LDN, LBN, Arp, Hickson, Collinder, Melotte,
//!   Stock, Trumpler (SIMBAD/VizieR; see `src/data/LICENSE-DATA`).
//! - **Stars** (~354k): every HD/HDE/HDEC designation in the Tycho-2/HD
//!   cross-index (`VizieR` IV/25), with J2000 Tycho-2-derived positions.
//!   The ~400 stars carrying IAU proper names are canonical under that
//!   name (`"Vega"`); their HD designation resolves to the same entry.
//!
//! All data lives in one packed little-endian blob
//! (`src/data/catalog.bin`, format `RPCAT001`, produced by
//! `scripts/pack_catalog.py` from the committed per-catalog CSVs and
//! the fetched star layer) embedded via `include_bytes!` and read
//! zero-copy: lookups binary-search the blob in place, star names are
//! formatted on demand from the HD number, and the only per-call heap
//! use is the returned [`ResolvedTarget`]. The
//! `embedded_matches_committed_csvs` test locks the blob to its CSV
//! sources so the two cannot drift.

#![deny(unsafe_code)]

use std::cmp::Ordering;
use std::sync::OnceLock;

use rp_vocabulary::IcrsCoord;
use serde::{Deserialize, Serialize};
use tracing::error;

const CATALOG_BIN: &[u8] = include_bytes!("data/catalog.bin");
const MAGIC: &[u8; 8] = b"RPCAT001";
const HEADER_LEN: usize = 8 + 6 * 4;
/// Sentinel for "no magnitude" in the packed centi-magnitude columns.
const MAG_NONE: i16 = i16::MIN;
/// Bit 31 of a key-table row reference marks a star row.
const STAR_BIT: u32 = 0x8000_0000;
const MAS_PER_DEGREE: f64 = 3_600_000.0;
const ARCMIN_PER_DEGREE: f64 = 60.0;

/// Which class of catalog entry a [`ResolvedTarget`] came from. The
/// planetarium-import naming contract treats the classes asymmetrically:
/// a deep-sky hit outranks any star hit regardless of separation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectClass {
    DeepSky,
    Star,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedTarget {
    /// Canonical name as packed (e.g. `"M 31"`, `"Sh2-101"`,
    /// `"HD 227018"`, `"Vega"`).
    pub name: String,
    /// `OpenNGC` type code (`G` = galaxy, `OCl` = open cluster, `HII`,
    /// `DrkN`, etc.; documented at
    /// <https://github.com/mattiaverga/OpenNGC>). Stars are `"*"`.
    pub object_type: String,
    /// J2000/ICRS pointing, validated by construction (ADR-019): one
    /// coordinate type spans catalog → store → planner, serialized as a
    /// nested `"coord": { "ra_hours", "dec_degrees" }` object (no
    /// `#[serde(flatten)]`).
    pub coord: IcrsCoord,
    /// V magnitude (or B fallback for `OpenNGC` rows; VT-derived V for
    /// stars). `None` if the source lacks one.
    pub magnitude: Option<f64>,
    /// Major axis in arcmin. `None` for stars and point sources.
    pub size_arcmin: Option<f64>,
    /// Entry class; see [`ObjectClass`].
    pub class: ObjectClass,
}

/// Per-class acceptance radii for [`Catalog::nearest`], wired from
/// `target_store.import.naming_tolerance_arcmin` /
/// `star_naming_tolerance_arcmin` in rp.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NearestTolerances {
    pub dso_arcmin: f64,
    pub star_arcmin: f64,
}

/// A [`Catalog::nearest`] hit. Offsets are of the query **from** the
/// catalog centroid: East = Δα·cos δ, North = Δδ.
#[derive(Debug, Clone, PartialEq)]
pub struct NearestMatch {
    pub target: ResolvedTarget,
    pub separation_arcmin: f64,
    pub east_offset_arcmin: f64,
    pub north_offset_arcmin: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("catalog blob has wrong magic (expected RPCAT001)")]
    BadMagic,
    #[error("catalog blob truncated: {actual} bytes, layout needs {expected}")]
    Truncated { expected: usize, actual: usize },
}

/// Zero-copy view over the packed catalog blob. All section offsets are
/// validated once at construction; accessors after that are defensive
/// (a logic error yields a zero/empty value, never a panic).
#[derive(Debug)]
pub struct Catalog {
    bytes: &'static [u8],
    dso_count: usize,
    star_count: usize,
    key_count: usize,
    named_count: usize,
    type_count: usize,
    // Section start offsets into `bytes`.
    dso_dec: usize,
    dso_ra: usize,
    dso_mag: usize,
    dso_size: usize,
    dso_type: usize,
    dso_cat: usize,
    dso_name: usize,
    star_dec: usize,
    star_ra: usize,
    star_hd: usize,
    star_mag: usize,
    keys: usize,
    named: usize,
    types: usize,
    pool: usize,
}

/// Normalize a query string for lookup: strip whitespace and dashes,
/// ASCII-lowercase, then rewrite well-known catalog prefixes onto the
/// canonical short form when followed by a digit — so `"Messier 41"`,
/// `"Sharpless 2-101"`, `"Barnard 33"`, `"HDE 227018"` collide with
/// `"M 41"`, `"Sh2-101"`, `"B 33"`, `"HD 227018"`.
///
/// MUST stay byte-identical to `normalize()` in
/// `scripts/pack_catalog.py`, which produces the packed key table this
/// function is matched against.
fn normalize(name: &str) -> String {
    let buf: String = name
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    const REWRITES: [(&str, &str); 8] = [
        ("sharpless2", "sh2"),
        ("sharpless", "sh2"),
        ("messier", "m"),
        ("barnard", "b"),
        ("collinder", "cr"),
        ("melotte", "mel"),
        ("trumpler", "tr"),
        ("hde", "hd"),
    ];
    for (prefix, replacement) in REWRITES {
        if let Some(rest) = buf.strip_prefix(prefix) {
            if rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_digit()) {
                return format!("{replacement}{rest}");
            }
            break;
        }
    }
    buf
}

fn read_u32(bytes: &[u8], off: usize) -> u32 {
    match bytes.get(off..off + 4) {
        Some(b) => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        None => 0,
    }
}

fn read_i32(bytes: &[u8], off: usize) -> i32 {
    read_u32(bytes, off).cast_signed()
}

fn read_i16(bytes: &[u8], off: usize) -> i16 {
    match bytes.get(off..off + 2) {
        Some(b) => i16::from_le_bytes([b[0], b[1]]),
        None => 0,
    }
}

fn read_u16(bytes: &[u8], off: usize) -> u16 {
    read_i16(bytes, off).cast_unsigned()
}

/// Great-circle separation between two points given in degrees, via the
/// haversine form (stable at small angles, exact at the poles).
fn separation_arcmin(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let (ra1, dec1, ra2, dec2) = (
        ra1.to_radians(),
        dec1.to_radians(),
        ra2.to_radians(),
        dec2.to_radians(),
    );
    let sin_ddec = ((dec2 - dec1) / 2.0).sin();
    let sin_dra = ((ra2 - ra1) / 2.0).sin();
    let h = sin_ddec * sin_ddec + dec1.cos() * dec2.cos() * sin_dra * sin_dra;
    2.0 * h.sqrt().asin().to_degrees() * ARCMIN_PER_DEGREE
}

/// Δα wrapped to ±180°, in degrees.
fn ra_delta_degrees(from: f64, to: f64) -> f64 {
    let mut d = to - from;
    if d > 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    d
}

impl Catalog {
    /// Process-wide singleton catalog, lazily initialized on first call.
    /// The embedded blob is committed and proven valid by the unit
    /// tests, so the error branch is defensive: it logs the failure and
    /// yields an empty catalog (every lookup misses) rather than
    /// panicking at first use of the service.
    pub fn embedded() -> &'static Self {
        static SINGLETON: OnceLock<Catalog> = OnceLock::new();
        SINGLETON.get_or_init(|| {
            Self::load_embedded().unwrap_or_else(|e| {
                error!("embedded catalog failed to validate: {e}; serving empty catalog");
                Self::empty()
            })
        })
    }

    /// Validate the embedded blob and build a view over it, without the
    /// process-wide singleton. Useful for tests that want independent
    /// instances.
    pub fn load_embedded() -> Result<Self, CatalogError> {
        Self::load(CATALOG_BIN)
    }

    const fn empty() -> Self {
        // An all-zero header describes a valid catalog with no rows;
        // every section is empty and every lookup misses.
        Self::layout(&[], 0, 0, 0, 0, 0, 0)
    }

    fn load(bytes: &'static [u8]) -> Result<Self, CatalogError> {
        if bytes.len() < HEADER_LEN || &bytes[..8] != MAGIC {
            return Err(CatalogError::BadMagic);
        }
        let dso = read_u32(bytes, 8) as usize;
        let star = read_u32(bytes, 12) as usize;
        let keys = read_u32(bytes, 16) as usize;
        let named = read_u32(bytes, 20) as usize;
        let types = read_u32(bytes, 24) as usize;
        let pool = read_u32(bytes, 28) as usize;
        let catalog = Self::layout(bytes, dso, star, keys, named, types, pool);
        let expected = catalog.pool + pool;
        if bytes.len() != expected {
            return Err(CatalogError::Truncated {
                expected,
                actual: bytes.len(),
            });
        }
        Ok(catalog)
    }

    #[allow(clippy::too_many_arguments)]
    const fn layout(
        bytes: &'static [u8],
        dso_count: usize,
        star_count: usize,
        key_count: usize,
        named_count: usize,
        type_count: usize,
        _pool_len: usize,
    ) -> Self {
        let dso_dec = HEADER_LEN;
        let dso_ra = dso_dec + 4 * dso_count;
        let dso_mag = dso_ra + 4 * dso_count;
        let dso_size = dso_mag + 2 * dso_count;
        let dso_type = dso_size + 2 * dso_count;
        let dso_cat = dso_type + dso_count;
        let dso_name = dso_cat + dso_count;
        let star_dec = dso_name + 4 * dso_count;
        let star_ra = star_dec + 4 * star_count;
        let star_hd = star_ra + 4 * star_count;
        let star_mag = star_hd + 4 * star_count;
        let keys = star_mag + 2 * star_count;
        let named = keys + 8 * key_count;
        let types = named + 8 * named_count;
        let pool = types + 4 * type_count;
        Self {
            bytes,
            dso_count,
            star_count,
            key_count,
            named_count,
            type_count,
            dso_dec,
            dso_ra,
            dso_mag,
            dso_size,
            dso_type,
            dso_cat,
            dso_name,
            star_dec,
            star_ra,
            star_hd,
            star_mag,
            keys,
            named,
            types,
            pool,
        }
    }

    fn pool_str(&self, off: usize) -> &str {
        let Some(&len) = self.bytes.get(self.pool + off) else {
            return "";
        };
        let start = self.pool + off + 1;
        self.bytes
            .get(start..start + len as usize)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or_default()
    }

    fn dso_coord_degrees(&self, idx: usize) -> (f64, f64) {
        (
            f64::from(read_u32(self.bytes, self.dso_ra + 4 * idx)) / MAS_PER_DEGREE,
            f64::from(read_i32(self.bytes, self.dso_dec + 4 * idx)) / MAS_PER_DEGREE,
        )
    }

    fn star_coord_degrees(&self, idx: usize) -> (f64, f64) {
        (
            f64::from(read_u32(self.bytes, self.star_ra + 4 * idx)) / MAS_PER_DEGREE,
            f64::from(read_i32(self.bytes, self.star_dec + 4 * idx)) / MAS_PER_DEGREE,
        )
    }

    fn dso_name(&self, idx: usize) -> &str {
        self.pool_str(read_u32(self.bytes, self.dso_name + 4 * idx) as usize)
    }

    fn dso_rank(&self, idx: usize) -> u8 {
        self.bytes.get(self.dso_cat + idx).copied().unwrap_or(0)
    }

    fn star_hd(&self, idx: usize) -> u32 {
        read_u32(self.bytes, self.star_hd + 4 * idx)
    }

    /// Proper name for an HD number, if that star is IAU-named.
    fn proper_name(&self, hd: u32) -> Option<&str> {
        let mut lo = 0usize;
        let mut hi = self.named_count;
        while lo < hi {
            let mid = usize::midpoint(lo, hi);
            let entry_hd = read_u32(self.bytes, self.named + 8 * mid);
            match entry_hd.cmp(&hd) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => {
                    let off = read_u32(self.bytes, self.named + 8 * mid + 4) as usize;
                    return Some(self.pool_str(off));
                }
            }
        }
        None
    }

    fn magnitude_from(&self, section: usize, idx: usize) -> Option<f64> {
        match read_i16(self.bytes, section + 2 * idx) {
            MAG_NONE => None,
            m => Some(f64::from(m) / 100.0),
        }
    }

    /// Coordinates are range-validated at pack time, so a `None` here is
    /// a blob-corruption bug: the row is logged and treated as a miss
    /// rather than panicking.
    fn checked_coord(&self, ra_degrees: f64, dec_degrees: f64) -> Option<IcrsCoord> {
        match IcrsCoord::try_new(ra_degrees / 15.0, dec_degrees) {
            Ok(c) => Some(c),
            Err(e) => {
                error!("packed catalog row has invalid coordinates ({e}); ignoring row");
                None
            }
        }
    }

    fn materialize_dso(&self, idx: usize) -> Option<ResolvedTarget> {
        let (ra, dec) = self.dso_coord_degrees(idx);
        let type_idx = self.bytes.get(self.dso_type + idx).copied().unwrap_or(0) as usize;
        let type_off = if type_idx < self.type_count {
            read_u32(self.bytes, self.types + 4 * type_idx) as usize
        } else {
            usize::MAX // out-of-table index reads as an empty type string
        };
        let size = match read_u16(self.bytes, self.dso_size + 2 * idx) {
            0 => None,
            s => Some(f64::from(s) / 10.0),
        };
        Some(ResolvedTarget {
            name: self.dso_name(idx).to_string(),
            object_type: self.pool_str(type_off).to_string(),
            coord: self.checked_coord(ra, dec)?,
            magnitude: self.magnitude_from(self.dso_mag, idx),
            size_arcmin: size,
            class: ObjectClass::DeepSky,
        })
    }

    fn materialize_star(&self, idx: usize) -> Option<ResolvedTarget> {
        let (ra, dec) = self.star_coord_degrees(idx);
        let hd = self.star_hd(idx);
        let name = match self.proper_name(hd) {
            Some(n) => n.to_string(),
            None => format!("HD {hd}"),
        };
        Some(ResolvedTarget {
            name,
            object_type: "*".to_string(),
            coord: self.checked_coord(ra, dec)?,
            magnitude: self.magnitude_from(self.star_mag, idx),
            size_arcmin: None,
            class: ObjectClass::Star,
        })
    }

    fn materialize(&self, row_ref: u32) -> Option<ResolvedTarget> {
        let idx = (row_ref & !STAR_BIT) as usize;
        if row_ref & STAR_BIT != 0 {
            self.materialize_star(idx)
        } else {
            self.materialize_dso(idx)
        }
    }

    fn key_at(&self, idx: usize) -> &str {
        self.pool_str(read_u32(self.bytes, self.keys + 8 * idx) as usize)
    }

    fn key_ref_at(&self, idx: usize) -> u32 {
        read_u32(self.bytes, self.keys + 8 * idx + 4)
    }

    fn key_lookup(&self, key: &str) -> Option<u32> {
        let mut lo = 0usize;
        let mut hi = self.key_count;
        while lo < hi {
            let mid = usize::midpoint(lo, hi);
            match self.key_at(mid).cmp(key) {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => return Some(self.key_ref_at(mid)),
            }
        }
        None
    }

    fn star_row_by_hd(&self, hd: u32) -> Option<usize> {
        // Star rows are dec-sorted, so HD lookup is a linear scan of the
        // u32 column — ~350k reads, well under a millisecond, and only
        // paid by explicit `HD nnn` queries.
        (0..self.star_count).find(|&i| self.star_hd(i) == hd)
    }

    /// Resolve a name to a target. Case-, whitespace-, and
    /// dash-insensitive; `"M 41"`, `"Messier 41"`, `"Sh2-101"`,
    /// `"sharpless 101"`, `"HD 227018"`, `"HDE 227018"`, `"Vega"`, and
    /// common-name aliases (`"Andromeda Galaxy"`) all resolve.
    #[must_use]
    pub fn resolve(&self, name: &str) -> Option<ResolvedTarget> {
        let key = normalize(name);
        if let Some(hd) = key.strip_prefix("hd").and_then(|d| d.parse::<u32>().ok()) {
            return self
                .star_row_by_hd(hd)
                .and_then(|i| self.materialize_star(i));
        }
        self.key_lookup(&key).and_then(|r| self.materialize(r))
    }

    /// Total number of catalog entries (deep-sky rows + star rows).
    /// Aliases are lookup keys, not entries, and are not counted.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.dso_count + self.star_count
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// First row whose dec (mas) is ≥ `min_mas`, by binary search over
    /// the dec-sorted column at `section`.
    fn dec_lower_bound(&self, section: usize, count: usize, min_mas: i64) -> usize {
        let mut lo = 0usize;
        let mut hi = count;
        while lo < hi {
            let mid = usize::midpoint(lo, hi);
            if i64::from(read_i32(self.bytes, section + 4 * mid)) < min_mas {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    /// Nearest catalog entry to `coord` under the class-tiered naming
    /// contract (planetarium-bridge design, issue #767): the best
    /// deep-sky hit within `tolerances.dso_arcmin` outranks any star hit
    /// regardless of separation; otherwise the best star within
    /// `tolerances.star_arcmin` wins. Separation orders hits within a
    /// class; exact ties fall back to catalog rank (M > NGC > IC >
    /// Sh2 > …) and then name, so entries at identical coordinates
    /// (M 42 / NGC 1976) resolve deterministically.
    #[must_use]
    pub fn nearest(
        &self,
        coord: &IcrsCoord,
        tolerances: &NearestTolerances,
    ) -> Option<NearestMatch> {
        let ra = coord.ra_hours() * 15.0;
        let dec = coord.dec_degrees();

        let dso = self
            .scan_band(
                self.dso_dec,
                self.dso_count,
                ra,
                dec,
                tolerances.dso_arcmin,
                |idx| self.dso_coord_degrees(idx),
            )
            .into_iter()
            .min_by(|a, b| {
                cmp_f64(a.1, b.1)
                    .then_with(|| self.dso_rank(a.0).cmp(&self.dso_rank(b.0)))
                    .then_with(|| self.dso_name(a.0).cmp(self.dso_name(b.0)))
            });
        if let Some((idx, separation)) = dso {
            return self
                .materialize_dso(idx)
                .map(|t| self.build_match(t, separation, ra, dec));
        }

        let star = self
            .scan_band(
                self.star_dec,
                self.star_count,
                ra,
                dec,
                tolerances.star_arcmin,
                |idx| self.star_coord_degrees(idx),
            )
            .into_iter()
            .min_by(|a, b| {
                cmp_f64(a.1, b.1).then_with(|| self.star_hd(a.0).cmp(&self.star_hd(b.0)))
            });
        star.and_then(|(idx, separation)| {
            self.materialize_star(idx)
                .map(|t| self.build_match(t, separation, ra, dec))
        })
    }

    /// All rows of a dec-sorted section within `radius_arcmin` of
    /// (`ra`, `dec`), as `(row index, separation arcmin)`. The dec band
    /// is binary-searched; candidates get the exact great-circle test.
    fn scan_band(
        &self,
        dec_section: usize,
        count: usize,
        ra: f64,
        dec: f64,
        radius_arcmin: f64,
        coord_of: impl Fn(usize) -> (f64, f64),
    ) -> Vec<(usize, f64)> {
        if radius_arcmin <= 0.0 || !radius_arcmin.is_finite() {
            return Vec::new();
        }
        let radius_mas = (radius_arcmin / ARCMIN_PER_DEGREE * MAS_PER_DEGREE).ceil() as i64;
        let dec_mas = (dec * MAS_PER_DEGREE) as i64;
        let mut hits = Vec::new();
        let mut idx = self.dec_lower_bound(dec_section, count, dec_mas - radius_mas);
        while idx < count {
            if i64::from(read_i32(self.bytes, dec_section + 4 * idx)) > dec_mas + radius_mas {
                break;
            }
            let (row_ra, row_dec) = coord_of(idx);
            let separation = separation_arcmin(ra, dec, row_ra, row_dec);
            if separation <= radius_arcmin {
                hits.push((idx, separation));
            }
            idx += 1;
        }
        hits
    }

    fn build_match(
        &self,
        target: ResolvedTarget,
        separation_arcmin: f64,
        query_ra: f64,
        query_dec: f64,
    ) -> NearestMatch {
        let centroid_ra = target.coord.ra_hours() * 15.0;
        let centroid_dec = target.coord.dec_degrees();
        NearestMatch {
            east_offset_arcmin: ra_delta_degrees(centroid_ra, query_ra)
                * centroid_dec.to_radians().cos()
                * ARCMIN_PER_DEGREE,
            north_offset_arcmin: (query_dec - centroid_dec) * ARCMIN_PER_DEGREE,
            separation_arcmin,
            target,
        }
    }

    /// Up to `limit` canonical names with the smallest Levenshtein
    /// distance from the query (≤ 3). Used to populate "did you
    /// mean…?" suggestions on a lookup miss in the MCP wrapper.
    /// Operates over the packed key table — canonical DSO names,
    /// common-name aliases, and IAU proper star names. Plain HD
    /// designations are deliberately not keys: suggesting `HD 227019`
    /// for a typo of `HD 227018` helps nobody, and resolving them goes
    /// through the numeric path instead. Aliases mapping to the same
    /// entry are deduped by canonical name.
    #[must_use]
    pub fn fuzzy_suggestions(&self, query: &str, limit: usize) -> Vec<String> {
        let q = normalize(query);
        let mut scored: Vec<(usize, u32)> = (0..self.key_count)
            .map(|i| (levenshtein(self.key_at(i), &q, 4), self.key_ref_at(i)))
            .filter(|(d, _)| *d <= 3)
            .collect();
        let mut named: Vec<(usize, String)> = scored
            .drain(..)
            .filter_map(|(d, r)| self.materialize(r).map(|t| (d, t.name)))
            .collect();
        named.sort_unstable_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let mut out: Vec<String> = Vec::with_capacity(limit);
        for (_, name) in named {
            if !out.contains(&name) {
                out.push(name);
                if out.len() == limit {
                    break;
                }
            }
        }
        out
    }
}

/// Total order over separations; NaN cannot occur (inputs are finite by
/// construction) but sorts last defensively.
fn cmp_f64(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Greater)
}

/// Truncated Levenshtein distance: returns `cap` if the true distance
/// is `≥ cap`. Cheap enough for a one-shot suggestion list across the
/// ~20k-entry key table.
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
mod tests;
