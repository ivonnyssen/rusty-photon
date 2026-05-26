//! Server-rendered pages and fragments (Maud + HTMX) for the configuration UI,
//! plus the form ⇆ Config mapping.
//!
//! The HTMX swap unit is the `#config-card` element: `GET /config/dsd-fp2`
//! returns the full page (or just the card for an HTMX request); `POST` and the
//! reconnect poll return a fresh `#config-card` fragment that HTMX swaps in by
//! `outerHTML`.

use std::collections::HashMap;

use maud::{html, Markup, DOCTYPE};
use serde_json::Value;

use crate::driver_client::{ConfigClientError, FieldError};

/// A status banner rendered above the form.
#[derive(Debug, Clone, Copy)]
pub enum Banner {
    /// `config.apply` returned `status:"ok"` — persisted, no reload needed.
    Saved,
    /// `config.apply` returned `status:"invalid"`.
    Invalid,
    /// The reconnect poll found the driver back after a reload.
    Reconnected,
}

/// The full HTML shell: dark theme, embedded CSS + HTMX, top nav.
pub fn layout(title: &str, body: Markup) -> Markup {
    html! {
        (DOCTYPE)
        html lang="en" {
            head {
                meta charset="utf-8";
                meta name="viewport" content="width=device-width, initial-scale=1";
                title { (title) }
                link rel="stylesheet" href="/assets/app.css";
                script src="/assets/htmx.min.js" {}
            }
            body {
                nav.topnav {
                    div.logo {}
                    span.title { "rusty-photon" }
                    span.crumb { "· configuration" }
                }
                main.container { (body) }
            }
        }
    }
}

/// The index: links to the configurable services (Phase 2: just `dsd-fp2`).
pub fn index_page() -> Markup {
    layout(
        "rusty-photon · configuration",
        html! {
            h1 { "Configuration" }
            p.subtitle {
                "Per-device settings. Changes are applied by the driver and take "
                "effect with a brief in-process reload."
            }
            ul.service-list {
                li {
                    a href="/config/dsd-fp2" {
                        span { "Deep Sky Dad FP2" }
                        span.svc-id { "dsd-fp2" }
                    }
                }
            }
        },
    )
}

/// The `dsd-fp2` configuration form, filled from the effective config. Override-
/// pinned fields render disabled; `errors` annotate fields after a rejected apply.
pub fn config_card(
    config: &Value,
    overrides: &[String],
    errors: &[FieldError],
    banner: Option<Banner>,
) -> Markup {
    let config_blob = serde_json::to_string(config).unwrap_or_default();
    let overrides_blob = serde_json::to_string(overrides).unwrap_or_default();
    html! {
        div #config-card.card {
            @if let Some(b) = banner { (banner_markup(b)) }
            h1 { "Deep Sky Dad FP2" }
            p.subtitle { "dsd-fp2 · CoverCalibrator" }
            form method="post" action="/config/dsd-fp2"
                hx-post="/config/dsd-fp2" hx-target="#config-card" hx-swap="outerHTML" {
                input type="hidden" name="__config" value=(config_blob);
                input type="hidden" name="__overrides" value=(overrides_blob);

                fieldset {
                    legend { "Serial" }
                    (field_input(config, overrides, errors, "Port", "serial.port", "/serial/port", "text"))
                    (field_input(config, overrides, errors, "Baud rate", "serial.baud_rate", "/serial/baud_rate", "number"))
                    (field_input(config, overrides, errors, "Polling interval", "serial.polling_interval", "/serial/polling_interval", "text"))
                    (field_input(config, overrides, errors, "Timeout", "serial.timeout", "/serial/timeout", "text"))
                }
                fieldset {
                    legend { "Server" }
                    (field_input(config, overrides, errors, "Port", "server.port", "/server/port", "number"))
                    (field_input(config, overrides, errors, "Discovery port", "server.discovery_port", "/server/discovery_port", "number"))
                }
                fieldset {
                    legend { "Cover calibrator" }
                    (field_input(config, overrides, errors, "Name", "cover_calibrator.name", "/cover_calibrator/name", "text"))
                    (field_input(config, overrides, errors, "Unique ID", "cover_calibrator.unique_id", "/cover_calibrator/unique_id", "text"))
                    (field_input(config, overrides, errors, "Description", "cover_calibrator.description", "/cover_calibrator/description", "text"))
                    (field_input(config, overrides, errors, "Max brightness", "cover_calibrator.max_brightness", "/cover_calibrator/max_brightness", "number"))
                    (field_checkbox(config, "Enabled", "cover_calibrator.enabled", "/cover_calibrator/enabled"))
                }
                div.actions { button.primary type="submit" { "Apply" } }
            }
        }
    }
}

