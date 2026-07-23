//! `session.file_naming_pattern` (rp-targets.md § File-naming
//! template): config-load validation, plus [`CompiledTemplate`]'s
//! render/parse-back engine. Parses a pattern into literal/token
//! segments, rejects unknown tokens, missing quota/uniqueness tokens,
//! and adjacent tokens with no unambiguous literal separator between
//! them ([`validate_pattern`], called at config load); compiles a
//! validated pattern into a reusable regex-backed engine that renders
//! [`TemplateFields`] into a filename base and parses one back
//! ([`CompiledTemplate`]).
//!
//! Landed: both halves of this module. **Not yet landed: any caller.**
//! `capture` still writes `<doc_uuid_8>.fits` regardless of the
//! configured pattern (Decision 11's `target` parameter — the thing
//! that would supply `render`'s `target`/`night_date`/`frame_type`
//! fields — hasn't landed), and the on-disk frame scan behind target
//! progress (rp-targets.md § Progress derivation) doesn't call
//! `parse` yet either.

use std::collections::HashMap;
use std::time::Duration;

use chrono::NaiveDate;
use regex::Regex;

use rp_targets::{Binning, TargetSlug};

use crate::planner::goal_wire::parse_binning;

/// One naming-template token: its canonical name, the leading/
/// trailing character classes a rendered value can start/end with
/// (the adjacent-token ambiguity check), and its regex shape
/// (rp-targets.md § File-naming template's shape table) — the named
/// capture group [`CompiledTemplate::compile`] builds and the
/// exact-match validator [`CompiledTemplate::render`] checks a
/// formatted value against before ever emitting it.
#[derive(Debug)]
struct TokenSpec {
    name: &'static str,
    leading: fn(char) -> bool,
    trailing: fn(char) -> bool,
    shape: &'static str,
}

fn is_digit(c: char) -> bool {
    c.is_ascii_digit()
}
fn is_slug_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
}
fn is_alnum(c: char) -> bool {
    c.is_ascii_alphanumeric()
}
fn is_hex(c: char) -> bool {
    c.is_ascii_digit() || ('a'..='f').contains(&c)
}
fn is_sensor_temp_leading(c: char) -> bool {
    c == '-' || c.is_ascii_digit()
}
fn is_literal_c(c: char) -> bool {
    c == 'C'
}
fn is_literal_lowercase_c(c: char) -> bool {
    c == 'c'
}
fn is_frame_type_leading(c: char) -> bool {
    matches!(c, 'L' | 'D' | 'F' | 'B')
}
fn is_frame_type_trailing(c: char) -> bool {
    matches!(c, 't' | 'k' | 's')
}

const TOKENS: &[TokenSpec] = &[
    TokenSpec {
        name: "target",
        leading: is_slug_char,
        trailing: is_slug_char,
        shape: "[a-z0-9-]+",
    },
    TokenSpec {
        name: "filter",
        leading: is_alnum,
        trailing: is_alnum,
        shape: "[A-Za-z0-9]+",
    },
    TokenSpec {
        name: "binning",
        leading: is_digit,
        trailing: is_digit,
        shape: r"\d+x\d+",
    },
    TokenSpec {
        name: "frame_number",
        leading: is_digit,
        trailing: is_digit,
        shape: r"\d+",
    },
    TokenSpec {
        name: "exposure",
        leading: is_digit,
        trailing: is_literal_lowercase_c,
        shape: r"\d+sec",
    },
    TokenSpec {
        name: "filter_position",
        leading: is_digit,
        trailing: is_digit,
        shape: r"\d+",
    },
    TokenSpec {
        name: "sensor_temp",
        leading: is_sensor_temp_leading,
        trailing: is_literal_c,
        shape: r"-?\d+C",
    },
    TokenSpec {
        name: "night_date",
        leading: is_digit,
        trailing: is_digit,
        shape: r"\d{4}-\d{2}-\d{2}",
    },
    TokenSpec {
        name: "frame_type",
        leading: is_frame_type_leading,
        trailing: is_frame_type_trailing,
        shape: "Light|Dark|Flat|Bias",
    },
    TokenSpec {
        name: "uuid8",
        leading: is_hex,
        trailing: is_hex,
        shape: "[0-9a-f]{8}",
    },
];

