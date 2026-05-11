# Tech spike: i18n in `ppba-driver`'s CLI via Fluent

**Date:** 2026-05-09
**Branch:** `worktree-i18n-spike`
**Parent plan:** [`docs/plans/i18n.md`](i18n.md) — workspace internationalization strategy
**Status:** in flight. Picks up §7 Phase 3 (CLI help) of the parent plan as a small, isolated probe. Not a substitute for the §7 Phase 1 dashboard spike — this answers a different question.

## 1. Goal

Prove the end-to-end ergonomics of putting Fluent + `i18n-embed` behind a `clap`-derive CLI in a single representative service, surfacing every problem that generalises to the other 8 services before we commit to a workspace-wide rollout.

**Success criteria:**

1. `LANG=de_DE.UTF-8 ppba-driver --help` renders German help text.
2. `LANG=de_DE.UTF-8 ppba-driver --log-level wat` renders a German error string.
3. `ppba-driver --help` (no env) renders English (matches today verbatim).
4. The English `.ftl` for ppba-driver is the source of truth; the German `.ftl` is LLM-bootstrapped with a `# machine-translated, needs review` header.
5. Translation verification is wired into `cargo rail run --profile commit`. Done via `crates/rusty-photon-i18n`'s `verify_translations` / `verify_translations_in_dir` (Fluent syntax + key parity + `{ $var }` placeholder parity), exercised by `services/ppba-driver/tests/translations.rs` which runs under `cargo test`, `cargo nextest`, the rail commit profile, and `bazel test //services/ppba-driver:translations`. `cargo-i18n` itself was rejected because it's a gettext-oriented build tool, doesn't have a `verify` mode, and chokes on workspace-inheritance Cargo.toml entries (`version.workspace = true`).
6. No behavioural change to anything beyond CLI help/errors — the bound port, log lines, ASCOM payloads, and `info!` startup messages stay byte-for-byte identical.

## 2. Why `ppba-driver`

| Property | Value |
|---|---|
| CLI shape | Single command, 6 long-arg flags, 1 typed `value_parser` with a user-facing error |
| Help-string count | 8 (1 `name`, 1 `about`, 6 `#[arg(/// ...)]` doc-comments) + 1 `parse_log_level` error |
| Subcommands | None |
| Locale-resolution timing | Trivial: locale is read from env before `Args::parse()`, no chicken-and-egg |
| Overlap with other in-flight work | Zero. The §7 Phase 1 dashboard spike lives in `sentinel`; this lives in `ppba-driver` |
| Generalisability | Every other "boring" service (`qhy-focuser`, `filemonitor`, `sentinel`, `plate-solver`, `calibrator-flats`, `sky-survey-camera`) has the same flat shape. Pattern transfers 1:1. `rp`'s subcommands are a follow-up |

**Rejected alternatives:** `rp` (subcommand complexity dilutes what the spike is measuring), `sentinel` (overlaps with the dashboard spike), `sky-survey-camera` (CLI is too thin — only 29 lines).

## 3. Surfaces translated in the spike

Exact `.ftl` keys, mapped to the attributes in `services/ppba-driver/src/main.rs`. Each row points at the `#[localized(...)]` attribute that wires the Fluent key onto the corresponding clap struct/field; the doc-comment immediately above each field is the English source-of-truth that survives in source for grep-ability and as a defensible fallback when the loader fails.

| Key | English source | Source location |
|---|---|---|
| `cli-about` | "ASCOM Alpaca driver for Pegasus Astro PPBA Gen2" | `#[localized(about = "cli-about")]` (struct, line 24) |
| `cli-help-config` | "Path to configuration file" | `#[localized(help = "cli-help-config")]` line 28 |
| `cli-help-port` | "Serial port path (overrides config file)" | `#[localized(help = "cli-help-port")]` line 33 |
| `cli-help-server-port` | "Server port (overrides config file)" | `#[localized(help = "cli-help-server-port")]` line 38 |
| `cli-help-enable-switch` | "Enable/disable Switch device" | `#[localized(help = "cli-help-enable-switch")]` line 43 |
| `cli-help-enable-observingconditions` | "Enable/disable ObservingConditions device" | `#[localized(help = "cli-help-enable-observingconditions")]` line 48 |
| `cli-help-log-level` | "Log level" | `#[localized(help = "cli-help-log-level")]` line 53 |
| `error-invalid-log-level` | `Invalid log level: { $value }. Use: trace, debug, info, warn, error` | `parse_log_level` body (line 57) via `rusty_photon_i18n::fl_active` |

