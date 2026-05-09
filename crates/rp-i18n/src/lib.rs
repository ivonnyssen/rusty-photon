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
    let bare = s.split('.').next().unwrap_or("en").replace('_', "-");
    bare.parse().unwrap_or_else(|_| en())
}

fn en() -> LanguageIdentifier {
    "en".parse().expect("en is a valid langid")
}

/// Negotiate which embedded locale(s) to load, falling back to `en`.
///
/// Errors loading the chosen languages are swallowed: a missing key falls
/// through Fluent's own fallback chain (`xx-YY → xx → en`) at render time, so
/// this function only fails silently on unrecoverable I/O — and even then, the
/// pre-loaded English bundle keeps the binary functional.
pub fn select_best<A: I18nAssets>(
    loader: &FluentLanguageLoader,
    assets: &A,
    requested: &LanguageIdentifier,
) {
    let available = loader.available_languages(assets).unwrap_or_default();
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
    let _ = loader.load_languages(assets, &chosen);
}

/// One-call setup: resolve the locale, load the matching assets, register
/// the loader as the thread's [`active_loader`], and hand back an `Arc` for
/// the caller to feed into [`LocalizedParser::parse_localized`].
///
/// `loader` is the value returned by [`fluent_language_loader!`], invoked
/// at the consumer crate so the macro can read that crate's `i18n.toml`.
pub fn init<A: I18nAssets>(loader: FluentLanguageLoader, assets: &A) -> Arc<FluentLanguageLoader> {
    let requested = resolve_locale();
    select_best(&loader, assets, &requested);
    let arc = Arc::new(loader);
    ACTIVE_LOADER.with(|cell| {
        let _ = cell.set(arc.clone());
    });
    arc
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
    fn fl_active_returns_none_when_no_loader_set() {
        // ACTIVE_LOADER is per-thread; this test runs on its own thread, so
        // the cell is empty. fl_active must not panic on the empty case.
        let result = fl_active(|_| "unused".to_string());
        assert_eq!(result, None);
    }
}
