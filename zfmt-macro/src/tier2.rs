//! Code generation additions for Tier-2 events (variable-length fields, §5.2, Phase 4).
//!
//! A Tier-2 struct has at least one `str`/`String`/`&str` field.
//! For such structs:
//!   - `payload_size()` is a runtime sum
//!   - `serialize_into()` serializes each field individually:
//!       fixed fields: zerocopy as usual
//!       str fields: LEB128(len) + bytes
//!   - Bytecode: `utf8-byte | var-length` instruction for each str field

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Expr, ExprLit, Fields, Lit, Type};

use crate::parse::canonical_type_str;

pub fn is_tier2_field(ty: &Type) -> bool {
    canonical_type_str(ty) == "str"
}

/// Returns true if any non-padding field in `fields` is a str field.
pub fn is_tier2(fields: &Fields) -> bool {
    let all: Vec<_> = match fields {
        Fields::Named(n) => n.named.iter().collect(),
        Fields::Unnamed(u) => u.unnamed.iter().collect(),
        Fields::Unit => vec![],
    };
    all.iter().any(|f| {
        let is_pad = f.ident.as_ref().map(|i| i.to_string().starts_with('_')).unwrap_or(false);
        !is_pad && is_tier2_field(&f.ty)
    })
}

/// Generate `payload_size(&self) -> usize` for a Tier-2 struct.
/// Sums: for fixed fields, `core::mem::size_of` of the field type; for
/// str fields, `leb128_encoded_len(s.len()) + s.len()`.
pub fn build_tier2_payload_size(fields: &Fields) -> TokenStream {
    let mut terms: Vec<TokenStream> = Vec::new();

    let all: Vec<_> = match fields {
        Fields::Named(n) => n.named.iter().map(|f| {
            let is_pad = f.ident.as_ref().map(|i| i.to_string().starts_with('_')).unwrap_or(false);
            let access = if let Some(id) = &f.ident {
                quote! { self.#id }
            } else { quote! { } };
            (f.ty.clone(), access, is_pad)
        }).collect(),
        Fields::Unnamed(u) => u.unnamed.iter().enumerate().map(|(i, f)| {
            let idx = syn::Index::from(i);
            (f.ty.clone(), quote! { self.#idx }, false)
        }).collect(),
        Fields::Unit => vec![],
    };

    let mut offset: usize = 0;
    for (ty, access, _is_pad) in &all {
        let canon = canonical_type_str(ty);
        if canon == "str" {
            // Flush any accumulated fixed-field contribution.
            // (offset already includes padding; we'll add it as a literal)
            // Then add the runtime str size.
            terms.push(quote! {
                {
                    let s: &str = #access;
                    ::zfmt::leb128::encoded_len(s.len() as u32) + s.len()
                }
            });
            offset = 0; // str fields break the fixed run; padding resets
        } else if let Some(sz) = crate::bytecode::size_of_canonical(&canon) {
            let align = sz.min(8);
            let aligned = align_up(offset, align);
            let pad = aligned - offset;
            offset = aligned + sz;
            let total = pad + sz;
            terms.push(quote! { #total });
        } else if let Type::Array(arr) = ty {
            let elem_canon = canonical_type_str(&arr.elem);
            if let Some(elem_sz) = crate::bytecode::size_of_canonical(&elem_canon) {
                if let Ok(n) = array_len_usize(&arr.len) {
                    let sz = n * elem_sz;
                    let align = elem_sz.min(8);
                    let aligned = align_up(offset, align);
                    let pad = aligned - offset;
                    offset = aligned + sz;
                    let total = pad + sz;
                    terms.push(quote! { #total });
                }
            }
        }
    }

    if terms.is_empty() {
        quote! { 0usize }
    } else {
        quote! { 0usize #( + #terms )* }
    }
}

/// Generate `serialize_into` statements for Tier-2.
/// Fixed fields: zerocopy slice per field (with alignment gaps as skip bytes).
/// Str fields: emit LEB128 length then bytes.
pub fn build_tier2_serialize(fields: &Fields) -> Vec<TokenStream> {
    let mut stmts: Vec<TokenStream> = Vec::new();

    let all: Vec<_> = match fields {
        Fields::Named(n) => n.named.iter().map(|f| {
            let is_pad = f.ident.as_ref().map(|i| i.to_string().starts_with('_')).unwrap_or(false);
            let access = if let Some(id) = &f.ident {
                quote! { self.#id }
            } else { quote! { } };
            (f.ty.clone(), access, is_pad)
        }).collect(),
        Fields::Unnamed(u) => u.unnamed.iter().enumerate().map(|(i, f)| {
            let idx = syn::Index::from(i);
            (f.ty.clone(), quote! { self.#idx }, false)
        }).collect(),
        Fields::Unit => vec![],
    };

    let mut offset: usize = 0;

    for (ty, access, _is_pad) in &all {
        let canon = canonical_type_str(ty);
        if canon == "str" {
            // Emit LEB128 length, then bytes.
            stmts.push(quote! {
                {
                    let s: &str = #access;
                    let slen = s.len() as u32;
                    let mut leb_buf = [0u8; 5];
                    let leb_n = ::zfmt::leb128::encode(slen, &mut leb_buf);
                    buf[_pos.._pos + leb_n].copy_from_slice(&leb_buf[..leb_n]);
                    _pos += leb_n;
                    let sb = s.as_bytes();
                    buf[_pos.._pos + sb.len()].copy_from_slice(sb);
                    _pos += sb.len();
                }
            });
            offset = 0;
        } else if let Some(sz) = crate::bytecode::size_of_canonical(&canon) {
            let align = sz.min(8);
            let aligned = align_up(offset, align);
            let pad = aligned - offset;
            offset = aligned + sz;
            if pad > 0 {
                stmts.push(quote! { _pos += #pad; });
            }
            stmts.push(quote! {
                {
                    let field_bytes = unsafe {
                        ::core::slice::from_raw_parts(
                            &(#access) as *const _ as *const u8,
                            #sz,
                        )
                    };
                    buf[_pos.._pos + #sz].copy_from_slice(field_bytes);
                    _pos += #sz;
                }
            });
        } else if let Type::Array(arr) = ty {
            let elem_canon = canonical_type_str(&arr.elem);
            if let Some(elem_sz) = crate::bytecode::size_of_canonical(&elem_canon) {
                if let Ok(n) = array_len_usize(&arr.len) {
                    let sz = n * elem_sz;
                    let align = elem_sz.min(8);
                    let aligned = align_up(offset, align);
                    let pad = aligned - offset;
                    offset = aligned + sz;
                    if pad > 0 {
                        stmts.push(quote! { _pos += #pad; });
                    }
                    stmts.push(quote! {
                        {
                            let field_bytes = unsafe {
                                ::core::slice::from_raw_parts(
                                    &(#access) as *const _ as *const u8,
                                    #sz,
                                )
                            };
                            buf[_pos.._pos + #sz].copy_from_slice(field_bytes);
                            _pos += #sz;
                        }
                    });
                }
            }
        }
    }

    stmts
}

/// Generate `impl ::zfmt::ZfmtEvent for Struct` for a Tier-2 (variable-length) struct.
/// Uses a 256-byte stack buffer and `serialize_into` since payload length is not const.
pub fn build_tier2_zfmt_event_impl(
    struct_name: &proc_macro2::Ident,
    impl_generics: &syn::ImplGenerics<'_>,
    ty_generics: &syn::TypeGenerics<'_>,
    where_clause: Option<&syn::WhereClause>,
    payload_size_expr: &TokenStream,
) -> TokenStream {
    quote! {
        impl #impl_generics ::zfmt::ZfmtEvent for #struct_name #ty_generics #where_clause {
            fn zfmt_tag(&self) -> u32 { Self::ZFMT_TAG }
            fn payload_size(&self) -> usize { #payload_size_expr }
            fn with_payload_bytes<F: ::core::ops::FnOnce(&[u8])>(&self, f: F) {
                const ZFMT_MAX_PAYLOAD: usize = 256;
                let sz = ::zfmt::ZfmtEvent::payload_size(self);
                let mut buf = [0u8; ZFMT_MAX_PAYLOAD];
                self.serialize_into(&mut buf);
                f(&buf[..sz]);
            }
        }
    }
}

fn align_up(offset: usize, align: usize) -> usize {
    if align == 0 { offset } else { (offset + align - 1) & !(align - 1) }
}

fn array_len_usize(expr: &Expr) -> Result<usize, ()> {
    if let Expr::Lit(ExprLit { lit: Lit::Int(n), .. }) = expr {
        n.base10_parse::<usize>().map_err(|_| ())
    } else {
        Err(())
    }
}
