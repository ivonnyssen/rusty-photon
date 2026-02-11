# Testing Rules and Guidelines

This document defines the rules and conventions for writing tests in the rusty-photon project. Follow these rules when adding new features, fixing bugs, or migrating existing tests.

## 1. Test Pyramid: Which Test Type to Use

The project uses four testing layers. Each serves a distinct purpose.

### 1.1 BDD Tests (Feature Files + Cucumber)

**Purpose:** Living specifications. These are the primary test type for service behavior.

**Use BDD tests for:**
- All observable device behavior (connect, disconnect, read values, evaluate state)
- Configuration loading and validation
- Error conditions that a user or integrator would encounter
- Cross-cutting concerns (concurrency, polling, state transitions)
- Any scenario where the Gherkin description itself has documentation value

**Do NOT use BDD tests for:**
- Wire protocol serialization/deserialization (use unit tests)
- Internal data structure invariants (use unit tests or property tests)
- ASCOM compliance (use ConformU)
- Fuzz-like edge case exploration (use property-based tests)

### 1.2 Unit Tests (Rust `#[test]` / `#[tokio::test]`)

**Purpose:** Fast, focused verification of internal components.

**Use unit tests for:**
- Protocol parsing and serialization (`test_protocol.rs`)
- Error type conversions and Display implementations (`test_error.rs`)
- Configuration defaults and deserialization (`test_config.rs`)
- Pure functions and data transformations
- In-source `#[cfg(test)]` modules for module-private logic

### 1.3 Property-Based Tests (proptest)

**Purpose:** Discover edge cases through randomized input.

**Use property tests for:**
- Determinism invariants (same input always produces same output)
- Robustness (no panics on arbitrary input)
- Round-trip properties (serialize then deserialize returns original)
- Domain invariants that should hold for all inputs

### 1.4 ConformU Integration Tests

**Purpose:** ASCOM Alpaca protocol compliance.

**Use ConformU tests for:**
- Verifying a service conforms to the ASCOM Alpaca standard
- These are always `#[ignore]` and run manually or in dedicated CI

---

## 2. BDD Feature File Rules

These are the most important rules. Feature files are both tests and documentation.

### 2.1 One Feature File Per Concern

Organize feature files by functional area, not by implementation module. Each feature file should answer the question: *"What does the system do regarding [concern]?"*

**Good file names (concern-oriented):**
- `safety_evaluation.feature` — How safety rules are evaluated
- `connection_lifecycle.feature` — How the device connects and disconnects
- `file_polling.feature` — How file changes are detected
- `configuration.feature` — How configuration is loaded and validated

**Bad file names (implementation-oriented):**
- `device_tests.feature` — Too vague
- `serial_manager.feature` — Names an internal component, not a behavior
- `misc.feature` — No organizing principle

### 2.2 Feature Descriptions State the Contract

The `Feature:` line plus its description should read as a specification, not a test plan. State the behavioral contract: what the system does, what rules apply, what the defaults are.

**Good:**
```gherkin
Feature: Safety evaluation rules
  The safety monitor evaluates file content against configured parsing rules.
  Parsing rules are evaluated in order; the first match determines safety.
  No match defaults to unsafe.
```

**Bad:**
```gherkin
Feature: Safety evaluation tests
  Tests for the safety evaluation logic.
```

### 2.3 Scenarios Describe Outcomes, Not Procedures

A scenario title should state **what happens** (the outcome), not **what you do** (the procedure). A reader should understand the expected behavior from the title alone.

**Good:**
```gherkin
Scenario: Device starts disconnected
Scenario: First matching rule wins
Scenario: Disconnected device reports unsafe via is_safe
Scenario: Fail to connect when monitored file does not exist
```

**Bad:**
```gherkin
Scenario: Test device connection
Scenario: Check rule priority
Scenario: Call is_safe when disconnected
```

### 2.4 Use Scenario Outlines for Parameterized Behavior

When the same behavioral pattern applies to multiple inputs, use `Scenario Outline` with `Examples`. This makes the pattern explicit and the variations easy to scan.