/// The "applying — reconnecting" fragment: polls `…/status` once a second until
/// the driver answers and the poll swaps in a fresh card.
pub fn reconnecting_card() -> Markup {
    html! {
        div #config-card.card hx-get="/config/dsd-fp2/status" hx-trigger="every 1s"
            hx-swap="outerHTML" hx-target="this" {
            div class="banner applying" {
                span.dot {}
                span { "Saved — the driver is reloading. Reconnecting…" }
            }
        }
    }
}

/// An error card derived from a `ConfigClientError`, with a retry affordance.
pub fn error_card(err: &ConfigClientError) -> Markup {
    let message = if err.is_action_not_implemented() {
        "This driver does not expose configuration actions.".to_string()
    } else {
        err.to_string()
    };
    error_card_with_message(&message)
}

/// An error card with an explicit message (e.g. a malformed form submission).
pub fn message_error_card(message: &str) -> Markup {
    error_card_with_message(message)
}

fn error_card_with_message(message: &str) -> Markup {
    html! {
        div #config-card.card {
            div class="banner error" { span.dot {} span { (message) } }
            p {
                a href="/config/dsd-fp2" hx-get="/config/dsd-fp2" hx-target="#config-card"
                    hx-swap="outerHTML" { "Retry" }
            }
        }
    }
}

fn banner_markup(banner: Banner) -> Markup {
    let (kind, text) = match banner {
        Banner::Saved => ("ok", "Saved. No reload was needed."),
        Banner::Invalid => (
            "error",
            "Some values were rejected. Fix the highlighted fields and apply again.",
        ),
        Banner::Reconnected => (
            "ok",
            "Reconnected. The driver reloaded with the new configuration.",
        ),
    };
    html! {
        div class=(format!("banner {kind}")) { span.dot {} span { (text) } }
    }
}

fn field_input(
    config: &Value,
    overrides: &[String],
    errors: &[FieldError],
    label: &str,
    name: &str,
    pointer: &str,
    input_type: &str,
) -> Markup {
    let pinned = overrides.iter().any(|o| o == name);
    let read_only = is_read_only(name);
    let disabled = pinned || read_only;
    let err = errors.iter().find(|e| e.path == name);
    let value = str_at(config, pointer);
    html! {
        div.field.pinned[disabled].invalid[err.is_some()] {
            label for=(name) { (label) }
            input type=(input_type) id=(name) name=(name) value=(value) disabled[disabled];
            @if let Some(e) = err { div.error { (e.msg) } }
            @if pinned {
                div.hint { "Pinned by a command-line override; change the driver's launch flags to edit it." }
            } @else if read_only {
                div.hint {
                    "Read-only for now — the BFF can't follow a change to the "
                    "driver's address yet, so editing it here would lose the "
                    "connection. Change it in the driver's config file."
                }
            }
        }
    }
}

/// The `enabled` checkbox, rendered **read-only**. It shows the current state
/// but cannot be edited from the page: disabling the device unregisters the
/// `covercalibrator/0` endpoint the config actions themselves live on, which
/// would lock this page out of the driver (recoverable only by a manual
/// config-file edit + restart). [`merge_form`] also ignores the
/// `cover_calibrator.enabled` form field, so the UI path can't flip it. (A
/// hand-crafted POST that edits `enabled` inside the `__config` blob still can —
/// that is equivalent to any forged config and is the driver's job to reject;
/// tracked as follow-up.)
fn field_checkbox(config: &Value, label: &str, name: &str, pointer: &str) -> Markup {
    let on = bool_at(config, pointer);
    html! {
        div.field {
            div.checkbox {
                input type="checkbox" id=(name) name=(name) checked[on] disabled;
                label for=(name) { (label) }
            }
            div.hint {
                "Read-only here — disabling the device would remove this "
                "configuration page's own endpoint. Change it in the driver's "
                "config file."
            }
        }
    }
}

