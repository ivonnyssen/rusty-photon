# Skill: Writing and Organizing Tests

## When to Read This

- Before writing any new tests (unit, BDD, or property-based)
- Before adding a new feature to a service (see the checklist in Section 9)
- Before migrating existing integration tests to BDD

## Prerequisites

- Read the service's design document (`docs/services/<service>.md`) per AGENTS.md Rule 1a
- Familiarity with the Rust test ecosystem (`cargo test`, `#[test]`, `#[tokio::test]`)
- For BDD tests: the `cucumber` crate (cucumber-rs)

---

## Procedure

### 1. Test Pyramid: Which Test Type to Use

The project uses four testing layers. Each serves a distinct purpose.

#### 1.1 BDD Tests (Feature Files + Cucumber)

**Purpose:** Living specifications. These are the primary test type for service behavior.

**Why this matters:** Feature files are the canonical contract for
everyone outside the service team -- plugin authors, frontend developers,
reviewers, integrators. When someone asks "what does this endpoint
return?" or "what is the wire format for X?", the answer should be a
feature file, not the Rust source. This is what makes BDD primary, not
optional: the test suite **is** the spec, and a reader should be able
to learn the system's behavior from `tests/features/` alone. Every
assertion in a scenario is also documentation -- write the scenarios
with that in mind (see §2.5 on keeping contract constants visible in
the feature file rather than buried in step code).

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

#### 1.2 Unit Tests (Rust `#[test]` / `#[tokio::test]`)

**Purpose:** Fast, focused verification of internal components.

**Use unit tests for:**
- Protocol parsing and serialization (in `src/protocol.rs` `#[cfg(test)]` module)
- Error type conversions and Display implementations (in `src/error.rs` `#[cfg(test)]` module)
- Configuration defaults and deserialization (in `src/config.rs` `#[cfg(test)]` module)
- Pure functions and data transformations
- In-source `#[cfg(test)]` modules for module-private logic

#### 1.3 Property-Based Tests (proptest)

**Purpose:** Discover edge cases through randomized input.

**Use property tests for:**
- Determinism invariants (same input always produces same output)
- Robustness (no panics on arbitrary input)
- Round-trip properties (serialize then deserialize returns original)
- Domain invariants that should hold for all inputs

#### 1.4 ConformU Integration Tests

**Purpose:** ASCOM Alpaca protocol compliance.

**Use ConformU tests for:**
- Verifying a service conforms to the ASCOM Alpaca standard
- These are always `#[ignore]` and run manually or in dedicated CI

---

### 2. BDD Feature File Rules

These are the most important rules. Feature files are both tests and documentation.

#### 2.1 One Feature File Per Concern

Organize feature files by functional area, not by implementation module. Each feature file should answer the question: *"What does the system do regarding [concern]?"*

**Good file names (concern-oriented):**
- `safety_evaluation.feature` -- How safety rules are evaluated
- `connection_lifecycle.feature` -- How the device connects and disconnects
- `file_polling.feature` -- How file changes are detected
- `configuration.feature` -- How configuration is loaded and validated

**Bad file names (implementation-oriented):**
- `device_tests.feature` -- Too vague
- `serial_manager.feature` -- Names an internal component, not a behavior
- `misc.feature` -- No organizing principle

#### 2.2 Feature Descriptions State the Contract

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

#### 2.3 Scenarios Describe Outcomes, Not Procedures

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

#### 2.4 Use Scenario Outlines for Parameterized Behavior

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

#### 2.5 Make Contract Constants Explicit in Steps

Step expressions should expose the values being checked, not hide them
behind opaque adjectives. A reader of the feature file should learn the
contract -- specific numbers, field names, status codes, wire-format
positions -- without opening any Rust code. This is what makes feature
files documentation; an opaque `should be valid` step is a hole in the
spec.

