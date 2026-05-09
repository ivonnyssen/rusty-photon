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
5. `cargo i18n verify` (or equivalent) is wired into `cargo rail run --profile commit` *or* an explicit follow-up issue is opened explaining what blocked it.
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

Exact `.ftl` keys, mapped to the strings in `services/ppba-driver/src/main.rs`:

| Key | English source | Source location |
|---|---|---|
| `cli-about` | "ASCOM Alpaca driver for Pegasus Astro PPBA Gen2" | `#[command(about = ...)]` line 17 |
| `cli-help-config` | "Path to configuration file" | doc-comment line 20 |
| `cli-help-port` | "Serial port path (overrides config file)" | line 24 |
| `cli-help-server-port` | "Server port (overrides config file)" | line 28 |
| `cli-help-enable-switch` | "Enable/disable Switch device" | line 32 |
| `cli-help-enable-observingconditions` | "Enable/disable ObservingConditions device" | line 36 |
| `cli-help-log-level` | "Log level" | line 40 |
| `error-invalid-log-level` | `Invalid log level: { $value }. Use: trace, debug, info, warn, error` | `parse_log_level` line 47 |

**Explicitly NOT translated in the spike:**

- `#[command(name = "ppba-driver")]` — binary name is an identifier, not prose.
- The `info!` startup logs ("Starting PPBA driver", "Serial port: ...", "Server port: ...") — §1 of the i18n plan keeps logs English.
- `clap`'s own built-in messages ("error: unrecognized argument", "Usage:", "Options:") — these come from `clap-builtin` and require a separate `clap` locale strategy. Track as Open Question 1; do **not** block on it.
- The `expect("failed to install ...")` panics in `shutdown_signal` — these are programmer errors, not user-facing.

## 4. Architecture

### New crate: `crates/rp-i18n`

Workspace member that owns the loader, the locale-negotiation logic, and the macro re-export. §7 Phase 1 of the parent plan introduces this crate; the spike pulls it forward and uses ppba-driver as its first consumer.

```
crates/rp-i18n/
├── Cargo.toml                       # i18n-embed, fluent-langneg, sys-locale, unic-langid
├── src/
│   └── lib.rs                       # resolve_locale, select_best, fl re-export
└── README.md
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

`rp_i18n::resolve_locale()` runs **before** `Args::parse()`. Order of precedence:

1. `RP_LOCALE` env var (workspace-explicit override).
2. `LC_ALL`, `LC_MESSAGES`, `LANG` env vars (POSIX).
3. `sys_locale::get_locale()` (OS-level).
4. Fallback: `en`.

Result is fed through `fluent_langneg::negotiate_languages(&requested, &available, Some(&en), Filtering::Matching)` to pick the best match from what's actually shipped in the binary. Falls back to `en` when nothing matches.

**Why before parse:** `clap`'s `--help` and parse errors exit the process before any user code runs. The locale must be known *as the `Command` is built*. There is no reliable way to set the locale via a CLI flag and also localise the help describing that flag — that's a known limitation of every i18n-clap integration. Env-only is the pragmatic answer; document in the operator runbook.

## 5. Code shape

### `crates/rp-i18n/src/lib.rs` (sketch)

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
#[folder = "../i18n/"]
struct Localizations;

#[derive(Parser)]
#[command(name = "ppba-driver", version)]
struct Args { /* unchanged */ }

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let loader = std::sync::Arc::new(fluent_language_loader!());
    let requested = rp_i18n::resolve_locale();
    rp_i18n::select_best(&loader, &Localizations, &requested);
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

- **Workspace `Cargo.toml`:** add `i18n-embed`, `i18n-embed-fl`, `rust-embed`, `fluent-langneg`, `sys-locale`, `unic-langid` as workspace dependencies (used by ≥2 crates: `rp-i18n` itself + `ppba-driver`, satisfying CLAUDE.md rule 10).
- **Bazel:** `CARGO_BAZEL_REPIN=1 bazel mod tidy` after the Cargo edit. `bazel build //crates/rp-i18n` + `//services/ppba-driver` validates shadow-mode.
- **Pre-push profile:** ideally add `cargo i18n verify --manifest-path services/ppba-driver/Cargo.toml` to the `commit` profile. If `cargo-i18n` is not installed in the dev environment, ship the spike without that gate and open a follow-up — `i18n-embed`'s loader fails at *binary* startup if a referenced key is missing in en, so the safety floor is intact.
- **Pre-commit hook (`.cargo-husky/hooks/pre-commit`):** unchanged.

## 7. Verification

| Check | Mechanism |
|---|---|
| `--help` renders Fluent strings, not literal Rust | Test in `services/ppba-driver/tests/cli_help.rs`: spawn `ppba-driver --help` with `LANG=C` and `LANG=de_DE.UTF-8`; assert key German phrases appear |
| `--log-level wat` renders the localised error | Same harness, capture stderr |
| Missing-key fallback: delete a key from `de.ftl`, rebuild | Manual; doc the fallback behaviour ("`xx → en`") in `crates/rp-i18n/README.md` |
| BDD harness still parses the `Bound ppba-driver server bound_addr=…` startup line | Existing `cargo nextest run -p ppba-driver --features mock` passes with no changes |
| `cargo rail run --profile commit -q` clean | Standard pre-push gate |

## 8. Decision gates (answered by the spike, fed back into `i18n.md` §7 / §8)

1. **Is `Command::mut_arg` ergonomic enough, or do we need to move to clap's builder API?** The spike resolves Open Question 7 from the plan operationally.
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

## 10. What this spike does NOT settle

- The UX-stack choice (`i18n.md` §3): the CLI surface is identical regardless of which UI tech wins.
- Sourcing graduation (`i18n.md` §4): one language pair (en/de) is too small to test the Weblate workflow.
- Translator UX with `.ftl` (Phase 1's decision gate): this spike has only the maintainer editing the file. The dashboard spike is the better venue for that gate.