fn str_at(config: &Value, pointer: &str) -> String {
    match config.pointer(pointer) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn bool_at(config: &Value, pointer: &str) -> bool {
    config
        .pointer(pointer)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The merged config produced from a submitted form, ready to send to
/// `config.apply`, plus the override-pinned paths (echoed back so a re-render
/// keeps those fields disabled).
#[derive(Debug)]
pub struct MergedForm {
    pub config: Value,
    pub overrides: Vec<String>,
    /// BFF-side parse/range errors for numeric fields (e.g. a port above
    /// 65535). When non-empty, the form is re-rendered with these field errors
    /// rather than sent to the driver.
    pub errors: Vec<FieldError>,
}

/// A malformed form submission (a missing or unparseable hidden field). Both
/// hidden fields are always emitted by [`config_card`], so their absence or
/// corruption means the submission did not come from a rendered page.
#[derive(Debug, thiserror::Error)]
pub enum FormError {
    #[error("the form was missing the hidden configuration field")]
    MissingConfig,
    #[error("the hidden configuration field was not valid JSON: {0}")]
    BadConfig(String),
    #[error("the form was missing the hidden overrides field")]
    MissingOverrides,
    #[error("the hidden overrides field was not valid JSON: {0}")]
    BadOverrides(String),
}

enum Kind {
    Str,
    /// `u16` (e.g. a TCP port).
    U16,
    /// Optional `u16` — an empty value persists `null`.
    OptU16,
    /// `u32` (e.g. baud rate, max brightness).
    U32,
}

struct FieldSpec {
    name: &'static str,
    pointer: &'static str,
    kind: Kind,
}

/// Editable fields overlaid onto the round-tripped config blob. `enabled` is
/// deliberately *not* here: it is read-only (see [`field_checkbox`]) so the UI
/// can't disable the device and unregister the very endpoint the config actions
/// live on — it round-trips from the hidden blob untouched.
const EDITABLE_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "serial.port",
        pointer: "/serial/port",
        kind: Kind::Str,
    },
    FieldSpec {
        name: "serial.baud_rate",
        pointer: "/serial/baud_rate",
        kind: Kind::U32,
    },
    FieldSpec {
        name: "serial.polling_interval",
        pointer: "/serial/polling_interval",
        kind: Kind::Str,
    },
    FieldSpec {
        name: "serial.timeout",
        pointer: "/serial/timeout",
        kind: Kind::Str,
    },
    FieldSpec {
        name: "server.port",
        pointer: "/server/port",
        kind: Kind::U16,
    },
    FieldSpec {
        name: "server.discovery_port",
        pointer: "/server/discovery_port",
        kind: Kind::OptU16,
    },
    FieldSpec {
        name: "cover_calibrator.name",
        pointer: "/cover_calibrator/name",
        kind: Kind::Str,
    },
    FieldSpec {
        name: "cover_calibrator.unique_id",
        pointer: "/cover_calibrator/unique_id",
        kind: Kind::Str,
    },
    FieldSpec {
        name: "cover_calibrator.description",
        pointer: "/cover_calibrator/description",
        kind: Kind::Str,
    },
    FieldSpec {
        name: "cover_calibrator.max_brightness",
        pointer: "/cover_calibrator/max_brightness",
        kind: Kind::U32,
    },
];

/// Fields shown but **not editable** from the page (rendered disabled; skipped
/// by [`merge_form`] so they round-trip from the hidden blob).
///
/// `server.port` changes the driver's bound address on reload, but the BFF keeps
/// using its configured `base_url` — so editing it here would lock the page out
/// of the driver until the BFF config is updated + restarted. Coordinating that
/// cross-service address change is deferred to the equipment roster (Phase 5);
/// until then the field is read-only. (`server.discovery_port` is *not* listed:
/// it is a separate UDP port that does not affect the HTTP endpoint the BFF
/// talks to, so changing it doesn't break the connection.)
const READ_ONLY_FIELDS: &[&str] = &["server.port"];

fn is_read_only(name: &str) -> bool {
    READ_ONLY_FIELDS.contains(&name)
}