**Bad -- the contract lives in the step implementation, invisible to a reader:**
```gherkin
Then the response should have a valid ImageBytes header
```

**Good -- the constants live in the feature file, where plugin authors and reviewers can see them:**
```gherkin
Then the image pixels header should match these constants (i32 little-endian):
  | field                     | offset | value |
  | metadata_version          | 0      | 1     |
  | data_start                | 16     | 44    |
  | image_element_type        | 20     | 2     |
  | transmission_element_type | 24     | 8     |
  | rank                      | 28     | 2     |
```

A single step definition iterates the table, parses each value at the
declared offset, and asserts equality. The spec is now legible to a
plugin or frontend author without `cargo doc` or grepping the source.

**When to use a Gherkin data table:**
- Validating a wire-format header, status-code matrix, or any structured
  output where the relationship between fields is the contract.
- Parameterizing the same assertion shape across many fields.

**When literal step text is enough:** for one or two values, a plain
step is more readable than a one-row table -- `Then the response status
should be 200` is already explicit. The same principle applies:
`Then bitpix should be 16` beats `Then bitpix should be valid for U16`,
because "valid" forces the reader into the step source to recover the
meaning.

#### 2.6 Use `@serial` Tag for Tests With Side Effects

Tag features or scenarios with `@serial` when they depend on timing, shared resources, or file I/O that cannot safely run in parallel.

```gherkin
@serial
Feature: File content polling
```

#### 2.7 Use `@wip` Tag for Scenarios Without Implementation Yet

This project's design-first / test-first workflow (see
[development-workflow.md](development-workflow.md)) sometimes requires
landing BDD scenarios on a feature branch *before* the production code
that makes them pass exists. The `@wip` tag lets such scenarios live in
the repo as durable design artifacts without breaking the green-suite
invariant.

```gherkin
@wip
Feature: Basic image measurement tool
  ...
```

The runner in `bdd.rs` filters scenarios tagged `@wip` (at feature or
scenario level) out of the default suite. Once the corresponding
implementation lands, **remove the tag** in the same commit that turns
the scenarios on for real.

To enable this in a service's `bdd.rs`, swap `.run_and_exit("tests/features")`
for `.filter_run("tests/features", filter_fn)`:

```rust
.filter_run("tests/features", |feat, _rule, sc| {
    let is_wip = feat.tags.iter().any(|t| t == "wip" || t == "@wip")
        || sc.tags.iter().any(|t| t == "wip" || t == "@wip");
    !is_wip
})
```

Use `@wip` only for not-yet-implemented behavior. A scenario that fails
intermittently belongs in an issue, not behind `@wip`. A scenario that
documents a deferred feature you are not actively working on belongs in
a follow-up ticket, not in the repo with `@wip`.

#### 2.8 Avoid Gherkin Parser Pitfalls

These are known issues with the Gherkin parser used by cucumber-rs:

- **Do NOT start description lines with `Rule`** -- it is a Gherkin 6+ keyword and will be parsed as structure, not text.
- **Do NOT use `|` in step text** -- it is the table delimiter. Use symbolic names mapped in step definitions instead.
- **Regex patterns go in step definitions, not feature files.** Use human-readable names in features (e.g., `"safe_or_ok"`) mapped to actual patterns in code via a resolver function.

---

### 3. Step Definition Rules

#### 3.1 Organize Steps by Concern, Matching Feature Files

Each feature file should have a corresponding step definition module. Steps that are shared across features (like connection steps used in both `connection_lifecycle.feature` and `file_polling.feature`) go in the module matching their primary concern.

```
features/                        steps/
  configuration.feature    ->      config_steps.rs
  connection_lifecycle.feature ->  connection_steps.rs
  safety_evaluation.feature  ->   safety_steps.rs
  file_polling.feature       ->   polling_steps.rs
  concurrency.feature        ->   concurrency_steps.rs
```

#### 3.2 Steps Must Be Reusable Across Scenarios

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