**Explicitly NOT translated in the spike:**

- `#[command(name = "ppba-driver")]` — binary name is an identifier, not prose.
- The `info!` startup logs ("Starting PPBA driver", "Serial port: ...", "Server port: ...") — §1 of the i18n plan keeps logs English.
- `clap`'s own built-in messages ("error: unrecognized argument", "Usage:", "Options:") — these come from `clap-builtin` and require a separate `clap` locale strategy. Track as Open Question 1; do **not** block on it.
- The `expect("failed to install ...")` panics in `shutdown_signal` — these are programmer errors, not user-facing.

## 4. Architecture

### New crate: `crates/rusty-photon-i18n`

Workspace member that owns the loader, the locale-negotiation logic, and the macro re-export. §7 Phase 1 of the parent plan introduces this crate; the spike pulls it forward and uses ppba-driver as its first consumer.

```
crates/rusty-photon-i18n/
├── Cargo.toml                       # i18n-embed, fluent-langneg, sys-locale, unic-langid, clap
├── src/
│   └── lib.rs                       # resolve_locale, select_best, init,
│                                    # LoadError, fl_active, LocalizedParser trait,
│                                    # fl + fluent_language_loader re-exports
└── (no README — canonical docs live in src/lib.rs)
```

The crate is **deliberately thin**: it owns the loader factory and the locale-resolution function. The `.ftl` files for each service live in `services/<svc>/i18n/{locale}/`, embedded via `i18n-embed`'s `RustEmbed` derive at the consumer site. This matches §6 of the plan ("`i18n/{locale}/{module}.ftl` tree per service").

### Consumer wiring: `services/ppba-driver/`

```
services/ppba-driver/
├── i18n/
│   ├── en/ppba-driver.ftl           # English source-of-truth (canonical)
│   └── de/ppba-driver.ftl           # LLM-bootstrapped, "needs review"
├── i18n.toml                        # cargo-i18n manifest (fallback_language = "en")
└── src/
    ├── main.rs                      # modified: locale resolution + Command mutation
    └── (everything else unchanged)
```

### Locale-resolution algorithm

`rusty_photon_i18n::resolve_locale()` runs **before** `Args::parse()`. Order of precedence:

1. `RP_LOCALE` env var (workspace-explicit override).
2. `LC_ALL`, `LC_MESSAGES`, `LANG` env vars (POSIX).
3. `sys_locale::get_locale()` (OS-level).
4. Fallback: `en`.

Result is fed through `fluent_langneg::negotiate_languages(&requested, &available, Some(&en), Filtering::Matching)` to pick the best match from what's actually shipped in the binary. Falls back to `en` when nothing matches.

**Why before parse:** `clap`'s `--help` and parse errors exit the process before any user code runs. The locale must be known *as the `Command` is built*. There is no reliable way to set the locale via a CLI flag and also localise the help describing that flag — that's a known limitation of every i18n-clap integration. Env-only is the pragmatic answer; document in the operator runbook.

## 5. Code shape

### `crates/rusty-photon-i18n/src/lib.rs` (sketch)

```rust
use fluent_langneg::{negotiate_languages, NegotiationStrategy};
use i18n_embed::{
    fluent::FluentLanguageLoader,
    I18nAssets, LanguageLoader,
};
use unic_langid::LanguageIdentifier;

pub use i18n_embed::fluent::fluent_language_loader;
pub use i18n_embed::LanguageLoader as _LanguageLoader;
pub use i18n_embed_fl::fl;

pub fn resolve_locale() -> LanguageIdentifier {
    let preferred = std::env::var("RP_LOCALE")
        .ok()
        .or_else(|| std::env::var("LC_ALL").ok())
        .or_else(|| std::env::var("LC_MESSAGES").ok())
        .or_else(|| std::env::var("LANG").ok())
        .or_else(sys_locale::get_locale)
        .unwrap_or_else(|| "en".to_string());
    parse_locale(&preferred)
}

fn parse_locale(s: &str) -> LanguageIdentifier {
    s.split('.')
        .next()
        .unwrap_or("en")
        .replace('_', "-")
        .parse()
        .unwrap_or_else(|_| "en".parse().expect("en is a valid langid"))
}

pub fn select_best<A: I18nAssets>(
    loader: &FluentLanguageLoader,
    assets: &A,
    requested: &LanguageIdentifier,
) {
    let available = loader.available_languages(assets).unwrap_or_default();
    let en: LanguageIdentifier = "en".parse().expect("en is a valid langid");
    let chosen: Vec<LanguageIdentifier> = negotiate_languages(
        &[requested.clone()],
        &available,
        Some(&en),
        NegotiationStrategy::Filtering,
    )
    .into_iter()
    .cloned()
    .collect();
    let _ = loader.load_languages(assets, &chosen);
}
```

