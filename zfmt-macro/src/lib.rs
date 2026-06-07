use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

mod bytecode;
mod codegen;
mod enum_derive;
mod fmtstr;
mod format_into;
mod hash;
mod log_text;
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

/// Intern a string literal into the `.zfmt_strings` linker section at compile
/// time and evaluate to its `u32` FNV-1a hash (§4.7).
///
/// ```ignore
/// let hash: u32 = zfmt::zfmt_str!("my interned string");
/// ```
#[proc_macro]
pub fn zfmt_str(input: TokenStream) -> TokenStream {
    let lit: syn::LitStr = match syn::parse(input) {
        Ok(l) => l,
        Err(e) => return e.to_compile_error().into(),
    };
    let s = lit.value();
    let string_section = codegen::gen_string_section(Some(&s), None);
    let hash32: u32 = hash::fnv1a_64(&s) as u32;
    quote::quote! {{
        #string_section
        #hash32
    }}.into()
}

/// Internal macro used by the unstructured logging arms of `log_debug!` etc.
///
/// Parses the format string at compile time, generates `Format::fmt` calls into
/// a `FixedBuf`, and sends the result as a `DebugMessage` via a direct binary
/// send (bypassing output-mode feature flags so unstructured events are always
/// emitted in binary form).
///
/// Not intended for direct use.
#[proc_macro]
pub fn __zfmt_log_text(input: TokenStream) -> TokenStream {
    log_text::generate(input)
}