#### 3.3 Use `expect()` in Given/When Steps, `assert!` in Then Steps

- **Given** steps set up preconditions. If setup fails, the test infrastructure is broken -- use `expect()` or `unwrap()` to fail fast with a clear message.
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

#### 3.4 Use a Pattern Resolver for Complex Values

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

#### 3.5 Distinguish "I do X" from "I try to do X"

Use separate step definitions for actions expected to succeed vs. actions expected to fail:

- `When I connect the device` -- calls `unwrap()`, fails the test if the action fails
- `When I try to connect the device` -- captures the error into `world.last_error`

This makes the intent clear in both the feature file and the step definition.

---

### 4. World Struct Rules

#### 4.1 Use `Option<T>` for State That Builds Incrementally

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

#### 4.2 Put Setup Helpers on the World Struct

Common setup logic (creating temp files, building configs, constructing devices) should be methods on the World struct, not free functions in step files. This keeps step definitions thin.

```rust
impl MyWorld {
    pub fn create_temp_file(&mut self, content: &str) -> PathBuf { ... }
    pub fn build_config(&self, file_path: PathBuf) -> Config { ... }
    pub fn build_device(&mut self) { ... }
}
```

#### 4.3 Use `TempDir` for File Lifecycle

The `tempfile::TempDir` stored in the World struct ensures automatic cleanup when each scenario ends. Never use hardcoded paths for test files.

#### 4.4 Wrap Devices in `Arc` for Concurrency Scenarios

Concurrency scenarios spawn multiple async tasks that all need access to the device. Store devices as `Arc<Device>` in the World struct from the start. This avoids needing different World types for concurrent vs. sequential scenarios.

---

### 5. BDD Test Infrastructure Rules

#### 5.1 Shared Infrastructure: `bdd-infra` Crate

The `crates/bdd-infra` crate provides shared process lifecycle management for all
services' BDD and integration tests. It eliminates per-service duplication of
binary discovery, spawning, port parsing, and graceful shutdown logic.

**Key types (default feature set):**

- `ServiceHandle` — spawns a service binary, parses its bound port from stdout,
  provides `stop()` for graceful SIGTERM shutdown, and drains stdout to prevent
  pipe deadlocks. On `Drop`, sends a best-effort SIGTERM.
- `parse_bound_port()` — standalone function also usable by ConformU tests.

**Optional `rp-harness` feature** — adds the higher-level helpers needed when a
test spawns `rp` alongside OmniSim and/or an orchestrator plugin:

- `OmniSimHandle` — singleton Alpaca simulator shared across scenarios.
- `RpConfigBuilder` + `CameraConfig` / `FilterWheelConfig` /
  `CoverCalibratorConfig` — fluent builder that emits rp's JSON config.
- `start_rp`, `wait_for_rp_healthy`, `write_temp_config_file`,
  `sibling_service_dir` — launch helpers.
- `WebhookReceiver`, `TestOrchestrator`, `OrchestratorBehavior` — in-process
  plugin stand-ins.
- `McpTestClient` — persistent rmcp client for calling rp's MCP tools.

Turn the feature on **only** for tests that actually spawn rp. Services whose
BDD tests only need `ServiceHandle` (filemonitor, qhy-focuser, ppba-driver,
sentinel, …) should leave the default features so they don't compile axum,
reqwest, and rmcp transitively.

```toml
# rp's own tests and any rp-client plugin's tests:
bdd-infra = { workspace = true, features = ["rp-harness"] }

# Services whose tests only spawn themselves:
bdd-infra = { workspace = true }
```

**Convention: per-plugin BDD suites.** End-to-end tests for an rp orchestrator
or event plugin live in that plugin's own `services/<plugin>/tests/` tree, not
in `services/rp/tests/`. Each plugin owns a small world type that embeds the
handles it needs and calls `rp_harness::start_rp(&config)` — the helper
derives `RP_BINARY` from the package name, so nothing needs to know where
`services/rp/` lives on disk. This keeps rp's test run time bounded as more
plugins land — `cargo-rail` only re-runs the plugin whose code changed.

