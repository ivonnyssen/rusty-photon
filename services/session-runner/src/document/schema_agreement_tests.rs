//! Keeps the published JSON Schema honest against the in-code validator.
//!
//! The validator is deliberately **stronger** than the schema (it also
//! enforces uniqueness, expression grammar, duration semantics, `$expr`
//! placement, and namespace scoping — rules JSON Schema cannot express),
//! so agreement is one-directional:
//!
//! > every document the validator accepts must pass the published schema.
//!
//! Equivalently (contrapositive): everything the schema rejects, the
//! validator rejects. A violation means the schema — the external
//! contract for authors, the future UI, and LLM generation — advertises
//! a stricter format than the engine implements, and must be fixed on
//! whichever side is wrong.

use serde_json::Value;

use super::corpus::{self, Case, Expect};
use super::Document;

static SCHEMA: &str = include_str!("../../schema/workflow-v1.schema.json");

fn schema_validator() -> jsonschema::Validator {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    jsonschema::validator_for(&schema).unwrap()
}

#[test]
fn test_everything_the_validator_accepts_passes_the_published_schema() {
    let schema = schema_validator();
    let mut failures = Vec::new();
    for Case { name, src, .. } in corpus::cases() {
        let value: Value = serde_json::from_str(&src).unwrap();
        if Document::from_value(&value).is_ok() && !schema.is_valid(&value) {
            let errors: Vec<String> = schema
                .iter_errors(&value)
                .map(|e| format!("{} at {}", e, e.instance_path()))
                .collect();
            failures.push(format!(
                "  {name}: schema rejects it: {}",
                errors.join("; ")
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} document(s) accepted by the validator but rejected by the published schema:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Guards against a silently-permissive schema hookup: the published
/// schema must actually reject structural garbage the corpus marks
/// invalid. (Validator-only rules — uniqueness, expressions, durations
/// beyond the pattern, `$expr` placement, `script` reservation — are
/// exempt: the schema legitimately accepts those.)
#[test]
fn test_the_published_schema_itself_rejects_structural_corpus_cases() {
    let schema = schema_validator();
    let structurally_invalid = [
        "document_not_an_object",
        "missing_version",
        "missing_name_and_root",
        "empty_name",
        "unknown_top_level_key",
        "parameter_required_false",
        "parameter_neither_required_nor_default",
        "empty_instruction",
        "misspelled_discriminant",
        "two_discriminants",
        "unknown_key_in_tool_instruction",
        "repeat_without_mode",
        "repeat_with_two_modes",
        "repeat_until_without_max_iterations",
        "repeat_missing_body",
        "repeat_empty_body",
        "if_missing_then",
        "set_empty",
        "set_key_not_under_session",
        "set_key_reserved_engine_state",
        "try_empty_body",
        "fail_without_message",
        "wait_empty",
        "wait_two_variants",
        "wait_duration_with_timeout",
        "wait_until_event_without_timeout",
        "log_missing_message",
        "log_bad_level",
        "trigger_missing_id_and_do",
        "trigger_on_with_both_sources",
        "poll_missing_interval",
        "trigger_empty_do",
    ];
    let cases = corpus::cases();
    let mut failures = Vec::new();
    for name in structurally_invalid {
        let case = cases
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("corpus case {name} disappeared"));
        assert!(
            matches!(case.expect, Expect::Invalid(_)),
            "{name} is no longer an invalid case"
        );
        let value: Value = serde_json::from_str(&case.src).unwrap();
        if schema.is_valid(&value) {
            failures.push(format!("  {name}: published schema accepts it"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} structurally-invalid corpus case(s) pass the published schema:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The schema deliberately accepts a structurally-well-formed `script`
/// node so the validator can reject it with the dedicated reservation
/// message instead of a generic unknown-key error.
#[test]
fn test_script_reservation_split_between_schema_and_validator() {
    let schema = schema_validator();
    let value = serde_json::json!({
        "version": 1, "name": "t", "root": { "script": "return 1" }
    });
    assert!(schema.is_valid(&value));
    let issues = Document::from_value(&value).unwrap_err();
    assert!(
        issues[0]
            .message
            .contains("reserved for a future format version"),
        "{}",
        issues[0].message
    );
}
