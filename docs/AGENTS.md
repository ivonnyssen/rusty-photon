# Agents and human operators MUST follow the rules below

1. You MUST read the relevant documentation before starting work:
   a. Read the design document for any service you are modifying (docs/services/<service>.md). For instance, filemonitor's design document is located in docs/services/filemonitor.md.
   b. Read the skill document for the type of task you are performing (docs/skills/). Specifically:
      - Developing a new feature or service: read docs/skills/development-workflow.md
      - Writing or modifying tests: read docs/skills/testing.md
      - Pushing code or running CI checks: read docs/skills/pre-push.md

2. You MUST ALWAYS update the appropriate README and / or the appropriate design document when you make a change to a service and the change is impacting what is stated in these documents. If in doubt, re-read the docs to evaluate impact.

3. You MUST use `cargo run` when you start any service for testing.

4. You MUST ALWAYS run `cargo rail run --profile commit -q` and `cargo fmt` before committing your work and fix all errors and warnings from the change you've made. The `commit` profile (defined in `.config/rail.toml`) runs `cargo check` and `cargo nextest run` against only the packages affected by your changes vs. the merge base, with `--locked --all-features --all-targets` baked in so feature-gated test code (e.g. `#[cfg(feature = "mock")]`) is exercised and feature-unification errors on test/bench/example targets are caught. Use `cargo rail plan --merge-base` to preview which packages would be affected. Note: rail's `build` surface is hardcoded to `cargo check` (a faster proxy for codegen-free type-checking); CI itself runs `cargo build` for the same flags, so a final `cargo build` is still worth running before you push if your change touches linker-visible code.

   **Requires `cargo-nextest`** (`cargo install cargo-nextest --locked`). Without nextest, rail's test surface falls back to plain `cargo test` and the profile's `--all-features` flag silently lands in the test-binary args slot instead of cargo's — mock-gated code will not be compiled and you will pass locally but fail CI.

   If `cargo-rail` is not available, fall back to `cargo build --all --all-targets --all-features --locked --quiet --color never` and `cargo nextest run --locked --all-features --all-targets --color never`. See docs/skills/pre-push.md for the full CI quality-gate suite.

   A Bazel build system is being introduced alongside Cargo (see `docs/plans/bazel-migration.md`). Cargo remains the canonical build during the migration; Bazel runs in shadow mode and is not a required pre-push step yet. If you want to exercise it, `bazel test //...` runs all non-`requires-cargo`, non-BDD targets.

5. You MUST NEVER commit to the main branch of the git repository. ALL work MUST happen on a branch. Before making any code changes, verify you are on a feature branch. If on main, create and switch to an appropriate feature branch first. Use appropriate naming for branches such as `feature/new_feature_name` or `chore/update_dependency_x`.

6. You MUST commit changes summarizing all the changes since the last commit. For the author of the commit, use the configured username in git with ' ($AI_AGENT_NAME)' appended and the user email. For example, `git commit --author="John Doe (Kiro CLI) <john@email.com>"` if you are Kiro or `git commit --author="John Doe (Claude Code) <john@email.com>"` if you are claude code.

7. When working on unit tests, you SHOULD prefer tests that will fail with clear errors (e.g. use `result.unwrap()`, instead of `assert!(result.is_ok())`). See docs/skills/testing.md for the complete testing guide.

8. You SHOULD use test that test the smallest amount of functionality possible, while still being comprehensive in aggregate.

9. You MUST use `debug!()` log messages throughout. Only use `info!()` log messages where users will derive clear advantage from them when using the services, such as `Service started succesfully`.

10. You MUST add dependencies to the workspace Cargo.toml when more than one service has the same dependency. Cargo.toml and Cargo.lock remain the single source of truth for dependency versions; Bazel's `crate_universe` reads them. After adding a crates.io dep, run `CARGO_BAZEL_REPIN=1 bazel mod tidy` to refresh `MODULE.bazel.lock` before committing.

11. You MUST persist project-wide knowledge (design decisions, motivations, conventions) in the repository documentation (docs/, README.md, ADRs) rather than in local agent memory. This ensures all operators and machines share the same context.