**Usage in test code:**

```rust
use bdd_infra::ServiceHandle;

let handle = ServiceHandle::start(env!("CARGO_PKG_NAME"), &config_path).await;
// handle.port, handle.base_url are available
handle.stop().await;
```

The `env!("CARGO_PKG_NAME")` macro resolves to the calling service's package
name at compile time. `bdd-infra` derives the binary discovery env var from
it: `rp` → `RP_BINARY`, `ppba-driver` → `PPBA_DRIVER_BINARY`, etc.

**Binary discovery order:**

1. Explicit env var `{PACKAGE_UPPER_SNAKE}_BINARY` (e.g., `FILEMONITOR_BINARY=/path/to/bin`).
2. `$CARGO_TARGET_DIR/debug/<pkg>` (or `$CARGO_LLVM_COV_TARGET_DIR/debug/<pkg>` under
   `cargo llvm-cov`) when either env var is set. If `CARGO_BUILD_TARGET` is also set,
   the triple is inserted: `.../<triple>/debug/<pkg>`. When one of these env vars is
   set we look **only** there — falling through to step 3 could silently pick up a
   stale, non-instrumented binary and skip coverage data collection.
3. Walking up from the current directory looking for `target/debug/<pkg>`.
   `cargo test -p <pkg>` runs tests with the cwd at the package dir, so the
   workspace `target/` is typically one level up.

If none match, the call panics with a diagnostic pointing at the fix.

**Pre-build requirement.** BDD tests do not compile the service binary
themselves — that's the job of `cargo build`. Always build with
`--all-features` (matching CI), so feature-gated paths like `ppba-driver`'s
`mock` hardware compile in:

```
cargo build --all-features --all-targets -p <pkg>
cargo test  --all-features --test bdd      -p <pkg>
```

`cargo rail run --merge-base` pre-builds the affected packages, as does
`.github/workflows/test.yml`.

**Port discovery:** All services print `bound_addr=<host>:<port>` to stdout when
they bind. The parser looks for `bound_addr=` in any line (the human-readable
prefix before it can vary per service).

#### 5.2 Entry Point Structure

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

Services that spawn rp (plugin workflows) import shared helpers from
`bdd_infra::rp_harness` directly — there is no `steps/infrastructure.rs`
re-export layer.

The `bdd.rs` entry point uses `#[path = "..."]` imports because test crate roots see siblings, not children.

**BDD tests that spawn child processes** (i.e. use `ServiceHandle`) **must** use the
`bdd_infra::bdd_main!` macro instead of a hand-written `#[tokio::main] async fn main()`.
The macro expands to an empty `fn main() {}` under Miri, because Miri does not
support the `pidfd_spawnp` FFI that tokio uses for process spawning on Linux.
BDD tests that are purely in-process (e.g. qhy-focuser with mock serial ports)
do not need the macro.

```rust
#[path = "bdd/world.rs"]
mod world;

#[path = "bdd/steps/mod.rs"]
mod steps;

bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::MyWorld;

    MyWorld::cucumber()
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    if let Some(handle) = world.service_handle.as_mut() {
                        handle.stop().await;
                    }
                }
            })
        })
        .run_and_exit("tests/features")
        .await;
}
```

