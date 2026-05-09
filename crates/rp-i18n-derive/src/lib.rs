//! Derive macro for [`rp_i18n::LocalizedParser`].
//!
//! Apply alongside `clap::Parser` and use `#[localized(about = "key")]` /
//! `#[localized(help = "key")]` to map struct/field help to Fluent keys.
//! See the crate-level docs in `crates/rp-i18n/src/lib.rs` and the spike
//! write-up at `docs/plans/i18n-cli-spike.md` §10 for the user-facing flow.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    parse_macro_input, punctuated::Punctuated, Attribute, Data, DeriveInput, Expr, ExprLit, Fields,
    Lit, MetaNameValue, Token,
};

/// Generate a `LocalizedParser` impl that mutates the clap `Command` produced
/// by `#[derive(Parser)]` to swap help text for `fl!()` calls before parse.
#[proc_macro_derive(LocalizedParser, attributes(localized))]
pub fn derive_localized_parser(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

fn expand(input: DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;

    let about_key = find_localized_value(&input.attrs, "about")?;

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => &named.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    name,
                    "LocalizedParser requires a struct with named fields",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new_spanned(
                name,
                "LocalizedParser can only be derived on structs",
            ));
        }
    };

    let mut mutations = Vec::<TokenStream2>::new();

    if let Some(key) = about_key {
        mutations.push(quote! {
            .about(::rp_i18n::fl!(loader, #key))
        });
    }

    for field in fields {
        let Some(ident) = &field.ident else { continue };
        let ident_str = ident.to_string();
        if let Some(key) = find_localized_value(&field.attrs, "help")? {
            mutations.push(quote! {
                .mut_arg(#ident_str, |a| a.help(::rp_i18n::fl!(loader, #key)))
            });
        }
    }

    Ok(quote! {
        impl ::rp_i18n::LocalizedParser for #name {
            fn parse_localized(loader: &::rp_i18n::FluentLanguageLoader) -> Self {
                use ::clap::{CommandFactory, FromArgMatches};
                let cmd = <Self as CommandFactory>::command()
                    #(#mutations)*;
                let matches = cmd.get_matches();
                match <Self as FromArgMatches>::from_arg_matches(&matches) {
                    Ok(args) => args,
                    Err(e) => e.exit(),
                }
            }

            fn try_parse_localized(
                loader: &::rp_i18n::FluentLanguageLoader,
            ) -> ::std::result::Result<Self, ::clap::Error> {
                use ::clap::{CommandFactory, FromArgMatches};
                let cmd = <Self as CommandFactory>::command()
                    #(#mutations)*;
                let matches = cmd.try_get_matches()?;
                <Self as FromArgMatches>::from_arg_matches(&matches)
            }
        }
    })
}

fn find_localized_value(attrs: &[Attribute], key: &str) -> syn::Result<Option<String>> {
    for attr in attrs {
        if !attr.path().is_ident("localized") {
            continue;
        }
        let pairs: Punctuated<MetaNameValue, Token![,]> =
            attr.parse_args_with(Punctuated::parse_terminated)?;
        for nv in &pairs {
            if !nv.path.is_ident(key) {
                continue;
            }
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(s), ..
            }) = &nv.value
            {
                return Ok(Some(s.value()));
            }
            return Err(syn::Error::new_spanned(
                &nv.value,
                format!("expected string literal for `{key}`"),
            ));
        }
    }
    Ok(None)
}