/// Deprecated aliases accepted for backward compatibility
/// (rp-targets.md: `{duration}`→`{exposure}`, `{sequence}`→`{frame_number}`).
fn resolve_alias(raw: &str) -> &str {
    match raw {
        "duration" => "exposure",
        "sequence" => "frame_number",
        other => other,
    }
}

fn token_spec(canonical: &str) -> Option<&'static TokenSpec> {
    TOKENS.iter().find(|t| t.name == canonical)
}

enum Segment<'a> {
    Literal(&'a str),
    Token(&'static TokenSpec),
}

/// Splits `pattern` into literal and `{token}` segments, resolving
/// deprecated aliases to their canonical name. Returns the offending
/// raw token name on the first unknown `{token}`.
fn parse_segments(pattern: &str) -> Result<Vec<Segment<'_>>, String> {
    let mut segments = Vec::new();
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        if open > 0 {
            segments.push(Segment::Literal(&rest[..open]));
        }
        let after_open = &rest[open + 1..];
        let close = after_open
            .find('}')
            .ok_or_else(|| format!("unterminated token starting at {:?}", &rest[open..]))?;
        let raw_name = &after_open[..close];
        let canonical = resolve_alias(raw_name);
        let spec = token_spec(canonical)
            .ok_or_else(|| format!("unknown naming-template token {{{raw_name}}}"))?;
        segments.push(Segment::Token(spec));
        rest = &after_open[close + 1..];
    }
    if !rest.is_empty() {
        segments.push(Segment::Literal(rest));
    }
    Ok(segments)
}

/// Validates a `session.file_naming_pattern` value against the
/// rp-targets.md contract: every quota token (`target`, `filter`,
/// `binning`, `exposure`) must appear, at least one uniqueness token
/// (`uuid8` or `frame_number`) must appear, every token must be known,
/// and no two variable-width tokens may sit adjacent without a literal
/// separator whose characters are excluded from both tokens' edge
/// charsets.
pub fn validate_pattern(pattern: &str) -> Result<(), String> {
    let segments = parse_segments(pattern)?;
    validate_segments(&segments)
}

/// The body of [`validate_pattern`], operating on already-parsed
/// segments so [`CompiledTemplate::compile`] can validate and compile
/// in one parse of the pattern string rather than two.
fn validate_segments(segments: &[Segment<'_>]) -> Result<(), String> {
    let present: Vec<&str> = segments
        .iter()
        .filter_map(|s| match s {
            Segment::Token(spec) => Some(spec.name),
            Segment::Literal(_) => None,
        })
        .collect();

    for required in ["target", "filter", "binning", "exposure"] {
        if !present.contains(&required) {
            return Err(format!(
                "file_naming_pattern is missing required token {{{required}}}"
            ));
        }
    }
    if !present.contains(&"uuid8") && !present.contains(&"frame_number") {
        return Err(
            "file_naming_pattern must contain a per-frame uniqueness token, {uuid8} or {frame_number}"
                .to_string(),
        );
    }

    // Two tokens directly adjacent (no literal at all between them) are
    // always ambiguous.
    for window in segments.windows(2) {
        if let [Segment::Token(left), Segment::Token(right)] = window {
            let (left, right) = (left.name, right.name);
            return Err(format!(
                "file_naming_pattern places {{{left}}} directly next to {{{right}}} with no literal separator between them"
            ));
        }
    }
    // Two tokens separated by a literal are ambiguous unless every
    // character of that literal is excluded from both the left token's
    // trailing charset and the right token's leading charset.
    for window in segments.windows(3) {
        if let [Segment::Token(left), Segment::Literal(sep), Segment::Token(right)] = window {
            if sep
                .chars()
                .any(|c| (left.trailing)(c) || (right.leading)(c))
            {
                let (left, right) = (left.name, right.name);
                return Err(format!(
                    "file_naming_pattern's separator {sep:?} between {{{left}}} and {{{right}}} does not unambiguously split them"
                ));
            }
        }
    }

    Ok(())
}

/// A capture's intent — the `{frame_type}` token's value. Only
/// `Light` frames bucket against `AcquisitionGoal` quotas (Dark/Flat/
/// Bias live under their own dirs) — see rp-targets.md § File-naming
/// template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum FrameType {
    #[display("Light")]
    Light,
    #[display("Dark")]
    Dark,
    #[display("Flat")]
    Flat,
    #[display("Bias")]
    Bias,
}