If your `World` holds an `McpTestClient` (or any other long-lived
streaming client), the `after` hook needs an extra step to avoid silently
losing BDD coverage — see [§5.4](#54-drop-mcp--streaming-clients-before-stopping-the-service).

#### 5.3 Register in Cargo.toml

```toml
[dev-dependencies]
bdd-infra = { workspace = true }

[[test]]
name = "bdd"
harness = false
```

No per-service metadata is required — `bdd-infra` derives everything from the
package name passed to `ServiceHandle::start`.

#### 5.4 Drop MCP / streaming clients before stopping the service

**Critical for coverage.** If your `World` holds an `McpTestClient` (or any
client that keeps a long-lived HTTP/streaming connection to a child service),
you **must** drop it *before* calling `ServiceHandle::stop()` in the cucumber
`after` hook. Otherwise the service hangs in graceful shutdown, gets SIGKILLed
after `stop()`'s 5-second timeout, and skips its `atexit` handlers — which
means **no `.profraw` is written and BDD coverage is silently lost**.

The mechanism: `rmcp`'s `StreamableHttpClientTransport` opens a long-lived
HTTP request to the server's `/mcp` endpoint. axum's
`with_graceful_shutdown` waits for in-flight connections to close before
returning. As long as the client is alive, the connection stays open, axum
never returns, the service can't run `atexit`, and SIGKILL claims the process
along with its in-memory LLVM coverage counters.

```rust
bdd_infra::bdd_main! {
    use cucumber::World as _;
    use world::MyWorld;

    MyWorld::cucumber()
        .after(|_feature, _rule, _scenario, _finished, maybe_world| {
            Box::pin(async move {
                if let Some(world) = maybe_world {
                    // Drop streaming clients FIRST so the server's graceful
                    // shutdown can complete. Skipping this turns BDD
                    // subprocess coverage silently into 0%.
                    world.mcp_client = None;
                    if let Some(handle) = world.service_handle.as_mut() {
                        handle.stop().await;
                    }
                }
            })
        })
        .filter_run("tests/features", |_, _, _| true)
        .await;
}
```

The same applies to any other long-lived client connection (custom WebSocket
transports, gRPC streams, etc.) — drop the client before stopping the
service it talks to.

**When this does NOT apply:** plugin BDD harnesses like `calibrator-flats`
where the MCP client lives *inside* the plugin's child process (talking to
rp), not in the BDD test process. Those harnesses just stop the plugin
first, then rp — when the plugin shuts down it closes its own MCP client
gracefully, freeing rp's connection.

**How to spot a regression of this issue:** `mcp.rs` (or any tool-dispatch
module) shows much higher coverage from unit tests than from BDD — e.g. a
big gap between BDD-only coverage and combined coverage. Confirm by
temporarily replacing `debug!("did not exit after SIGTERM, sending SIGKILL", ...)`
in `bdd-infra` with `eprintln!` and re-running BDD with stderr captured. A
SIGKILL count > 0 means the lifecycle ordering is wrong somewhere.

---

### 6. Unit Test Rules

These rules apply to traditional `#[test]` and `#[tokio::test]` tests.

#### 6.1 One Test Function Per Behavior

Each test should verify exactly one behavior. Name the test `test_<component>_<behavior>`:

```rust
#[tokio::test]
async fn test_focuser_position_not_connected() { ... }

#[tokio::test]
async fn test_focuser_move_negative_position_rejected() { ... }
```

#### 6.2 Use `unwrap()` Over `assert!(result.is_ok())`

Per AGENTS.md Rule 7: prefer tests that fail with clear errors. `unwrap()` produces a message showing **what** the error was. `assert!(result.is_ok())` just says `false`.

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

#### 6.3 Test Both Success and Error Paths

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

#### 6.4 Test File Organization

- `src/<module>.rs` `#[cfg(test)] mod tests` -- Unit and mock-based component tests (inline with source)
- `test_cli.rs` -- CLI argument parsing
- `test_server.rs` / `test_lib.rs` -- Server integration tests
- `conformu_integration.rs` -- ASCOM conformance (always `#[ignore]`)

#### 6.5 Mock Infrastructure Lives in Test Files

Hand-written mocks (`MockSerialReader`, `MockSerialWriter`, `MockSerialPortFactory`) are defined in the test files that use them. They are NOT feature-gated. The `#[cfg(feature = "mock")]` flag is reserved for the feature-gated `MockSerialPortFactory` in `src/` used by ConformU and server tests.

