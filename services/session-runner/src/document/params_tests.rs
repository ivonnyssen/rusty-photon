//! Parameter-binding (validation layer 3) tests.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::*;

fn decls(spec: Value) -> BTreeMap<String, ParameterDecl> {
    let doc = json!({ "version": 1, "name": "t", "parameters": spec,
                      "root": { "sequence": [] } });
    Document::from_value(&doc).unwrap().parameters
}

fn golden_decls() -> BTreeMap<String, ParameterDecl> {
    decls(json!({
        "cam": { "type": "string", "required": true },
        "count": { "type": "integer", "default": 3 },
        "fraction": { "type": "number", "default": 0.5 },
        "guide": { "type": "boolean", "default": true },
        "initial": { "type": "duration", "default": "1s" }
    }))
}

#[test]
fn test_supplied_values_bind_and_defaults_fill_the_rest() {
    let bound = bind_parameters(
        &golden_decls(),
        Some(&json!({ "cam": "main-cam", "count": 7 })),
    )
    .unwrap();
    assert_eq!(
        bound,
        json!({ "cam": "main-cam", "count": 7, "fraction": 0.5,
                "guide": true, "initial": "1s" })
    );
}

#[test]
fn test_no_parameters_object_is_fine_when_all_are_defaulted() {
    let d = decls(json!({ "n": { "type": "integer", "default": 1 } }));
    assert_eq!(bind_parameters(&d, None).unwrap(), json!({ "n": 1 }));
    assert_eq!(
        bind_parameters(&d, Some(&Value::Null)).unwrap(),
        json!({ "n": 1 })
    );
}

#[test]
fn test_missing_required_parameter_is_reported() {
    let issues = bind_parameters(&golden_decls(), None).unwrap_err();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].pointer, "/parameters");
    assert!(
        issues[0]
            .message
            .contains("missing required parameter `cam` (type `string`)"),
        "{}",
        issues[0].message
    );
}

#[test]
fn test_unknown_parameter_names_the_declared_set() {
    let issues = bind_parameters(
        &golden_decls(),
        Some(&json!({ "cam": "c", "camera": "typo" })),
    )
    .unwrap_err();
    assert_eq!(issues[0].pointer, "/parameters/camera");
    assert!(
        issues[0].message.contains("unknown parameter `camera`")
            && issues[0]
                .message
                .contains("cam, count, fraction, guide, initial"),
        "{}",
        issues[0].message
    );
}

#[test]
fn test_unknown_parameter_against_empty_declarations() {
    let issues = bind_parameters(&BTreeMap::new(), Some(&json!({ "x": 1 }))).unwrap_err();
    assert!(
        issues[0].message.contains("declares no parameters"),
        "{}",
        issues[0].message
    );
}

#[test]
fn test_type_mismatches_are_reported_per_parameter() {
    let issues = bind_parameters(
        &golden_decls(),
        Some(&json!({ "cam": 5, "count": 2.5, "guide": "yes" })),
    )
    .unwrap_err();
    let by_pointer: Vec<(&str, &str)> = issues
        .iter()
        .map(|i| (i.pointer.as_str(), i.message.as_str()))
        .collect();
    assert_eq!(issues.len(), 3, "{by_pointer:?}");
    assert!(issues
        .iter()
        .any(|i| i.pointer == "/parameters/cam" && i.message.contains("expected a `string`")));
    assert!(issues
        .iter()
        .any(|i| i.pointer == "/parameters/count" && i.message.contains("expected an `integer`")));
    assert!(issues
        .iter()
        .any(|i| i.pointer == "/parameters/guide" && i.message.contains("expected a `boolean`")));
}

#[test]
fn test_integer_accepts_json_integers_only() {
    let d = decls(json!({ "n": { "type": "integer", "required": true } }));
    assert!(bind_parameters(&d, Some(&json!({ "n": -3 }))).is_ok());
    assert!(bind_parameters(&d, Some(&json!({ "n": 3.0 }))).is_err());
}

#[test]
fn test_number_accepts_integers_and_floats() {
    let d = decls(json!({ "x": { "type": "number", "required": true } }));
    assert!(bind_parameters(&d, Some(&json!({ "x": 3 }))).is_ok());
    assert!(bind_parameters(&d, Some(&json!({ "x": 0.5 }))).is_ok());
    assert!(bind_parameters(&d, Some(&json!({ "x": "3" }))).is_err());
}

#[test]
fn test_duration_parameters_enforce_the_document_surface() {
    let d = decls(json!({ "t": { "type": "duration", "required": true } }));
    let bound = bind_parameters(&d, Some(&json!({ "t": "1h30m" }))).unwrap();
    // Duration values stay strings — expressions read them via seconds().
    assert_eq!(bound, json!({ "t": "1h30m" }));

    let issues = bind_parameters(&d, Some(&json!({ "t": "1day" }))).unwrap_err();
    assert!(
        issues[0].message.contains("document duration format"),
        "{}",
        issues[0].message
    );
    let issues = bind_parameters(&d, Some(&json!({ "t": 90 }))).unwrap_err();
    assert!(
        issues[0].message.contains("expected a `duration`"),
        "{}",
        issues[0].message
    );
}

#[test]
fn test_parameters_must_be_an_object() {
    let issues = bind_parameters(&golden_decls(), Some(&json!([1, 2]))).unwrap_err();
    assert_eq!(issues[0].pointer, "/parameters");
    assert!(issues[0].message.contains("must be a JSON object"));
}

#[test]
fn test_all_binding_problems_are_reported_together() {
    let issues = bind_parameters(
        &golden_decls(),
        Some(&json!({ "count": "many", "extra": 1 })),
    )
    .unwrap_err();
    // Wrong type, unknown name, and the missing required parameter all
    // surface in one pass.
    assert_eq!(issues.len(), 3, "{issues:?}");
}