```gherkin
Scenario Outline: Contains rule evaluation
  Given case-insensitive matching
  And a contains rule with pattern "OPEN" that evaluates to safe
  And a contains rule with pattern "CLOSED" that evaluates to unsafe
  And a device configured with these rules
  When I evaluate the safety of "<content>"
  Then the result should be <expected>

  Examples:
    | content              | expected |
    | Roof Status: OPEN    | safe     |
    | Roof Status: CLOSED  | unsafe   |
    | roof status: open    | safe     |
    | Unknown status       | unsafe   |
```

Use individual scenarios (not outlines) when different inputs need different setup or tell different stories.

### 2.5 Use `@serial` Tag for Tests With Side Effects

Tag features or scenarios with `@serial` when they depend on timing, shared resources, or file I/O that cannot safely run in parallel.

```gherkin
@serial
Feature: File content polling
```

### 2.6 Avoid Gherkin Parser Pitfalls

These are known issues with the Gherkin parser used by cucumber-rs:

- **Do NOT start description lines with `Rule`** — it is a Gherkin 6+ keyword and will be parsed as structure, not text.
- **Do NOT use `|` in step text** — it is the table delimiter. Use symbolic names mapped in step definitions instead.
- **Regex patterns go in step definitions, not feature files.** Use human-readable names in features (e.g., `"safe_or_ok"`) mapped to actual patterns in code via a resolver function.

---

## 3. Step Definition Rules

### 3.1 Organize Steps by Concern, Matching Feature Files

Each feature file should have a corresponding step definition module. Steps that are shared across features (like connection steps used in both `connection_lifecycle.feature` and `file_polling.feature`) go in the module matching their primary concern.

```
features/                        steps/
  configuration.feature    →       config_steps.rs
  connection_lifecycle.feature →   connection_steps.rs
  safety_evaluation.feature  →    safety_steps.rs
  file_polling.feature       →    polling_steps.rs
  concurrency.feature        →    concurrency_steps.rs
```

### 3.2 Steps Must Be Reusable Across Scenarios

Write step definitions that are generic enough to be composed across scenarios. The same `Given a monitoring file containing {string}` step is used in connection, polling, and concurrency scenarios.

**Good (reusable, parameterized):**
```rust
#[given(expr = "a monitoring file containing {string}")]
fn monitoring_file_containing(world: &mut MyWorld, content: String) {
    world.create_temp_file(&content);
}
```

**Bad (scenario-specific):**
```rust
#[given("a file with OPEN content for safety test")]
fn setup_safety_file(world: &mut MyWorld) {
    world.create_temp_file("OPEN");
}
```

### 3.3 Use `expect()` in Given/When Steps, `assert!` in Then Steps

- **Given** steps set up preconditions. If setup fails, the test infrastructure is broken — use `expect()` or `unwrap()` to fail fast with a clear message.
- **When** steps execute the action under test. Use `unwrap()` for actions that should succeed, or capture errors into `world.last_error` when testing failure paths.
- **Then** steps verify outcomes. Use `assert!`, `assert_eq!`, or `assert_ne!` with descriptive messages.

```rust
// Given: fail fast on setup problems
#[given(expr = "a monitoring file containing {string}")]
fn monitoring_file(world: &mut MyWorld, content: String) {
    world.create_temp_file(&content);  // uses expect() internally
}

// When: capture errors for failure scenarios
#[when("I try to connect the device")]
async fn try_connect(world: &mut MyWorld) {
    match device.set_connected(true).await {
        Ok(()) => world.last_error = None,
        Err(e) => world.last_error = Some(e.to_string()),
    }
}

// Then: assert with context
#[then("the result should be safe")]
fn result_safe(world: &mut MyWorld) {
    let result = world.safety_result.expect("no safety result");
    assert!(result, "expected safe but got unsafe");
}
```

### 3.4 Use a Pattern Resolver for Complex Values