#### 6.6 Serialize Server Tests

Server integration tests that bind to ports must use a static `Mutex<()>` to prevent parallel execution conflicts with the discovery service:

```rust
static SERVER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[tokio::test]
async fn test_server_starts() {
    let _lock = SERVER_LOCK.lock().unwrap();
    // ... start server on port 0 ...
}
```

#### 6.7 Mock Strategy: Hand-Written vs mockall

The project uses two mock strategies depending on the use case:

**Hand-written mocks** — Use for stateful device simulators that maintain internal
state across multiple calls. These mocks simulate hardware behavior: they process
commands, maintain device state (temperature, position, voltage), and return
responses from a queue. mockall cannot express this kind of stateful simulation.

Examples: `MockSerialPortFactory` in ppba-driver and qhy-focuser, which simulate
serial port communication with response queues and device state machines.

**mockall (`#[automock]`)** — Use for service-boundary traits where you need simple
"expect this call, return this value" behavior. These are thin abstractions over
external APIs (HTTP clients, DNS providers, ACME protocol) where the mock just
needs to return canned responses or verify call arguments.

Examples: `DnsProvider` trait in rp-tls (mocks Cloudflare API), `AcmeClient` trait
in rp-tls (mocks Let's Encrypt ACME protocol).

**How to use mockall with async traits:**

Place `#[automock]` before `#[async_trait]` on the trait definition, gated to
test builds only:

```rust
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait MyService: Send + Sync {
    async fn do_something(&self, input: String) -> Result<String>;
}
```

In tests, configure expectations on the generated `MockMyService`:

```rust
let mut mock = MockMyService::new();
mock.expect_do_something()
    .withf(|input| input == "expected")
    .returning(|_| Ok("result".to_string()));
```

**Rule of thumb:** If the mock needs internal state or command processing logic,
hand-write it. If it wraps an external API and just needs call/return behavior,
use mockall.

---

### 7. Migration Strategy: From Integration Tests to BDD

When migrating existing integration tests to BDD:

1. **Group related tests into features.** Multiple `test_device_*` functions testing connection become scenarios in `connection_lifecycle.feature`.

2. **Extract the behavioral pattern.** A test like `test_focuser_position_not_connected` becomes:
   ```gherkin
   Scenario: Position read fails when disconnected
     Given a device that is not connected
     When I try to read the focuser position
     Then the operation should fail with a not-connected error
   ```

3. **Keep unit tests for protocol and serialization.** These have no behavioral story to tell -- they verify data encoding. BDD adds no value here.

4. **Keep property tests alongside BDD.** Property tests verify invariants across random inputs. BDD scenarios test specific, documented behaviors. They complement each other.

5. **Delete the migrated integration test** once the BDD equivalent is passing and covers the same behavior.

---

### 8. Naming Conventions Summary

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

### 9. Checklist: Adding a New Feature

When adding a new feature to a service:

1. Read the service's design document (`docs/services/<service>.md`)
2. Write feature file(s) describing the new behavior in Gherkin
3. Implement step definitions, reusing existing steps where possible
4. Add new fields to the World struct if needed
5. Write unit tests for any new protocol commands or serialization
6. Write property tests if the feature has invariants over arbitrary input
7. Run `cargo build --all --quiet --color never`, `cargo test --all --quiet --color never`, `cargo fmt`
8. Update the design document if behavior described there has changed

---

## References

- [AGENTS.md](../AGENTS.md) -- Rule 7 (prefer `unwrap()`) and Rule 8 (test smallest functionality)
- [Pre-push skill](pre-push.md) -- Running the full CI quality-gate suite before pushing
- [ASCOM Alpaca reference](../references/ascom-alpaca.md) -- Protocol details for ASCOM device tests