### `services/ppba-driver/i18n/en/ppba-driver.ftl`

```ftl
cli-about = ASCOM Alpaca driver for Pegasus Astro PPBA Gen2
cli-help-config = Path to configuration file
cli-help-port = Serial port path (overrides config file)
cli-help-server-port = Server port (overrides config file)
cli-help-enable-switch = Enable/disable Switch device
cli-help-enable-observingconditions = Enable/disable ObservingConditions device
cli-help-log-level = Log level
error-invalid-log-level = Invalid log level: { $value }. Use: trace, debug, info, warn, error
```

### `services/ppba-driver/i18n/de/ppba-driver.ftl`

```ftl
# machine-translated, needs review
cli-about = ASCOM-Alpaca-Treiber für Pegasus Astro PPBA Gen2
cli-help-config = Pfad zur Konfigurationsdatei
…
error-invalid-log-level = Ungültige Protokollstufe: { $value }. Verwende: trace, debug, info, warn, error
```

### `services/ppba-driver/src/main.rs` — diff shape

Keep the derive struct verbatim. Resolve the locale, instantiate the loader, and use `Command::mut_arg` to overwrite help strings before parse. The doc-comments stay in source as the English source-of-truth and as a defensible fallback if the loader fails.

```rust
use clap::CommandFactory;
use i18n_embed::fluent::fluent_language_loader;
use i18n_embed_fl::fl;
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "i18n/"]   // relative to the consumer crate's Cargo.toml
struct Localizations;

#[derive(Parser)]
#[command(name = "ppba-driver", version)]
struct Args { /* unchanged */ }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let loader = std::sync::Arc::new(fluent_language_loader!());
    let requested = rusty_photon_i18n::resolve_locale();
    rusty_photon_i18n::select_best(&loader, &Localizations, &requested);
    LOADER.with(|cell| { let _ = cell.set(loader.clone()); });

    let cmd = Args::command()
        .about(fl!(loader, "cli-about"))
        .mut_arg("config",                       |a| a.help(fl!(loader, "cli-help-config")))
        .mut_arg("port",                         |a| a.help(fl!(loader, "cli-help-port")))
        .mut_arg("server_port",                  |a| a.help(fl!(loader, "cli-help-server-port")))
        .mut_arg("enable_switch",                |a| a.help(fl!(loader, "cli-help-enable-switch")))
        .mut_arg("enable_observingconditions",   |a| a.help(fl!(loader, "cli-help-enable-observingconditions")))
        .mut_arg("log_level",                    |a| a.help(fl!(loader, "cli-help-log-level")));
    let matches = cmd.get_matches();
    let args = Args::from_arg_matches(&matches)?;
    /* … rest unchanged … */
}
```

**The `parse_log_level` snag** is the spike's first interesting finding: clap calls value-parsers during `get_matches()`, so the `fl!()` call inside the parser needs the loader already initialised. The clean answer is a `thread_local!` `OnceCell<Arc<FluentLanguageLoader>>` populated before `get_matches()`. **Surface this in the spike write-up — it is exactly the kind of friction we want to discover early.**

## 6. Build & CI wiring

- **Workspace `Cargo.toml`:** add `i18n-embed`, `i18n-embed-fl`, `rust-embed`, `fluent-langneg`, `sys-locale`, `unic-langid` as workspace dependencies (used by ≥2 crates: `rusty-photon-i18n` itself + `ppba-driver`, satisfying CLAUDE.md rule 10).
- **Bazel:** `CARGO_BAZEL_REPIN=1 bazel mod tidy` after the Cargo edit. `bazel build //crates/rusty-photon-i18n` + `//services/ppba-driver` validates shadow-mode.
- **Pre-push profile:** ideally add `cargo i18n verify --manifest-path services/ppba-driver/Cargo.toml` to the `commit` profile. If `cargo-i18n` is not installed in the dev environment, ship the spike without that gate and open a follow-up — `i18n-embed`'s loader fails at *binary* startup if a referenced key is missing in en, so the safety floor is intact.
- **Pre-commit hook (`.cargo-husky/hooks/pre-commit`):** unchanged.

