// Proc-macro crate — populated in Phase 2.
use proc_macro::TokenStream;

#[proc_macro_derive(Zfmt, attributes(zfmt))]
pub fn derive_zfmt(_input: TokenStream) -> TokenStream {
    TokenStream::new()
}