impl FrameType {
    /// The inverse of the derived `Display` — `None` for anything
    /// other than the four exact, case-sensitive literals the
    /// `{frame_type}` shape (`Light|Dark|Flat|Bias`) allows.
    fn parse(s: &str) -> Option<Self> {
        match s {
            "Light" => Some(FrameType::Light),
            "Dark" => Some(FrameType::Dark),
            "Flat" => Some(FrameType::Flat),
            "Bias" => Some(FrameType::Bias),
            _ => None,
        }
    }
}

/// One frame's naming-template field values — [`CompiledTemplate::render`]'s
/// input and [`CompiledTemplate::parse`]'s output. Every field is
/// optional: a caller supplies only what its configured pattern
/// actually references, and `parse` only ever populates fields the
/// pattern's tokens name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TemplateFields {
    pub target: Option<TargetSlug>,
    pub filter: Option<String>,
    pub binning: Option<Binning>,
    /// Per-spec sequence number; rendered zero-padded to width 4
    /// (`0002`).
    pub frame_number: Option<u32>,
    /// Rendered `format!("{}sec", d.as_secs())` — `render` rejects a
    /// non-whole-second value, since sub-second exposures have no
    /// naming-template representation (rp-targets.md § File-naming
    /// template).
    pub exposure: Option<Duration>,
    pub filter_position: Option<u32>,
    /// Whole-degree Celsius; rendered `format!("{c}C")` (Rust's `i32`
    /// `Display` already omits the sign for non-negatives and includes
    /// it for negatives, matching the `-?\d+C` shape exactly).
    pub sensor_temp_c: Option<i32>,
    /// The observing-night date (rp-targets.md § Progress derivation's
    /// noon-rollover rule) — a calendar concern this module doesn't
    /// compute itself, only render/parse.
    pub night_date: Option<NaiveDate>,
    pub frame_type: Option<FrameType>,
    /// The first 8 hex characters of the exposure document's UUID.
    pub uuid8: Option<String>,
}

impl TemplateFields {
    /// The value `render` should substitute for a `{name}` token, or
    /// an error naming the missing field or, for `{exposure}`, the
    /// violated whole-second business rule.
    fn rendered_value(&self, name: &str) -> Result<String, String> {
        let value = match name {
            "target" => self.target.as_ref().map(ToString::to_string),
            "filter" => self.filter.clone(),
            "binning" => self.binning.as_ref().map(ToString::to_string),
            "frame_number" => self.frame_number.map(|n| format!("{n:04}")),
            "exposure" => match self.exposure {
                Some(d) if d.subsec_nanos() == 0 => Some(format!("{}sec", d.as_secs())),
                Some(d) => {
                    return Err(format!(
                        "exposure {d:?} is not a whole number of seconds; the naming \
                         template only supports whole-second exposures"
                    ))
                }
                None => None,
            },
            "filter_position" => self.filter_position.map(|n| n.to_string()),
            "sensor_temp" => self.sensor_temp_c.map(|c| format!("{c}C")),
            "night_date" => self.night_date.map(|d| d.format("%Y-%m-%d").to_string()),
            "frame_type" => self.frame_type.map(|t| t.to_string()),
            "uuid8" => self.uuid8.clone(),
            _ => None,
        };
        value.ok_or_else(|| format!("missing value for token {{{name}}}"))
    }

    /// Fills in the field named `name` from one capture group's
    /// matched text. A value that fails its typed parse (should never
    /// happen — the enclosing regex already constrained its shape)
    /// leaves the field `None` rather than panicking; `capture`'s
    /// caller sees an absent field, not a crash.
    fn set_from_capture(&mut self, name: &str, value: &str) {
        match name {
            "target" => self.target = TargetSlug::new(value).ok(),
            "filter" => self.filter = Some(value.to_string()),
            "binning" => self.binning = parse_binning(value).ok(),
            "frame_number" => self.frame_number = value.parse().ok(),
            "exposure" => {
                self.exposure = value
                    .strip_suffix("sec")
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(Duration::from_secs);
            }
            "filter_position" => self.filter_position = value.parse().ok(),
            "sensor_temp" => {
                self.sensor_temp_c = value.strip_suffix('C').and_then(|s| s.parse().ok());
            }
            "night_date" => self.night_date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok(),
            "frame_type" => self.frame_type = FrameType::parse(value),
            "uuid8" => self.uuid8 = Some(value.to_string()),
            _ => {}
        }
    }
}