When feature files need to reference technical values (regex patterns, JSON, binary data), use human-readable symbolic names in Gherkin and resolve them in step definitions.

```rust
fn resolve_regex_pattern(name: &str) -> String {
    match name {
        "safe_or_ok" => r"Status:\s*(SAFE|OK)".to_string(),
        "danger_or_error" => r"Status:\s*(DANGER|ERROR)".to_string(),
        "invalid_bracket" => "[invalid(".to_string(),
        other => other.to_string(),
    }
}
```

This keeps feature files readable while allowing precise technical control.

### 3.5 Distinguish "I do X" from "I try to do X"

Use separate step definitions for actions expected to succeed vs. actions expected to fail:

- `When I connect the device` — calls `unwrap()`, fails the test if the action fails
- `When I try to connect the device` — captures the error into `world.last_error`

This makes the intent clear in both the feature file and the step definition.

---

## 4. World Struct Rules

### 4.1 Use `Option<T>` for State That Builds Incrementally

The World struct accumulates state across Given/When steps. Use `Option<T>` for values that are set during the scenario and `Vec<T>` for collections that grow.

```rust
#[derive(Debug, Default, World)]
pub struct MyWorld {
    pub config: Option<Config>,           // Set once during setup
    pub device: Option<Arc<MyDevice>>,    // Built from accumulated state
    pub rules: Vec<ParsingRule>,          // Grows with each Given step
    pub last_error: Option<String>,       // Captured in When steps
    pub temp_dir: Option<TempDir>,        // Manages temp file lifetime
}
```

### 4.2 Put Setup Helpers on the World Struct

Common setup logic (creating temp files, building configs, constructing devices) should be methods on the World struct, not free functions in step files. This keeps step definitions thin.

```rust
impl MyWorld {
    pub fn create_temp_file(&mut self, content: &str) -> PathBuf { ... }
    pub fn build_config(&self, file_path: PathBuf) -> Config { ... }
    pub fn build_device(&mut self) { ... }
}
```

### 4.3 Use `TempDir` for File Lifecycle

The `tempfile::TempDir` stored in the World struct ensures automatic cleanup when each scenario ends. Never use hardcoded paths for test files.

### 4.4 Wrap Devices in `Arc` for Concurrency Scenarios

Concurrency scenarios spawn multiple async tasks that all need access to the device. Store devices as `Arc<Device>` in the World struct from the start. This avoids needing different World types for concurrent vs. sequential scenarios.

---

## 5. BDD Test Infrastructure Rules

### 5.1 Entry Point Structure

Each service's BDD tests follow this structure:

```
tests/
  bdd.rs                    # Entry point (harness = false)
  bdd/
    world.rs                # World struct + helpers
    steps/
      mod.rs                # pub mod for each step file
      connection_steps.rs
      ...
  features/
    connection_lifecycle.feature
    ...
```

The `bdd.rs` entry point uses `#[path = "..."]` imports because test crate roots see siblings, not children:

```rust
#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

use cucumber::World as _;
use world::MyWorld;

#[tokio::main]
async fn main() {
    MyWorld::run("tests/features").await;
}
```

### 5.2 Register in Cargo.toml

```toml
[[test]]
name = "bdd"
harness = false
```

---

## 6. Unit Test Rules

These rules apply to traditional `#[test]` and `#[tokio::test]` tests.

### 6.1 One Test Function Per Behavior

Each test should verify exactly one behavior. Name the test `test_<component>_<behavior>`:

```rust
#[tokio::test]
async fn test_focuser_position_not_connected() { ... }

#[tokio::test]
async fn test_focuser_move_negative_position_rejected() { ... }
```

### 6.2 Use `unwrap()` Over `assert!(result.is_ok())`

Per CLAUDE.md Rule 7: prefer tests that fail with clear errors. `unwrap()` produces a message showing **what** the error was. `assert!(result.is_ok())` just says `false`.

**Good:**
```rust
let position = device.position().await.unwrap();
assert_eq!(position, 10000);
```

**Bad:**
```rust
let result = device.position().await;
assert!(result.is_ok());
```

