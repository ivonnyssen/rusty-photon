//! Workspace-shared Fluent loader and locale resolver.
//!
//! See [`docs/plans/i18n.md`](../../../docs/plans/i18n.md) for the strategy
//! and [`docs/plans/i18n-cli-spike.md`](../../../docs/plans/i18n-cli-spike.md)
//! for the spike that introduces this crate.

use fluent_langneg::{negotiate_languages, NegotiationStrategy};
use i18n_embed::{fluent::FluentLanguageLoader, I18nAssets, LanguageLoader};
use unic_langid::LanguageIdentifier;

pub use i18n_embed::fluent::fluent_language_loader;
pub use i18n_embed_fl::fl;

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

/// Load the best matching locales from `assets` into `loader`, falling back to `en`.
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
}
