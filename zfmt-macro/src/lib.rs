use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

mod bytecode;
mod codegen;
mod enum_derive;
mod fmtstr;
mod format_into;
mod hash;
mod parse;
mod tier1;
mod tier2;

#[proc_macro_derive(Zfmt, attributes(zfmt))]
pub fn derive_zfmt(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let result = match &input.data {
        syn::Data::Struct(_) => tier1::derive_struct(&input),
        syn::Data::Enum(_) => enum_derive::derive_enum(&input),
        syn::Data::Union(_) => Err(syn::Error::new_spanned(
            &input.ident,
            "zfmt: #[derive(Zfmt)] is not supported on unions",
        )),
    };
    result.unwrap_or_else(|e| e.to_compile_error()).into()
}