### 6.3 Test Both Success and Error Paths

For any operation that can fail, write separate tests for the success path and each meaningful failure mode. Assert the specific error code, not just that an error occurred.

```rust
#[tokio::test]
async fn test_focuser_position_connected() {
    // ... connect ...
    let position = device.position().await.unwrap();
    assert_eq!(position, 10000);
}

#[tokio::test]
async fn test_focuser_position_not_connected() {
    let err = device.position().await.unwrap_err();
    assert_eq!(err.code, ASCOMErrorCode::NOT_CONNECTED);
}
```

### 6.4 Test File Organization

- `test_protocol.rs` — Wire protocol serialization/deserialization
- `test_config.rs` — Configuration defaults and JSON handling
- `test_error.rs` — Error types, Display, and conversions
- `test_serial_manager.rs` — Connection lifecycle and polling
- `test_device_mock.rs` — Device trait implementation with mock I/O
- `test_cli.rs` — CLI argument parsing
- `test_server.rs` / `test_lib.rs` — Server integration tests
- `test_property.rs` — Property-based tests
- `conformu_integration.rs` — ASCOM conformance (always `#[ignore]`)

### 6.5 Mock Infrastructure Lives in Test Files

Hand-written mocks (`MockSerialReader`, `MockSerialWriter`, `MockSerialPortFactory`) are defined in the test files that use them. They are NOT feature-gated. The `#[cfg(feature = "mock")]` flag is reserved for the feature-gated `MockSerialPortFactory` in `src/` used by ConformU and server tests.

### 6.6 Serialize Server Tests

Server integration tests that bind to ports must use a static `Mutex<()>` to prevent parallel execution conflicts with the discovery service:

```rust
static SERVER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
async fn test_server_starts() {
    let _lock = SERVER_LOCK.lock().unwrap();
    // ... start server on port 0 ...
}
```

---

## 7. Migration Strategy: From Integration Tests to BDD

When migrating existing integration tests to BDD:

1. **Group related tests into features.** Multiple `test_device_*` functions testing connection become scenarios in `connection_lifecycle.feature`.

2. **Extract the behavioral pattern.** A test like `test_focuser_position_not_connected` becomes:
   ```gherkin
   Scenario: Position read fails when disconnected
     Given a device that is not connected
     When I try to read the focuser position
     Then the operation should fail with a not-connected error
   ```

3. **Keep unit tests for protocol and serialization.** These have no behavioral story to tell — they verify data encoding. BDD adds no value here.

4. **Keep property tests alongside BDD.** Property tests verify invariants across random inputs. BDD scenarios test specific, documented behaviors. They complement each other.

5. **Delete the migrated integration test** once the BDD equivalent is passing and covers the same behavior.

---

## 8. Naming Conventions Summary

| Element | Convention | Example |
|---|---|---|
| Feature file | `snake_case.feature`, named by concern | `safety_evaluation.feature` |
| Feature title | Title case, names the concern | `Feature: Safety evaluation rules` |
| Scenario title | Sentence case, states the outcome | `Scenario: Device starts disconnected` |
| Step definition file | `snake_case_steps.rs` | `connection_steps.rs` |
| Step function name | `snake_case`, describes the action | `fn monitoring_file_containing(...)` |
| Unit test function | `test_<component>_<behavior>` | `fn test_focuser_position_not_connected()` |
| Test data file | Descriptive name in test directory | `tests/config.json` |

---

## 9. Checklist: Adding a New Feature

When adding a new feature to a service:

1. Read the service's design document (`docs/services/<service>.md`)
2. Write feature file(s) describing the new behavior in Gherkin
3. Implement step definitions, reusing existing steps where possible
4. Add new fields to the World struct if needed
5. Write unit tests for any new protocol commands or serialization
6. Write property tests if the feature has invariants over arbitrary input
7. Run `cargo build --all --quiet --color never`, `cargo test --all --quiet --color never`, `cargo fmt`
8. Update the design document if behavior described there has changed