## 7. Verification

| Check | Mechanism |
|---|---|
| `--help` renders Fluent strings, not literal Rust | Test in `services/ppba-driver/tests/cli_help.rs`: spawn `ppba-driver --help` with `LANG=C` and `LANG=de_DE.UTF-8`; assert key German phrases appear |
| `--log-level wat` renders the localised error | Same harness, capture stderr |
| Missing-key fallback: delete a key from `de.ftl`, rebuild | Manual; the `xx → en` fallback chain is documented on `select_best` in `crates/rusty-photon-i18n/src/lib.rs` (canonical crate docs — there's no README, per §4) |
| BDD harness still parses the `Bound ppba-driver server bound_addr=…` startup line | Existing `cargo nextest run -p ppba-driver --features mock` passes with no changes |
| `cargo rail run --profile commit -q` clean | Standard pre-push gate |

## 8. Decision gates (answered by the spike, fed back into `i18n.md` §7 / §8)

1. **Is `Command::mut_arg` ergonomic enough, or do we need to move to clap's builder API?** *Answered: `mut_arg` works, and follow-up work in this branch added `crates/rusty-photon-i18n-derive` as a workspace proc macro that automates the `mut_arg` calls from `#[localized(help = "key")]` attributes — see §11. Final consumer call-sites are `Args::parse_localized(&loader)`, one line, with no manual `Command::mut_arg` plumbing visible. Resolves Open Question 7 from the parent plan.*
2. **Does `cargo i18n verify` integrate cleanly with rail's `commit` profile, or do we need a separate rail surface?** Resolves part of Phase 1's CI plumbing.
3. **`.ftl`-per-service vs `.ftl`-per-surface (cli.ftl, errors.ftl, dashboard.ftl)?** The spike ships with one file per service; if it feels cramped at ~10 strings, a Phase-2 split is cheap.
4. **`RP_LOCALE` vs `RUSTY_PHOTON_LOCALE`?** Spike ships `RP_LOCALE` (shorter, matches the binary prefix). Resolves Open Question 1.
5. **Does the `parse_log_level` thread-local pattern feel acceptable, or do we move log-level validation out of clap and into post-parse code?** Surfaces the value-parser timing issue early.
6. **Server-binary size delta** — the spike measures `ls -lh target/release/ppba-driver` before/after as a sanity check. Expected ~100 KB Fluent runtime cost (per `i18n.md` §8 question 5).

## 9. Out of scope for this spike

- Translating ppba-driver's `thiserror` enum (Phase 3's other half — the `LocalizedError` presentation layer).
- Translating clap's *built-in* strings ("Usage:", "Options:", "error: …").
- Locale switching at runtime (the process must restart with a new env var — fine for CLI).
- Hosting on Weblate. The spike's `de.ftl` is LLM-bootstrapped, committed in PR.
- All other services. The spike validates the pattern in one place; other services are mechanical follow-ups.

## 10. Workspace proc macro: `rusty-photon-i18n-derive`

Added in a follow-up commit on the same branch after manual `Command::mut_arg`
chains proved repetitive. Companion crate that pairs with `clap::Parser`:

```rust
#[derive(Parser, LocalizedParser)]
#[localized(about = "cli-about")]
struct Args {
    #[arg(short, long)]
    #[localized(help = "cli-help-config")]
    config: Option<PathBuf>,
    // ...
}

#[tokio::main]
async fn main() {
    let (loader, i18n_status) = rusty_photon_i18n::init(fluent_language_loader!(), &Localizations);
    let args = Args::parse_localized(&loader);
    tracing_subscriber::fmt().with_max_level(args.log_level).init();
    if let Err(e) = i18n_status {
        tracing::warn!(?e, "i18n: locale negotiation degraded; running with English fallback");
    }
    // ...
}
```

The `#[localized(help = "...")]` attribute is per-field; `#[localized(about = "...")]`
is on the struct. Fields without `#[localized(...)]` keep clap's compile-time
default — so opting a single field out of translation costs nothing.