#[derive(Debug)]
enum TemplatePart {
    Literal(String),
    Token(&'static TokenSpec),
}

/// A `session.file_naming_pattern` (or a future `directory_pattern`)
/// compiled once into a reusable render/parse engine. Compiling is the
/// expensive step (building the combined regex); `render`/`parse` are
/// then cheap, so a caller should compile once per configured pattern
/// — at config load, or the first time it's needed — and reuse the
/// result for every capture / every frame the on-disk scan visits.
#[derive(Debug)]
pub struct CompiledTemplate {
    parts: Vec<TemplatePart>,
    /// Combined anchored regex with one named capture group per
    /// `TemplatePart::Token` (`{name}_{occurrence}`, since the `regex`
    /// crate rejects duplicate group names and nothing stops a
    /// pattern from repeating a token), used by `parse`.
    regex: Regex,
    /// Per-token-name exact-match validator (`^(?:{shape})$`) built
    /// from the same `TokenSpec::shape` the combined regex embeds —
    /// `render` checks every formatted value against it before ever
    /// emitting it, so `parse(render(x))` can never fail to read back
    /// a value `render` actually produced.
    validators: HashMap<&'static str, Regex>,
}

impl CompiledTemplate {
    /// Validates `pattern` (the same contract as [`validate_pattern`])
    /// and compiles it into a reusable render/parse engine.
    ///
    /// # Errors
    ///
    /// Returns the same error [`validate_pattern`] would for an
    /// invalid pattern, or an internal message if a `TOKENS` entry's
    /// own shape fails to compile as a regex — a static table bug,
    /// which is why every `TOKENS` shape is exercised by this module's
    /// tests.
    pub fn compile(pattern: &str) -> Result<Self, String> {
        let segments = parse_segments(pattern)?;
        validate_segments(&segments)?;

        let mut parts = Vec::with_capacity(segments.len());
        let mut regex_pattern = String::from("^");
        let mut validators = HashMap::new();
        let mut occurrences: HashMap<&'static str, u32> = HashMap::new();

        for segment in segments {
            match segment {
                Segment::Literal(text) => {
                    regex_pattern.push_str(&regex::escape(text));
                    parts.push(TemplatePart::Literal(text.to_string()));
                }
                Segment::Token(spec) => {
                    let occurrence = occurrences.entry(spec.name).or_insert(0);
                    regex_pattern.push_str(&format!(
                        "(?P<{}_{occurrence}>(?:{}))",
                        spec.name, spec.shape
                    ));
                    *occurrence += 1;
                    if !validators.contains_key(spec.name) {
                        let validator =
                            Regex::new(&format!("^(?:{})$", spec.shape)).map_err(|e| {
                                format!(
                                    "internal: token {{{}}}'s shape regex is invalid: {e}",
                                    spec.name
                                )
                            })?;
                        validators.insert(spec.name, validator);
                    }
                    parts.push(TemplatePart::Token(spec));
                }
            }
        }
        regex_pattern.push('$');
        let regex = Regex::new(&regex_pattern)
            .map_err(|e| format!("internal: compiled naming-template regex is invalid: {e}"))?;

        Ok(Self {
            parts,
            regex,
            validators,
        })
    }

