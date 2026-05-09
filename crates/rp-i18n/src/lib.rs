//! Workspace-shared Fluent loader, locale resolver, and clap-derive companion.
//!
//! See [`docs/plans/i18n.md`](../../../docs/plans/i18n.md) for the strategy
//! and [`docs/plans/i18n-cli-spike.md`](../../../docs/plans/i18n-cli-spike.md)
//! for the spike that introduces this crate.
//!
//! ## Typical use
//!
//! ```ignore
//! use clap::Parser;
//! use rust_embed::RustEmbed;
//! use rp_i18n::{fluent_language_loader, LocalizedParser};
//!
//! #[derive(RustEmbed)]
//! #[folder = "i18n/"]
//! struct Localizations;
//!
//! #[derive(Parser, LocalizedParser)]
//! #[localized(about = "cli-about")]
//! struct Args {
//!     #[arg(short, long)]
//!     #[localized(help = "cli-help-config")]
//!     config: Option<String>,
//! }
//!
//! let loader = rp_i18n::init(fluent_language_loader!(), &Localizations);
//! let args = Args::parse_localized(&loader);
//! ```
//!
//! Doctest is `ignore`d because `fluent_language_loader!` reads the consumer
//! crate's `i18n.toml` at compile time and `RustEmbed` needs a real `i18n/`
//! tree — neither exists in this crate. See `services/ppba-driver` for a
//! working consumer.

use std::cell::OnceCell;
use std::sync::Arc;

use fluent_langneg::{negotiate_languages, NegotiationStrategy};
use i18n_embed::{I18nAssets, LanguageLoader};
use unic_langid::LanguageIdentifier;

pub use i18n_embed::fluent::{fluent_language_loader, FluentLanguageLoader};
pub use i18n_embed_fl::fl;
pub use rp_i18n_derive::LocalizedParser;

/// Trait emitted by `#[derive(LocalizedParser)]`.
///
/// `parse_localized` is the loud convenience that exits the process on parse
/// errors (matching `clap::Parser::parse`); `try_parse_localized` returns
/// the error so callers can render it themselves (matching
/// `clap::Parser::try_parse`).
pub trait LocalizedParser: Sized {
    fn parse_localized(loader: &FluentLanguageLoader) -> Self;
    fn try_parse_localized(loader: &FluentLanguageLoader) -> Result<Self, clap::Error>;
}

thread_local! {
    static ACTIVE_LOADER: OnceCell<Arc<FluentLanguageLoader>> = const { OnceCell::new() };
}

/// Resolve the desired locale from environment, falling back to OS, then `en`.
///
/// Precedence:
/// 1. `RP_LOCALE`
/// 2. `LC_ALL`, `LC_MESSAGES`, `LANG`
/// 3. [`sys_locale::get_locale`]
/// 4. `en`
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
    // POSIX locales look like `de_DE.UTF-8@euro`; `LanguageIdentifier` accepts
    // only the language-region core. Strip the `.encoding` suffix, the
    // `@modifier` suffix, and any surrounding whitespace before normalising
    // `_` to `-`.
    let trimmed = s.trim();
    let no_modifier = trimmed.split('@').next().unwrap_or("");
    let no_encoding = no_modifier.split('.').next().unwrap_or("en");
    let bare = no_encoding.replace('_', "-");
    if bare.is_empty() {
        return en();
    }
    bare.parse().unwrap_or_else(|_| en())
}

fn en() -> LanguageIdentifier {
    "en".parse().expect("en is a valid langid")
}

/// Negotiate which embedded locale(s) to load, falling back to `en`.
///
/// Returns `Ok(())` when the requested locale (or its `xx → en` fallback) is
/// successfully loaded into `loader`. Returns `Err(LoadError)` when asset
/// enumeration or bundle loading fails — but the binary stays functional
/// either way because [`fluent_language_loader!`] preloads the English
/// fallback bundle at compile time, so `fl!()` calls still resolve.
///
/// Both error paths emit `tracing::warn!` so a misconfigured `i18n/` tree is
/// visible in operator logs rather than silently degrading to English.
pub fn select_best<A: I18nAssets>(
    loader: &FluentLanguageLoader,
    assets: &A,
    requested: &LanguageIdentifier,
) -> Result<(), LoadError> {
    let available = match loader.available_languages(assets) {
        Ok(langs) => langs,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "i18n: failed to enumerate embedded locales — keeping English fallback only"
            );
            return Err(LoadError::Available);
        }
    };
    let en = en();
    let chosen: Vec<LanguageIdentifier> = negotiate_languages(
        std::slice::from_ref(requested),
        &available,
        Some(&en),
        NegotiationStrategy::Filtering,
    )
    .into_iter()
    .cloned()
    .collect();
    if let Err(e) = loader.load_languages(assets, &chosen) {
        tracing::warn!(
            error = %e,
            requested = %requested,
            "i18n: failed to load negotiated locale bundle — falling back to English"
        );
        return Err(LoadError::Load);
    }
    Ok(())
}

