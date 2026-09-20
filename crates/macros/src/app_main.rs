//! `#[webx::main]` / `#[webx::main(embed)]` and `#[webx::embed_assets]`.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, Ident, ItemFn};

/// File name written by `webx::builder()...build()` into `OUT_DIR`.
const GENERATED_FILE: &str = "webx_embedded_assets.rs";

fn embedded_assets_path() -> Option<std::path::PathBuf> {
    let out = std::env::var_os("OUT_DIR")?;
    let path = std::path::Path::new(&out).join(GENERATED_FILE);
    path.exists().then_some(path)
}

fn is_tokio_main(attr: &syn::Attribute) -> bool {
    let path = attr.path();
    path.segments.len() == 2
        && path.segments[0].ident == "tokio"
        && path.segments[1].ident == "main"
}

/// Arguments for `#[webx::main]` / `#[webx::main(embed)]`.
#[derive(Debug)]
struct MainArgs {
    embed: bool,
}

impl Parse for MainArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.is_empty() {
            return Ok(Self { embed: false });
        }
        let flag: Ident = input.parse()?;
        if flag != "embed" {
            return Err(syn::Error::new(
                flag.span(),
                "unknown argument; expected `embed` or no arguments\n\
                 - `#[webx::main]` — tokio only (disk SPA / no baked wwwroot)\n\
                 - `#[webx::main(embed)]` — also include the build.rs asset table",
            ));
        }
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after `embed`"));
        }
        Ok(Self { embed: true })
    }
}

pub(crate) fn main_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as MainArgs);
    let mut func = parse_macro_input!(item as ItemFn);
    func.attrs.retain(|a| !is_tokio_main(a));

    let embed = if args.embed {
        if embedded_assets_path().is_none() {
            return syn::Error::new(
                proc_macro2::Span::call_site(),
                "`#[webx::main(embed)]` requires build.rs to call\n\
                 `webx::builder().web_root(\"wwwroot\").build()` so OUT_DIR contains\n\
                 webx_embedded_assets.rs.\n\
                 \n\
                 Semantics:\n\
                 - build.rs `web_root` = which files are baked into the binary (compile-time)\n\
                 - `.use_spa(\"wwwroot\")` = disk overlay for operator overrides (runtime)\n\
                 - without `(embed)` = nothing is baked in; SPA is disk-only / auto-detect",
            )
            .to_compile_error()
            .into();
        }
        quote! { ::webx::spa::embed_assets!(); }
    } else {
        // Explicit: do not include even if a stale OUT_DIR file remains.
        quote! {}
    };

    TokenStream::from(quote! {
        #embed
        #[::tokio::main]
        #func
    })
}

pub(crate) fn embed_assets_impl(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new_spanned(
            proc_macro2::TokenStream::from(attr),
            "#[webx::embed_assets] takes no arguments",
        )
        .to_compile_error()
        .into();
    }

    let item = proc_macro2::TokenStream::from(item);
    if embedded_assets_path().is_none() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[webx::embed_assets] requires build.rs to call \
             webx::builder().web_root(...).build() so OUT_DIR contains \
             webx_embedded_assets.rs",
        )
        .to_compile_error()
        .into();
    }

    TokenStream::from(quote! {
        ::webx::spa::embed_assets!();
        #item
    })
}

#[cfg(test)]
mod tests {
    use super::MainArgs;

    fn parse(input: &str) -> syn::Result<MainArgs> {
        syn::parse_str::<MainArgs>(input)
    }

    #[test]
    fn no_arguments_means_no_embedding() {
        let args = parse("").expect("empty args are valid");
        assert!(!args.embed);
    }

    #[test]
    fn embed_opt_in_is_recognised() {
        let args = parse("embed").expect("`embed` is valid");
        assert!(args.embed);
    }

    #[test]
    fn an_unknown_flag_names_both_forms() {
        let err = parse("embedded").expect_err("unknown flags must be rejected");
        let message = err.to_string();
        assert!(message.contains("unknown argument"), "got {message}");
        assert!(
            message.contains("#[webx::main]"),
            "the message must show the plain form, got {message}"
        );
        assert!(
            message.contains("#[webx::main(embed)]"),
            "the message must show the embedding form, got {message}"
        );
    }

    #[test]
    fn trailing_tokens_after_embed_are_rejected() {
        let err = parse("embed now").expect_err("extra tokens must be rejected");
        assert!(err.to_string().contains("after `embed`"), "got {err}");
    }
}