/// Rebuild the full Config from a submitted form: start from the hidden
/// round-tripped blob and overlay the editable fields. Override-pinned and
/// read-only fields are not overlaid (the driver skips override-pinned ones
/// anyway, and read-only ones must round-trip untouched), so a transient
/// `--port` can't be re-submitted into the file.
pub fn merge_form(form: &HashMap<String, String>) -> Result<MergedForm, FormError> {
    let raw = form.get("__config").ok_or(FormError::MissingConfig)?;
    let mut config: Value =
        serde_json::from_str(raw).map_err(|e| FormError::BadConfig(e.to_string()))?;
    // `__overrides` is required and validated like `__config`: a malformed value
    // would otherwise be silently treated as "no overrides", letting pinned
    // fields render editable (and be overlaid) on a re-render instead of
    // surfacing the bad submission.
    let overrides_raw = form.get("__overrides").ok_or(FormError::MissingOverrides)?;
    let overrides: Vec<String> =
        serde_json::from_str(overrides_raw).map_err(|e| FormError::BadOverrides(e.to_string()))?;

    let is_pinned = |name: &str| overrides.iter().any(|o| o == name);

    let mut errors = Vec::new();
    for spec in EDITABLE_FIELDS {
        if is_pinned(spec.name) || is_read_only(spec.name) {
            continue;
        }
        let Some(raw) = form.get(spec.name) else {
            continue;
        };
        let trimmed = raw.trim();
        match spec.kind {
            Kind::Str => set_pointer(&mut config, spec.pointer, Value::String(raw.clone())),
            // Optional integer: an empty value persists `null`.
            Kind::OptU16 if trimmed.is_empty() => {
                set_pointer(&mut config, spec.pointer, Value::Null)
            }
            // Required integer, empty: keep the prior blob value. Clearing a
            // port must not silently become 0 (which dsd-fp2 reads as an
            // OS-assigned port).
            Kind::U16 | Kind::U32 if trimmed.is_empty() => {}
            // Parse into the field's bounded type. A non-empty value that
            // doesn't fit is a field error (actionable feedback) rather than a
            // silent coercion or a later driver-side parse failure.
            Kind::U16 | Kind::OptU16 => match trimmed.parse::<u16>() {
                Ok(n) => set_pointer(&mut config, spec.pointer, Value::from(n)),
                Err(_) => errors.push(field_error(
                    spec.name,
                    "must be a whole number between 0 and 65535",
                )),
            },
            Kind::U32 => match trimmed.parse::<u32>() {
                Ok(n) => set_pointer(&mut config, spec.pointer, Value::from(n)),
                Err(_) => errors.push(field_error(spec.name, "must be a whole number")),
            },
        }
    }

    // `enabled` is intentionally not overlaid from its form field: it is
    // read-only in the UI (see `field_checkbox`), so it round-trips from the
    // hidden blob and the normal UI can't disable the device (which would
    // unregister the config endpoint and lock the page out). This does not stop
    // a hand-crafted POST that edits `enabled` inside the `__config` blob itself
    // — that is equivalent to any forged config and is the driver's job to reject
    // (deferred); the form-field handling only governs the UI path.

    Ok(MergedForm {
        config,
        overrides,
        errors,
    })
}

fn field_error(path: &str, msg: &str) -> FieldError {
    FieldError {
        path: path.to_string(),
        msg: msg.to_string(),
    }
}