`rusty_photon_i18n::init` is the one-call lifecycle: it resolves the locale via
[`resolve_locale`], negotiates against the embedded `Localizations` via
[`select_best`], populates the crate-internal `ACTIVE_LOADER` thread-local
(used by `value_parser` callbacks via [`fl_active`]), and returns
`(Arc<FluentLanguageLoader>, Result<(), LoadError>)`. The `Arc` is what
`parse_localized` consumes; the `Result` reports whether the requested
locale actually loaded — log it **after** `tracing_subscriber::fmt().init()`
runs (since `init` itself executes pre-tracing, any internal warnings would
otherwise be dropped).

Net effect on the consumer: a single-page service like `ppba-driver` shrinks
the i18n-related code in `main.rs` from ~30 lines (manual `mut_arg` chain +
local thread-local + macro plumbing) to 2 (`init` + `parse_localized`).

The macro is **opt-in**: `clap::Parser` alone still works exactly as today.
Services that don't want translation pay nothing.

[`resolve_locale`]: ../../crates/rusty-photon-i18n/src/lib.rs
[`select_best`]: ../../crates/rusty-photon-i18n/src/lib.rs
[`fl_active`]: ../../crates/rusty-photon-i18n/src/lib.rs

## 11. Bazel binary i18n: diagnosis and fix

The first cut of this spike shipped a Bazel-built `ppba-driver_mock` binary that printed `No localization for id: "cli-about"` for every key. The cargo path always worked. This section captures the root cause and the one-line fix that is now in place.

### Root cause

Not the proc-macro sandbox, not `compile_data`, not `walkdir`. The bug is `cfg(debug_assertions)`.

`rust-embed` ships with two implementations behind the same derive:

- **Embedded** (`#[cfg(not(debug_assertions))]`): bake the directory tree into the binary at compile time.
- **Dynamic** (`#[cfg(debug_assertions)]`, default in dev builds): bake the *path* into the binary and read files from disk at *runtime*.

Under `cargo build` (debug, `debug_assertions=on`), the dynamic impl runs at runtime and reads `$CARGO_MANIFEST_DIR_AT_COMPILE_TIME/i18n/`. That path is the source tree, which is reachable when running from the cargo target dir, so it appears to work.

Under `bazel build` (default `--compilation_mode=fastbuild`, also `debug_assertions=on` per `rules_rust`'s default toolchain), the dynamic impl runs at runtime and reads the value `rules_rust` set as `CARGO_MANIFEST_DIR` — a sandbox path of the form `${pwd}/services/ppba-driver` that existed during compilation but does not exist at runtime when the binary runs from `bazel-bin/...`. `walkdir` returns zero files, the embed is empty, every `fl!()` falls through to "No localization for id: …".

`bazel build --compilation_mode=opt` masked the bug — `debug_assertions=off` flips to the embedded impl, which uses the same `walkdir` call but at *compile time*, where the sandbox path is valid and `compile_data` has the `.ftl` tree in place.

### Fix

Enable `rust-embed`'s `debug-embed` feature workspace-wide, which makes the compile-time embed unconditional regardless of `debug_assertions`:

```toml
# Cargo.toml [workspace.dependencies]
rust-embed = { version = "8", features = ["debug-embed"] }
```

This is also the more correct semantic choice: a translatable CLI binary should ship with its translations, not lazy-load them from a path that may not exist at runtime. The previous behaviour where a moved cargo-built binary would silently lose its translations is fixed by the same flag.

### Verified after the fix

- `bazel build //services/ppba-driver:ppba-driver_mock` (default fastbuild) followed by `LANG=de_DE.UTF-8 ./bazel-bin/services/ppba-driver/ppba-driver_mock --help` renders German.
- `LANG=C ./bazel-bin/services/ppba-driver/ppba-driver_mock --help` renders English.
- `LANG=de_DE.UTF-8 ./bazel-bin/services/ppba-driver/ppba-driver_mock --log-level wat` renders the German `error-invalid-log-level` string.
- `bazel test //services/ppba-driver:all` and `bazel test //services/ppba-driver:bdd` pass.
- Cargo `cli_help` + `translations` integration tests pass; `cargo rail run --profile commit -q` is clean.

## 12. What this spike does NOT settle

- The UX-stack choice (`i18n.md` §3): the CLI surface is identical regardless of which UI tech wins.
- Sourcing graduation (`i18n.md` §4): one language pair (en/de) is too small to test the Weblate workflow.
- Translator UX with `.ftl` (Phase 1's decision gate): this spike has only the maintainer editing the file. The dashboard spike is the better venue for that gate.