/// Why a [`select_best`] / [`init`] call did not load the requested locale.
/// The binary continues to function via Fluent's English fallback either way;
/// this is surfaced for callers that want to log or telemeter the miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadError {
    /// `i18n_embed::I18nAssets::available_languages` failed.
    Available,
    /// `FluentLanguageLoader::load_languages` failed.
    Load,
    /// Another `init` call already populated the thread-local loader.
    AlreadyInitialized,
}

/// One-call setup: resolve the locale, load the matching assets, register
/// the loader as the thread's [`active_loader`], and hand back the `Arc`
/// alongside a status telling the caller whether the requested locale
/// actually loaded.
///
/// `loader` is the value returned by [`fluent_language_loader!`], invoked at
/// the consumer crate so the macro can read that crate's `i18n.toml`.
///
/// The returned `Arc` is always usable: Fluent's preloaded English fallback
/// keeps `fl!()` calls resolving even when the status is `Err`. Callers
/// should typically log the status **after** their tracing subscriber is
/// initialised, since `init` itself runs before logging is set up:
///
/// ```ignore
/// let (loader, i18n_status) = rp_i18n::init(fluent_language_loader!(), &Localizations);
/// let args = Args::parse_localized(&loader);
/// tracing_subscriber::fmt().with_max_level(args.log_level).init();
/// if let Err(e) = i18n_status {
///     tracing::warn!(?e, "i18n: locale negotiation degraded; running with English fallback");
/// }
/// ```
///
/// `Err(LoadError::AlreadyInitialized)` means a previous `init` call on this
/// thread already won the [`active_loader`] race — the loader returned here
/// is still freshly built, but `fl_active` will keep returning the original.
pub fn init<A: I18nAssets>(
    loader: FluentLanguageLoader,
    assets: &A,
) -> (Arc<FluentLanguageLoader>, Result<(), LoadError>) {
    let requested = resolve_locale();
    let load_status = select_best(&loader, assets, &requested);
    let arc = Arc::new(loader);
    let register_status = ACTIVE_LOADER.with(|cell| {
        if cell.set(arc.clone()).is_err() {
            Err(LoadError::AlreadyInitialized)
        } else {
            Ok(())
        }
    });
    // Surface the most informative error — a load miss matters more than a
    // double-init, since the latter is usually a refactor / test artefact.
    let status = load_status.and(register_status);
    (arc, status)
}

/// The loader registered by the most recent [`init`] call on this thread,
/// if any. Used by `clap` `value_parser` callbacks (which run inside
/// `get_matches()` and so cannot capture the loader by reference).
pub fn active_loader() -> Option<Arc<FluentLanguageLoader>> {
    ACTIVE_LOADER.with(|cell| cell.get().cloned())
}

/// Convenience for [`active_loader`] + closure: returns `None` if no loader
/// is registered, so callers can `.unwrap_or_else(|| english_fallback())`.
pub fn fl_active<F>(f: F) -> Option<String>
where
    F: FnOnce(&FluentLanguageLoader) -> String,
{
    ACTIVE_LOADER.with(|cell| cell.get().map(|loader| f(loader.as_ref())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_locale_strips_encoding_and_normalises_separator() {
        let id = parse_locale("de_DE.UTF-8");
        assert_eq!(id.language.as_str(), "de");
    }

    #[test]
    fn parse_locale_returns_en_for_garbage() {
        let id = parse_locale("not a locale");
        assert_eq!(id.language.as_str(), "en");
    }

    #[test]
    fn parse_locale_handles_bare_lang() {
        let id = parse_locale("fr");
        assert_eq!(id.language.as_str(), "fr");
    }

    #[test]
    fn parse_locale_strips_modifier_suffix() {
        // de_DE@euro: legacy POSIX modifier the LanguageIdentifier parser
        // doesn't accept on its own.
        let id = parse_locale("de_DE@euro");
        assert_eq!(id.language.as_str(), "de");
        // Region should still survive the strip.
        assert_eq!(id.region.map(|r| r.as_str().to_string()), Some("DE".into()));
    }

    #[test]
    fn parse_locale_strips_encoding_and_modifier_together() {
        let id = parse_locale("sr_RS.UTF-8@latin");
        assert_eq!(id.language.as_str(), "sr");
        assert_eq!(id.region.map(|r| r.as_str().to_string()), Some("RS".into()));
    }

    #[test]
    fn parse_locale_trims_whitespace() {
        let id = parse_locale("   de_DE.UTF-8   ");
        assert_eq!(id.language.as_str(), "de");
    }

    #[test]
    fn parse_locale_empty_string_falls_back_to_en() {
        let id = parse_locale("");
        assert_eq!(id.language.as_str(), "en");
    }

    #[test]
    fn fl_active_returns_none_when_no_loader_set() {
        // ACTIVE_LOADER is per-thread; this test runs on its own thread, so
        // the cell is empty. fl_active must not panic on the empty case.
        let result = fl_active(|_| "unused".to_string());
        assert_eq!(result, None);
    }
}
