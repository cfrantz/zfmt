//! Code generation for Tier-1 structs (§5.1, §8.1).

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{spanned::Spanned, DeriveInput, Expr, ExprLit, Fields, Lit, LitByteStr, Type};

use crate::{
    bytecode::{
        item, operand, opcode, push_skip, push_uleb128, size_of_canonical,
    },
    hash::{fnv1a_64, struct_hash_input, tag_of},
    parse::{canonical_type_str, extract_format_str, is_padding_field, parse_fields},
};

/// Top-level entry point called from lib.rs for a struct.
pub fn derive_struct(input: &DeriveInput) -> syn::Result<TokenStream> {
    let struct_name = &input.ident;
    let name_str = struct_name.to_string();
    let format_str = extract_format_str(&input.attrs)?;

    // Collect generics so the impl blocks are `impl<'a, T> Foo<'a, T>`.
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let fields_syn = match &input.data {
        syn::Data::Struct(s) => &s.fields,
        _ => {
            return Err(syn::Error::new(
                input.span(),
                "zfmt: #[derive(Zfmt)] on enums is not yet implemented (Phase 5)",
            ))
        }
    };

    let parsed_fields = parse_fields(fields_syn)?;

    // Build canonical hash input and compute tag.
    let field_pairs: Vec<(&str, &str)> = parsed_fields
        .iter()
        .map(|f| (f.name.as_str(), f.canonical_type.as_str()))
        .collect();
    let hash_input = struct_hash_input(
        &name_str,
        format_str.as_deref(),
        &field_pairs,
    );
    let full_hash: u64 = fnv1a_64(&hash_input);
    let tag: u32 = tag_of(full_hash);

    let tier2 = crate::tier2::is_tier2(fields_syn);

    // Build bytecode (same structure for Tier-1 and Tier-2; var-length operand for str fields).
    let bytecode = if tier2 {
        build_tier2_bytecode(fields_syn)?
    } else {
        build_tier1_bytecode(fields_syn)?
    };

    // format_hash: FNV-1a of the format string (0 if none).
    let format_hash: u32 = format_str
        .as_deref()
        .map(|s| fnv1a_64(s) as u32)
        .unwrap_or(0);

    // Pad bytecode to 4-byte alignment.
    let mut bc = bytecode.clone();
    while bc.len() % 4 != 0 {
        bc.push(0);
    }
    let bc_len = bytecode.len() as u32; // unpadded length stored in entry

    // Generate the linker section static.
    let section_name = format!(".zfmt_events.{:08x}", tag);
    let static_name = format_ident!(
        "__ZFMT_EVENT_{:08X}",
        tag
    );

    // Build the entry bytes: tag(4) + pad(4) + full_hash(8) + format_hash(4) + pad(4) + bc_len(4) + bytecode[padded]
    let mut entry_bytes: Vec<u8> = Vec::new();
    entry_bytes.extend_from_slice(&tag.to_le_bytes());
    entry_bytes.extend_from_slice(&0u32.to_le_bytes()); // _pad
    entry_bytes.extend_from_slice(&full_hash.to_le_bytes());
    entry_bytes.extend_from_slice(&format_hash.to_le_bytes());
    entry_bytes.extend_from_slice(&0u32.to_le_bytes()); // _pad
    entry_bytes.extend_from_slice(&bc_len.to_le_bytes());
    entry_bytes.extend_from_slice(&bc);
    let entry_len = entry_bytes.len();
    let entry_lit = LitByteStr::new(&entry_bytes, Span::call_site());

    let payload_size_expr = if tier2 {
        crate::tier2::build_tier2_payload_size(fields_syn)
    } else {
        build_payload_size_expr(fields_syn)
    };

    let (serialize_stmts, has_pos) = if tier2 {
        (crate::tier2::build_tier2_serialize(fields_syn), true)
    } else {
        (build_serialize_stmts(fields_syn), false)
    };

    let format_into_impl = crate::format_into::maybe_generate(input)?;

    let tag_lit = tag;
    let full_hash_lit = full_hash;

    let pos_init = if has_pos {
        quote! { let mut _pos: usize = 0; }
    } else {
        quote! {}
    };

    Ok(quote! {
        impl #impl_generics #struct_name #ty_generics #where_clause {
            pub const ZFMT_TAG: u32 = #tag_lit;
            pub const ZFMT_FULL_HASH: u64 = #full_hash_lit;

            pub fn zfmt_tag(&self) -> u32 { Self::ZFMT_TAG }

            pub fn payload_size(&self) -> usize {
                #payload_size_expr
            }

            /// Write the serialized payload into `buf`.
            /// `buf` must be at least `payload_size()` bytes long.
            pub fn serialize_into(&self, buf: &mut [u8]) {
                #pos_init
                #(#serialize_stmts)*
            }
        }

        #format_into_impl

        #[used]
        #[cfg_attr(
            target_os = "none",
            link_section = #section_name
        )]
        #[cfg_attr(
            not(target_os = "none"),
            link_section = #section_name
        )]
        static #static_name: [u8; #entry_len] = *#entry_lit;
    })
}

// ---------------------------------------------------------------------------
// Bytecode builder

