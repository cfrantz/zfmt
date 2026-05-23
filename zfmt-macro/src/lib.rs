use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

mod bytecode;
mod fmtstr;
mod format_into;
mod hash;
mod parse;
mod tier1;

#[proc_macro_derive(Zfmt, attributes(zfmt))]
pub fn derive_zfmt(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    tier1::derive_struct(&input)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}