fn set_pointer(config: &mut Value, pointer: &str, value: Value) {
    if let Some(slot) = config.pointer_mut(pointer) {
        *slot = value;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_config() -> Value {
        json!({
            "serial": { "port": "/dev/ttyACM0", "baud_rate": 115200, "polling_interval": "500ms", "timeout": "3s" },
            "server": { "port": 11119, "discovery_port": 32227, "tls": null, "auth": null },
            "cover_calibrator": { "name": "FP2", "unique_id": "dsd-fp2-001", "description": "panel", "enabled": true, "max_brightness": 4096 }
        })
    }

    fn form_from(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn config_card_embeds_current_values_and_hidden_blob() {
        let markup = config_card(&sample_config(), &[], &[], None).into_string();
        assert!(markup.contains(r#"value="/dev/ttyACM0""#), "{markup}");
        assert!(markup.contains(r#"value="4096""#), "{markup}");
        // The hidden blob round-trips the full config for POST.
        assert!(markup.contains(r#"name="__config""#), "{markup}");
    }

    #[test]
    fn config_card_disables_override_pinned_fields() {
        let overrides = vec!["serial.port".to_string()];
        let markup = config_card(&sample_config(), &overrides, &[], None).into_string();
        // The serial.port input carries `disabled`.
        let pos = markup.find(r#"name="serial.port""#).unwrap();
        let start = markup[..pos].rfind("<input").unwrap();
        let end = markup[start..].find('>').unwrap() + start;
        assert!(
            markup[start..=end].contains("disabled"),
            "{}",
            &markup[start..=end]
        );
        assert!(
            markup.contains("Pinned by a command-line override"),
            "{markup}"
        );
    }

    #[test]
    fn config_card_shows_field_errors() {
        let errors = vec![FieldError {
            path: "serial.baud_rate".to_string(),
            msg: "must be greater than 0".to_string(),
        }];
        let markup =
            config_card(&sample_config(), &[], &errors, Some(Banner::Invalid)).into_string();
        assert!(markup.contains("must be greater than 0"), "{markup}");
        assert!(markup.contains("invalid"), "{markup}");
    }

    #[test]
    fn config_card_renders_enabled_read_only() {
        // The Enabled checkbox is shown for reference but disabled, so the UI
        // can't unregister the device (and the config endpoint with it).
        let markup = config_card(&sample_config(), &[], &[], None).into_string();
        let pos = markup.find(r#"name="cover_calibrator.enabled""#).unwrap();
        let start = markup[..pos].rfind("<input").unwrap();
        let end = markup[start..].find('>').unwrap() + start;
        assert!(
            markup[start..=end].contains("disabled"),
            "enabled checkbox not disabled: {}",
            &markup[start..=end]
        );
    }

    #[test]
    fn config_card_renders_server_port_read_only() {
        // server.port is read-only so the UI can't change the driver's address
        // out from under the BFF (which would lock the page out of the driver).
        let markup = config_card(&sample_config(), &[], &[], None).into_string();
        let pos = markup.find(r#"name="server.port""#).unwrap();
        let start = markup[..pos].rfind("<input").unwrap();
        let end = markup[start..].find('>').unwrap() + start;
        assert!(
            markup[start..=end].contains("disabled"),
            "server.port input not disabled: {}",
            &markup[start..=end]
        );
    }

    #[test]
    fn reconnecting_card_polls_status() {
        let markup = reconnecting_card().into_string();
        assert!(
            markup.contains(r#"hx-get="/config/dsd-fp2/status""#),
            "{markup}"
        );
        assert!(markup.contains(r#"hx-trigger="every 1s""#), "{markup}");
    }

    #[test]
    fn error_card_explains_action_not_implemented() {
        let err = ConfigClientError::Ascom {
            code: crate::driver_client::ACTION_NOT_IMPLEMENTED,
            message: "nope".to_string(),
        };
        let markup = error_card(&err).into_string();
        assert!(
            markup.contains("does not expose configuration actions"),
            "{markup}"
        );
    }

    #[test]
    fn merge_form_overlays_editable_fields() {
        let form = form_from(&[
            ("__config", &sample_config().to_string()),
            ("__overrides", "[]"),
            ("serial.port", "/dev/ttyACM5"),
            ("cover_calibrator.max_brightness", "2048"),
        ]);
        let merged = merge_form(&form).unwrap();
        assert_eq!(
            merged
                .config
                .pointer("/serial/port")
                .and_then(Value::as_str),
            Some("/dev/ttyACM5")
        );
        assert_eq!(
            merged
                .config
                .pointer("/cover_calibrator/max_brightness")
                .and_then(Value::as_u64),
            Some(2048)
        );
    }

    #[test]
    fn merge_form_does_not_overlay_pinned_fields() {
        let form = form_from(&[
            ("__config", &sample_config().to_string()),
            ("__overrides", r#"["serial.port"]"#),
            ("serial.port", "/dev/ttyACM9"),
        ]);
        let merged = merge_form(&form).unwrap();
        // The pinned field keeps the blob's value, not the (disabled) submission.
        assert_eq!(
            merged
                .config
                .pointer("/serial/port")
                .and_then(Value::as_str),
            Some("/dev/ttyACM0")
        );
        assert_eq!(merged.overrides, vec!["serial.port".to_string()]);
    }

    #[test]
    fn merge_form_never_changes_enabled() {
        // `merge_form` ignores the `cover_calibrator.enabled` form field
        // entirely (whether absent or forged), so `enabled` round-trips from the
        // hidden `__config` blob. (Tampering with the blob itself is a separate,
        // driver-side concern.)
        let form = form_from(&[
            ("__config", &sample_config().to_string()), // enabled: true
            ("__overrides", "[]"),
            ("cover_calibrator.enabled", "false"), // forged — must be ignored
        ]);
        let merged = merge_form(&form).unwrap();
        assert_eq!(
            merged
                .config
                .pointer("/cover_calibrator/enabled")
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn merge_form_empty_optional_becomes_null() {
        let form = form_from(&[
            ("__config", &sample_config().to_string()),
            ("__overrides", "[]"),
            ("server.discovery_port", ""),
        ]);
        let merged = merge_form(&form).unwrap();
        assert!(merged
            .config
            .pointer("/server/discovery_port")
            .unwrap()
            .is_null());
    }

    #[test]
    fn merge_form_missing_blob_is_an_error() {
        let form = form_from(&[("serial.port", "/dev/ttyACM0")]);
        let err = merge_form(&form).unwrap_err();
        assert!(matches!(err, FormError::MissingConfig));
    }

    #[test]
    fn merge_form_out_of_range_discovery_port_is_a_field_error() {
        // 99999 doesn't fit a u16, so it's a field error rather than a value
        // the driver would reject with a non-field parse error.
        let form = form_from(&[
            ("__config", &sample_config().to_string()),
            ("__overrides", "[]"),
            ("server.discovery_port", "99999"),
        ]);
        let merged = merge_form(&form).unwrap();
        assert_eq!(merged.errors.len(), 1, "{:?}", merged.errors);
        assert_eq!(merged.errors[0].path, "server.discovery_port");
        // The prior value is kept (not coerced) so re-render shows it.
        assert_eq!(
            merged
                .config
                .pointer("/server/discovery_port")
                .and_then(Value::as_u64),
            Some(32227)
        );
    }

    #[test]
    fn merge_form_empty_required_number_keeps_prior_value() {
        // Clearing a required number (baud_rate) must not silently become 0.
        let form = form_from(&[
            ("__config", &sample_config().to_string()),
            ("__overrides", "[]"),
            ("serial.baud_rate", ""),
        ]);
        let merged = merge_form(&form).unwrap();
        assert!(merged.errors.is_empty(), "{:?}", merged.errors);
        assert_eq!(
            merged
                .config
                .pointer("/serial/baud_rate")
                .and_then(Value::as_u64),
            Some(115200)
        );
    }

    #[test]
    fn merge_form_never_changes_server_port() {
        // server.port is read-only (the BFF can't follow a driver port change
        // yet), so a submitted/forged value is ignored — it round-trips from the
        // hidden blob, preventing a self-lockout via the UI.
        let form = form_from(&[
            ("__config", &sample_config().to_string()), // server.port: 11119
            ("__overrides", "[]"),
            ("server.port", "22222"), // forged — must be ignored
        ]);
        let merged = merge_form(&form).unwrap();
        assert_eq!(
            merged
                .config
                .pointer("/server/port")
                .and_then(Value::as_u64),
            Some(11119)
        );
    }

    #[test]
    fn merge_form_non_numeric_baud_rate_is_a_field_error() {
        let form = form_from(&[
            ("__config", &sample_config().to_string()),
            ("__overrides", "[]"),
            ("serial.baud_rate", "fast"),
        ]);
        let merged = merge_form(&form).unwrap();
        assert_eq!(merged.errors.len(), 1, "{:?}", merged.errors);
        assert_eq!(merged.errors[0].path, "serial.baud_rate");
    }

    #[test]
    fn merge_form_missing_overrides_is_an_error() {
        // `__overrides` is required like `__config`; absence is a malformed
        // submission, not silently "no overrides".
        let form = form_from(&[("__config", &sample_config().to_string())]);
        let err = merge_form(&form).unwrap_err();
        assert!(matches!(err, FormError::MissingOverrides), "{err:?}");
    }

    #[test]
    fn merge_form_invalid_overrides_is_an_error() {
        let form = form_from(&[
            ("__config", &sample_config().to_string()),
            ("__overrides", "not json"),
        ]);
        let err = merge_form(&form).unwrap_err();
        assert!(matches!(err, FormError::BadOverrides(_)), "{err:?}");
    }
}
