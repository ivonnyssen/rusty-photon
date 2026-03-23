use crate::world::{FilemonitorWorld, ParsingRuleConfig};
use cucumber::{given, then};

#[given("case-insensitive matching")]
fn case_insensitive(world: &mut FilemonitorWorld) {
    world.case_sensitive = false;
}

#[given("case-sensitive matching")]
fn case_sensitive(world: &mut FilemonitorWorld) {
    world.case_sensitive = true;
}

#[given(expr = "a contains rule with pattern {string} that evaluates to safe")]
fn contains_rule_safe(world: &mut FilemonitorWorld, pattern: String) {
    world.rules.push(ParsingRuleConfig {
        rule_type: "contains".to_string(),
        pattern,
        safe: true,
    });
}

#[given(expr = "a contains rule with pattern {string} that evaluates to unsafe")]
fn contains_rule_unsafe(world: &mut FilemonitorWorld, pattern: String) {
    world.rules.push(ParsingRuleConfig {
        rule_type: "contains".to_string(),
        pattern,
        safe: false,
    });
}

fn resolve_regex_pattern(name: &str) -> String {
    match name {
        "safe_or_ok" => r"Status:\s*(SAFE|OK)".to_string(),
        "danger_or_error" => r"Status:\s*(DANGER|ERROR)".to_string(),
        "invalid_bracket" => "[invalid(".to_string(),
        other => other.to_string(),
    }
}

#[given(expr = "a regex rule with pattern {string} that evaluates to safe")]
fn regex_rule_safe(world: &mut FilemonitorWorld, pattern: String) {
    world.rules.push(ParsingRuleConfig {
        rule_type: "regex".to_string(),
        pattern: resolve_regex_pattern(&pattern),
        safe: true,
    });
}

#[given(expr = "a regex rule with pattern {string} that evaluates to unsafe")]
fn regex_rule_unsafe(world: &mut FilemonitorWorld, pattern: String) {
    world.rules.push(ParsingRuleConfig {
        rule_type: "regex".to_string(),
        pattern: resolve_regex_pattern(&pattern),
        safe: false,
    });
}

#[given("filemonitor is running with these rules")]
async fn filemonitor_running_with_rules(world: &mut FilemonitorWorld) {
    world.start_filemonitor().await;
}

#[given(expr = "filemonitor is running with these rules and {int} second polling")]
async fn filemonitor_running_with_rules_and_polling(world: &mut FilemonitorWorld, interval: u64) {
    world.polling_interval = interval;
    world.start_filemonitor().await;
}

#[then(expr = "is_safe should return {word}")]
async fn is_safe_should_return(world: &mut FilemonitorWorld, expected: String) {
    let result = world.alpaca_get_issafe().await.unwrap();
    let expected_val = expected == "true";
    assert_eq!(
        result, expected_val,
        "expected is_safe={expected_val} but got {result}"
    );
}

#[then("is_safe should fail with a not connected error")]
async fn is_safe_should_fail_not_connected(world: &mut FilemonitorWorld) {
    let result = world.alpaca_get_issafe().await;
    let (error_number, error_message) = result.expect_err("expected NotConnected error but got Ok");
    assert_eq!(
        error_number, 1031,
        "expected NOT_CONNECTED error code (1031) but got {}: {}",
        error_number, error_message
    );
}

#[then(expr = "the name should be {string}")]
async fn name_should_be(world: &mut FilemonitorWorld, expected: String) {
    let json = world.alpaca_get("name").await;
    let name = json["Value"].as_str().unwrap_or("");
    assert_eq!(
        name, expected,
        "expected name '{expected}' but got '{name}'"
    );
}

#[then(expr = "the description should be {string}")]
async fn description_should_be(world: &mut FilemonitorWorld, expected: String) {
    let json = world.alpaca_get("description").await;
    let description = json["Value"].as_str().unwrap_or("");
    assert_eq!(description, expected);
}

#[then(expr = "the driver info should be {string}")]
async fn driver_info_should_be(world: &mut FilemonitorWorld, expected: String) {
    let json = world.alpaca_get("driverinfo").await;
    let driver_info = json["Value"].as_str().unwrap_or("");
    assert_eq!(driver_info, expected);
}

#[then(expr = "the driver version should be {string}")]
async fn driver_version_should_be(world: &mut FilemonitorWorld, expected: String) {
    let json = world.alpaca_get("driverversion").await;
    let driver_version = json["Value"].as_str().unwrap_or("");
    assert_eq!(driver_version, expected);
}