/// Build bytecode for all fields of a struct, inserting skip instructions for
/// repr(C) padding bytes between consecutive fixed-size fields.
fn build_tier1_bytecode(fields: &Fields) -> syn::Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut prev_end_offset: usize = 0; // running offset to detect padding

    let all_fields: Vec<_> = match fields {
        Fields::Named(n) => n.named.iter().collect(),
        Fields::Unnamed(u) => u.unnamed.iter().collect(),
        Fields::Unit => vec![],
    };

    for field in &all_fields {
        let canon = canonical_type_str(&field.ty);

        // Align the current field.
        let (field_align, field_size) = align_and_size_of(&field.ty, &canon)?;
        let aligned_offset = align_up(prev_end_offset, field_align);
        let pad = aligned_offset - prev_end_offset;
        if pad > 0 {
            push_skip(&mut out, pad);
        }

        if is_padding_field(field) {
            // Treat padding fields as skip bytes.
            push_skip(&mut out, field_size);
        } else {
            emit_field_bytecode(&mut out, &field.ty, &canon)?;
        }

        prev_end_offset = aligned_offset + field_size;
    }

    // End instruction.
    out.push(opcode(item::END, operand::SINGLE));
    Ok(out)
}

/// Emit the bytecode instruction(s) for one non-padding field.
fn emit_field_bytecode(out: &mut Vec<u8>, ty: &Type, canon: &str) -> syn::Result<()> {
    // Fixed-array: [T; N]
    if let Type::Array(arr) = ty {
        let elem_canon = canonical_type_str(&arr.elem);
        if let Some(item_ty) = crate::bytecode::item_type_for(&elem_canon) {
            let n = array_len(&arr.len)?;
            if item_ty == item::UTF8_BYTE || elem_canon == "u8" {
                // u8 arrays: emit as utf8-byte fixed-array (covers [u8; N])
                out.push(opcode(item::UTF8_BYTE, operand::FIXED_ARRAY));
            } else {
                out.push(opcode(item_ty, operand::FIXED_ARRAY));
            }
            push_uleb128(out, n as u64);
            return Ok(());
        }
    }

    if let Some(item_ty) = crate::bytecode::item_type_for(canon) {
        out.push(opcode(item_ty, operand::SINGLE));
        return Ok(());
    }

    Err(syn::Error::new(
        ty.span(),
        format!(
            "zfmt: unsupported field type `{}` for Tier-1 struct; \
             only primitive integers, floats, bool, char, and fixed arrays are allowed",
            canon
        ),
    ))
}

/// Build bytecode for a Tier-2 struct — same as Tier-1 but str fields use
/// `utf8-byte | var-length` (operand 3).  Str fields have no fixed size so
/// alignment tracking resets after each one.
fn build_tier2_bytecode(fields: &Fields) -> syn::Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut prev_end_offset: usize = 0;

    let all_fields: Vec<_> = match fields {
        Fields::Named(n) => n.named.iter().collect(),
        Fields::Unnamed(u) => u.unnamed.iter().collect(),
        Fields::Unit => vec![],
    };

    for field in &all_fields {
        let canon = canonical_type_str(&field.ty);

        if canon == "str" {
            // str fields: LEB128 element count in stream, then UTF-8 bytes.
            // No alignment padding before/after a var-length field.
            prev_end_offset = 0; // variable, alignment tracking resets
            out.push(opcode(item::UTF8_BYTE, operand::VAR_LENGTH));
            continue;
        }

        // Fixed field — same as Tier-1.
        let (field_align, field_size) = align_and_size_of(&field.ty, &canon)?;
        let aligned_offset = align_up(prev_end_offset, field_align);
        let pad = aligned_offset - prev_end_offset;
        if pad > 0 {
            push_skip(&mut out, pad);
        }
        if is_padding_field(field) {
            push_skip(&mut out, field_size);
        } else {
            emit_field_bytecode(&mut out, &field.ty, &canon)?;
        }
        prev_end_offset = aligned_offset + field_size;
    }

    out.push(opcode(item::END, operand::SINGLE));
    Ok(out)
}

// ---------------------------------------------------------------------------
// payload_size expression builder

fn build_payload_size_expr(_fields: &Fields) -> TokenStream {
    quote! { ::core::mem::size_of::<Self>() }
}

// ---------------------------------------------------------------------------
// serialize_into statement builder

fn build_serialize_stmts(_fields: &Fields) -> Vec<TokenStream> {
    // Tier-1: zerocopy of the whole struct (repr(C) guarantees layout).
    vec![quote! {
        let bytes = unsafe {
            ::core::slice::from_raw_parts(
                self as *const Self as *const u8,
                ::core::mem::size_of::<Self>(),
            )
        };
        buf[..bytes.len()].copy_from_slice(bytes);
    }]
}

// ---------------------------------------------------------------------------
// Alignment / size helpers

fn align_and_size_of(ty: &Type, canon: &str) -> syn::Result<(usize, usize)> {
    // Primitive types
    if let Some(sz) = size_of_canonical(canon) {
        return Ok((sz.min(8), sz)); // natural alignment, capped at 8
    }
    // Fixed arrays: align = align(elem), size = N * size(elem)
    if let Type::Array(arr) = ty {
        let elem_canon = canonical_type_str(&arr.elem);
        if let Some(elem_sz) = size_of_canonical(&elem_canon) {
            let n = array_len(&arr.len)?;
            return Ok((elem_sz.min(8), n * elem_sz));
        }
    }
    Err(syn::Error::new(
        ty.span(),
        format!("zfmt: cannot determine size of type `{}`", canon),
    ))
}

fn align_up(offset: usize, align: usize) -> usize {
    if align == 0 {
        return offset;
    }
    (offset + align - 1) & !(align - 1)
}

fn array_len(expr: &Expr) -> syn::Result<usize> {
    if let Expr::Lit(ExprLit { lit: Lit::Int(n), .. }) = expr {
        n.base10_parse::<usize>()
            .map_err(|e| syn::Error::new(n.span(), e))
    } else {
        Err(syn::Error::new(expr.span(), "zfmt: array length must be a literal integer"))
    }
}