    /// Renders `fields` through the pattern, producing the filename
    /// base (no extension) — e.g.
    /// `"m33_Ha_1x1_0002_120sec_fpos_680_-20C_a1b2c3d4"`.
    ///
    /// # Errors
    ///
    /// Names the missing token when `fields` lacks a value the
    /// pattern references, or names the offending value when a
    /// supplied value doesn't match the token's shape (e.g. a filter
    /// name containing a character outside `[A-Za-z0-9]`, or a
    /// non-whole-second exposure).
    pub fn render(&self, fields: &TemplateFields) -> Result<String, String> {
        let mut out = String::new();
        for part in &self.parts {
            match part {
                TemplatePart::Literal(text) => out.push_str(text),
                TemplatePart::Token(spec) => {
                    let value = fields.rendered_value(spec.name)?;
                    let validator = self.validators.get(spec.name).ok_or_else(|| {
                        format!(
                            "internal: no shape validator built for token {{{}}}",
                            spec.name
                        )
                    })?;
                    if !validator.is_match(&value) {
                        return Err(format!(
                            "value {value:?} for token {{{}}} does not match its shape",
                            spec.name
                        ));
                    }
                    out.push_str(&value);
                }
            }
        }
        Ok(out)
    }

    /// Parses a rendered filename base back into fields, or `None` if
    /// it doesn't match the pattern at all. A non-match is not an
    /// error: the on-disk frame scan's job (rp-targets.md § Progress
    /// derivation) is to skip and `debug!`-log a filename that doesn't
    /// match, never to fail the scan over it.
    #[must_use]
    pub fn parse(&self, filename_stem: &str) -> Option<TemplateFields> {
        let caps = self.regex.captures(filename_stem)?;
        let mut fields = TemplateFields::default();
        let mut occurrences: HashMap<&'static str, u32> = HashMap::new();
        for part in &self.parts {
            if let TemplatePart::Token(spec) = part {
                let occurrence = occurrences.entry(spec.name).or_insert(0);
                let group_name = format!("{}_{occurrence}", spec.name);
                *occurrence += 1;
                if let Some(m) = caps.name(&group_name) {
                    fields.set_from_capture(spec.name, m.as_str());
                }
            }
        }
        Some(fields)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    const DEFAULT_PATTERN: &str =
        "{target}_{filter}_{binning}_{frame_number}_{exposure}_fpos_{filter_position}_{sensor_temp}_{uuid8}";
    const DEPRECATED_ALIAS_PATTERN: &str =
        "{target}_{filter}_{binning}_{sequence}_{duration}_fpos_{filter_position}_{sensor_temp}_{uuid8}";

    #[test]
    fn default_pattern_is_valid() {
        validate_pattern(DEFAULT_PATTERN).unwrap();
    }

    #[test]
    fn deprecated_alias_pattern_is_valid() {
        validate_pattern(DEPRECATED_ALIAS_PATTERN).unwrap();
    }

    #[test]
    fn missing_quota_token_is_rejected() {
        let err = validate_pattern("{target}_{frame_number}_{uuid8}").unwrap_err();
        assert!(err.contains("filter") || err.contains("binning") || err.contains("exposure"));
    }

    #[test]
    fn adjacent_ambiguous_tokens_are_rejected() {
        let err = validate_pattern(
            "{target}_{filter}_{binning}_{frame_number}{exposure}_fpos_{filter_position}_{sensor_temp}_{uuid8}",
        )
        .unwrap_err();
        assert!(err.contains("frame_number") && err.contains("exposure"));
    }

    #[test]
    fn unknown_token_is_rejected() {
        let err =
            validate_pattern("{target}_{filter}_{binning}_{frame_number}_{exposure}_{bogus_token}")
                .unwrap_err();
        assert!(err.contains("bogus_token"));
    }

    #[test]
    fn missing_uniqueness_token_is_rejected() {
        let err = validate_pattern("{target}_{filter}_{binning}_{exposure}").unwrap_err();
        assert!(err.contains("uuid8") || err.contains("frame_number"));
    }

    // --- CompiledTemplate: render / parse ---------------------------

    fn documented_example_fields() -> TemplateFields {
        TemplateFields {
            target: Some(TargetSlug::new("m33").unwrap()),
            filter: Some("Ha".to_string()),
            binning: Some(Binning { x: 1, y: 1 }),
            frame_number: Some(2),
            exposure: Some(Duration::from_secs(120)),
            filter_position: Some(680),
            sensor_temp_c: Some(-20),
            uuid8: Some("a1b2c3d4".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn compile_rejects_the_same_invalid_patterns_validate_pattern_does() {
        let err = CompiledTemplate::compile("{target}_{bogus_token}").unwrap_err();
        assert!(err.contains("bogus_token"));
    }

    #[test]
    fn render_reproduces_the_documented_example() {
        let template = CompiledTemplate::compile(DEFAULT_PATTERN).unwrap();
        let rendered = template.render(&documented_example_fields()).unwrap();
        assert_eq!(rendered, "m33_Ha_1x1_0002_120sec_fpos_680_-20C_a1b2c3d4");
    }

    #[test]
    fn parse_recovers_rendered_fields_round_trip() {
        let template = CompiledTemplate::compile(DEFAULT_PATTERN).unwrap();
        let fields = documented_example_fields();
        let rendered = template.render(&fields).unwrap();
        let parsed = template.parse(&rendered).unwrap();
        assert_eq!(parsed, fields, "parse(render(x)) must equal x");
    }

    #[test]
    fn render_via_the_deprecated_alias_pattern_matches_the_canonical_one() {
        // {duration}/{sequence} are the same tokens as {exposure}/
        // {frame_number} under the hood (resolve_alias); rendering
        // through either pattern spelling must produce identical output.
        let canonical = CompiledTemplate::compile(DEFAULT_PATTERN).unwrap();
        let deprecated = CompiledTemplate::compile(DEPRECATED_ALIAS_PATTERN).unwrap();
        let fields = documented_example_fields();
        assert_eq!(
            canonical.render(&fields).unwrap(),
            deprecated.render(&fields).unwrap()
        );
    }

    #[test]
    fn render_names_the_missing_token() {
        let template = CompiledTemplate::compile(DEFAULT_PATTERN).unwrap();
        let mut fields = documented_example_fields();
        fields.filter = None;
        let err = template.render(&fields).unwrap_err();
        assert!(err.contains("filter"), "{err}");
    }

    #[test]
    fn render_rejects_a_filter_name_outside_the_token_shape() {
        let template = CompiledTemplate::compile(DEFAULT_PATTERN).unwrap();
        let mut fields = documented_example_fields();
        fields.filter = Some("H-alpha".to_string()); // hyphen is outside [A-Za-z0-9]
        let err = template.render(&fields).unwrap_err();
        assert!(err.contains("filter"), "{err}");
    }

    #[test]
    fn render_rejects_a_non_whole_second_exposure() {
        let template = CompiledTemplate::compile(DEFAULT_PATTERN).unwrap();
        let mut fields = documented_example_fields();
        fields.exposure = Some(Duration::from_millis(1500));
        let err = template.render(&fields).unwrap_err();
        assert!(err.contains("whole"), "{err}");
    }

    #[test]
    fn parse_returns_none_for_a_non_matching_filename() {
        let template = CompiledTemplate::compile(DEFAULT_PATTERN).unwrap();
        assert!(template.parse("not-even-close-to-the-pattern").is_none());
    }

    #[test]
    fn render_and_parse_round_trip_frame_type_and_night_date() {
        let pattern = "{target}_{filter}_{binning}_{exposure}_{frame_number}_{frame_type}_{night_date}_{uuid8}";
        let template = CompiledTemplate::compile(pattern).unwrap();
        let fields = TemplateFields {
            target: Some(TargetSlug::new("ngc7000").unwrap()),
            filter: Some("L".to_string()),
            binning: Some(Binning { x: 2, y: 2 }),
            exposure: Some(Duration::from_secs(30)),
            frame_number: Some(1),
            frame_type: Some(FrameType::Dark),
            night_date: Some(NaiveDate::from_ymd_opt(2026, 6, 2).unwrap()),
            uuid8: Some("deadbeef".to_string()),
            ..Default::default()
        };
        let rendered = template.render(&fields).unwrap();
        assert_eq!(
            rendered,
            "ngc7000_L_2x2_30sec_0001_Dark_2026-06-02_deadbeef"
        );
        assert_eq!(template.parse(&rendered).unwrap(), fields);
    }

    #[test]
    fn frame_type_round_trips_every_variant() {
        for ft in [
            FrameType::Light,
            FrameType::Dark,
            FrameType::Flat,
            FrameType::Bias,
        ] {
            assert_eq!(FrameType::parse(&ft.to_string()), Some(ft));
        }
    }
}
