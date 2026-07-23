//! Config-load validation for `session.file_naming_pattern`
//! (rp-targets.md § File-naming template, Decision 11's predecessor
//! work). Parses the pattern into literal/token segments, rejects
//! unknown tokens, missing quota/uniqueness tokens, and adjacent
//! variable-width tokens with no unambiguous literal separator between
//! them. Rendering and parse-back against real filenames (the
//! progress-derivation half of the design) is not implemented yet —
//! this module only answers "would rp accept this pattern at startup."

/// One naming-template token: its canonical name and the leading/
/// trailing character classes a rendered value can start/end with —
/// used only for the adjacent-token ambiguity check.
struct TokenSpec {
    name: &'static str,
    leading: fn(char) -> bool,
    trailing: fn(char) -> bool,
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
    },
    TokenSpec {
        name: "filter",
        leading: is_alnum,
        trailing: is_alnum,
    },
    TokenSpec {
        name: "binning",
        leading: is_digit,
        trailing: is_digit,
    },
    TokenSpec {
        name: "frame_number",
        leading: is_digit,
        trailing: is_digit,
    },
    TokenSpec {
        name: "exposure",
        leading: is_digit,
        trailing: is_literal_lowercase_c,
    },
    TokenSpec {
        name: "filter_position",
        leading: is_digit,
        trailing: is_digit,
    },
    TokenSpec {
        name: "sensor_temp",
        leading: is_sensor_temp_leading,
        trailing: is_literal_c,
    },
    TokenSpec {
        name: "night_date",
        leading: is_digit,
        trailing: is_digit,
    },
    TokenSpec {
        name: "frame_type",
        leading: is_frame_type_leading,
        trailing: is_frame_type_trailing,
    },
    TokenSpec {
        name: "uuid8",
        leading: is_hex,
        trailing: is_hex,
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
}
