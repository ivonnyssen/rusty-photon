//! Parser for the `services/<svc>/pkg/doctor.toml` catalog-metadata
//! microformat (see `docs/services/doctor.md` §The derived catalog).
//!
//! The format is deliberately a strict TOML subset — `#` comments, blank
//! lines, and exactly the keys below — parsed with std only, so the 17
//! per-service parity tests and doctor's embedded catalog share one parser
//! without pulling a TOML crate into every service's dev-dependencies.
//! Anything outside the subset is a loud error, never a guess.
//!
//! ```toml
//! class = "alpaca"  # "alpaca" | "core"
//! port = 11113
//! ```

/// Which shared server shape a service's `server` block uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerClass {
    /// [`crate::AlpacaServerConfig`] — the 11 Alpaca drivers.
    Alpaca,
    /// [`crate::ServerConfig`] — the plain-HTTP services.
    Core,
}

/// One service's parsed catalog metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoctorToml {
    pub class: ServerClass,
    pub port: u16,
}

/// Parse a `doctor.toml`. Errors name the offending line so a parity-test
/// failure is self-explanatory.
pub fn parse(content: &str) -> Result<DoctorToml, String> {
    let mut class = None;
    let mut port = None;
    for (idx, raw) in content.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("line {}: expected `key = value`: {raw:?}", idx + 1))?;
        match (key.trim(), value.trim()) {
            ("class", v) => {
                let parsed = match v {
                    "\"alpaca\"" => ServerClass::Alpaca,
                    "\"core\"" => ServerClass::Core,
                    other => {
                        return Err(format!(
                            "line {}: class must be \"alpaca\" or \"core\", got {other}",
                            idx + 1
                        ))
                    }
                };
                if class.replace(parsed).is_some() {
                    return Err(format!("line {}: duplicate key `class`", idx + 1));
                }
            }
            ("port", v) => {
                let parsed = v
                    .parse::<u16>()
                    .map_err(|e| format!("line {}: port is not a u16: {e}", idx + 1))?;
                if port.replace(parsed).is_some() {
                    return Err(format!("line {}: duplicate key `port`", idx + 1));
                }
            }
            (other, _) => return Err(format!("line {}: unknown key `{other}`", idx + 1)),
        }
    }
    Ok(DoctorToml {
        class: class.ok_or("missing key `class`")?,
        port: port.ok_or("missing key `port`")?,
    })
}

/// Strip a `#` comment. The microformat's only quoted strings are the two
/// `class` literals, which cannot contain `#`, so no quote-awareness needed.
fn strip_comment(line: &str) -> &str {
    match line.split_once('#') {
        Some((before, _)) => before,
        None => line,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_canonical_file_shape() {
        let meta = parse(
            "# Catalog metadata for rusty-photon-doctor (docs/services/doctor.md).\n\
             class = \"alpaca\"  # which shared server shape\n\
             port = 11113\n",
        )
        .unwrap();
        assert_eq!(meta.class, ServerClass::Alpaca);
        assert_eq!(meta.port, 11113);
    }

    #[test]
    fn parses_core_class() {
        let meta = parse("class = \"core\"\nport = 11114\n").unwrap();
        assert_eq!(meta.class, ServerClass::Core);
    }

    #[test]
    fn rejects_unknown_class() {
        let err = parse("class = \"driver\"\nport = 1\n").unwrap_err();
        assert!(err.contains("class must be"), "{err}");
    }

    #[test]
    fn rejects_unknown_key() {
        let err = parse("class = \"core\"\nport = 1\nhealth = \"/health\"\n").unwrap_err();
        assert!(err.contains("unknown key `health`"), "{err}");
    }

    #[test]
    fn rejects_missing_keys() {
        assert!(parse("class = \"core\"\n").unwrap_err().contains("port"));
        assert!(parse("port = 1\n").unwrap_err().contains("class"));
    }

    #[test]
    fn rejects_duplicate_keys() {
        let err = parse("port = 1\nport = 2\nclass = \"core\"\n").unwrap_err();
        assert!(err.contains("duplicate key `port`"), "{err}");
    }

    #[test]
    fn rejects_out_of_range_port() {
        let err = parse("class = \"core\"\nport = 70000\n").unwrap_err();
        assert!(err.contains("not a u16"), "{err}");
    }

    #[test]
    fn rejects_non_key_value_lines() {
        let err = parse("[section]\nclass = \"core\"\nport = 1\n").unwrap_err();
        assert!(err.contains("line 1"), "{err}");
    }
}
