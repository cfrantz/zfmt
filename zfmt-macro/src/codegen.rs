//! Generic field planning and code generation shared by structs and enum variants.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Expr, ExprLit, Lit, LitByteStr, Type};

use crate::{
    bytecode::{item, operand, opcode, push_skip, push_uleb128, size_of_canonical},
    parse::canonical_type_str,
};

/// A single logical field for code-generation purposes.
pub struct FieldPlan {
    /// Canonical type string.
    pub canon: String,
    /// The syn Type (for unsafe pointer casts in generated code).
    pub ty: Type,
    /// Expression that yields the field value (e.g. `self.foo` or bound `foo`).
    pub access: TokenStream,
    /// True if this is a padding field (name starts with `_`).
    pub is_pad: bool,
}

/// Compute (payload_size expression, serialize_stmts) for a list of FieldPlans.
///
/// The generated code matches the repr(C) layout: natural alignment gaps are
/// filled with zero bytes, and variable-length (str) fields are LEB128-prefixed.
pub fn gen_payload_and_serialize(plans: &[FieldPlan]) -> (TokenStream, Vec<TokenStream>) {
    let mut size_terms: Vec<TokenStream> = Vec::new();
    let mut stmts: Vec<TokenStream> = Vec::new();
    let mut offset: usize = 0; // running byte offset for fixed-width fields

    for plan in plans {
        if plan.canon == "str" {
            // Variable-length: LEB128(len) + bytes.  Alignment resets after var field.
            let access = &plan.access;
            size_terms.push(quote! {{
                let _s: &str = &#access;
                ::zfmt::leb128::encoded_len(_s.len() as u64) + _s.len()
            }});
            stmts.push(quote! {{
                let _s: &str = &#access;
                let _slen = _s.len() as u64;
                let mut _leb = [0u8; 10];
                let _ln = ::zfmt::leb128::encode(_slen, &mut _leb);
                buf[_pos.._pos + _ln].copy_from_slice(&_leb[.._ln]);
                _pos += _ln;
                let _sb = _s.as_bytes();
                buf[_pos.._pos + _sb.len()].copy_from_slice(_sb);
                _pos += _sb.len();
            }});
            offset = 0; // variable field breaks alignment tracking
            continue;
        }

        let (field_align, field_size) = match layout_of(&plan.ty, &plan.canon) {
            Some(x) => x,
            None => continue, // skip unresolvable types gracefully
        };

        let aligned = align_up(offset, field_align);
        let pad = aligned - offset;
        offset = aligned + field_size;

        if pad > 0 {
            size_terms.push(quote! { #pad });
            stmts.push(quote! {{
                for _i in 0..#pad { buf[_pos + _i] = 0; }
                _pos += #pad;
            }});
        }

        if plan.is_pad {
            // Padding field — skip (write zeros) without displaying.
            size_terms.push(quote! { #field_size });
            stmts.push(quote! {{
                for _i in 0..#field_size { buf[_pos + _i] = 0; }
                _pos += #field_size;
            }});
        } else {
            let access = &plan.access;
            size_terms.push(quote! { #field_size });
            stmts.push(quote! {{
                let _fb = unsafe {
                    ::core::slice::from_raw_parts(
                        &#access as *const _ as *const u8,
                        #field_size,
                    )
                };
                buf[_pos.._pos + #field_size].copy_from_slice(_fb);
                _pos += #field_size;
            }});
        }
    }

    let size_expr = if size_terms.is_empty() {
        quote! { 0usize }
    } else {
        quote! { 0usize #( + #size_terms )* }
    };

    (size_expr, stmts)
}

/// Build Tier-1/Tier-2 bytecode for a list of FieldPlans.
/// Returns `Err` if a field type is not supported.
pub fn gen_bytecode(plans: &[FieldPlan]) -> Result<Vec<u8>, String> {
    let mut bc: Vec<u8> = Vec::new();
    let mut offset: usize = 0;

    for plan in plans {
        if plan.canon == "str" {
            bc.push(opcode(item::UTF8_BYTE, operand::VAR_LENGTH));
            offset = 0;
            continue;
        }

        let (field_align, field_size) = match layout_of(&plan.ty, &plan.canon) {
            Some(x) => x,
            None => return Err(format!("zfmt: cannot determine layout of `{}`", plan.canon)),
        };

        let aligned = align_up(offset, field_align);
        let pad = aligned - offset;
        offset = aligned + field_size;

        if pad > 0 {
            push_skip(&mut bc, pad);
        }

        if plan.is_pad {
            push_skip(&mut bc, field_size);
        } else {
            emit_field_bytecode_for(&mut bc, &plan.ty, &plan.canon)?;
        }
    }

    bc.push(opcode(item::END, operand::SINGLE));
    Ok(bc)
}

/// Emit bytecode instructions for a single non-padding field.
fn emit_field_bytecode_for(out: &mut Vec<u8>, ty: &Type, canon: &str) -> Result<(), String> {
    if let Type::Array(arr) = ty {
        let elem_canon = canonical_type_str(&arr.elem);
        if let Some(item_ty) = crate::bytecode::item_type_for(&elem_canon) {
            let n = array_len_of(&arr.len).ok_or_else(|| {
                format!("zfmt: array length in `{}` must be a literal integer", canon)
            })?;
            if elem_canon == "u8" {
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
    Err(format!(
        "zfmt: unsupported field type `{}`: \
         only primitives, fixed arrays, and str are supported",
        canon
    ))
}

// ---------------------------------------------------------------------------
// Layout helpers

/// Returns (alignment, size) for a canonical type.
pub fn layout_of(ty: &Type, canon: &str) -> Option<(usize, usize)> {
    if let Some(sz) = size_of_canonical(canon) {
        return Some((sz.min(8), sz));
    }
    if let Type::Array(arr) = ty {
        let elem_canon = canonical_type_str(&arr.elem);
        if let Some(elem_sz) = size_of_canonical(&elem_canon) {
            let n = array_len_of(&arr.len)?;
            return Some((elem_sz.min(8), n * elem_sz));
        }
    }
    None
}

pub fn align_up(offset: usize, align: usize) -> usize {
    if align == 0 { offset } else { (offset + align - 1) & !(align - 1) }
}

pub fn array_len_of(expr: &Expr) -> Option<usize> {
    if let Expr::Lit(ExprLit { lit: Lit::Int(n), .. }) = expr {
        n.base10_parse::<usize>().ok()
    } else {
        None
    }
}

/// Compute the total size of a list of FieldPlans including tail padding,
/// given the maximum alignment seen across all fields.
pub fn total_size_with_tail_padding(plans: &[FieldPlan]) -> Option<usize> {
    let mut offset: usize = 0;
    let mut max_align: usize = 1;
    for plan in plans {
        if plan.canon == "str" {
            return None; // variable-size
        }
        let (fa, fs) = layout_of(&plan.ty, &plan.canon)?;
        if fa > max_align { max_align = fa; }
        let aligned = align_up(offset, fa);
        offset = aligned + fs;
    }
    Some(align_up(offset, max_align))
}

// ---------------------------------------------------------------------------
// String section helpers

/// Generate a `.zfmt_strings.<hex>` linker section static for a format string.
///
/// Entry layout (§8.2):
///   hash(u32 LE) + len(u16 LE) + _pad(u16=0) + bytes[padded to 4-byte boundary]
///
/// Returns an empty TokenStream when `format_str` is None.
pub fn gen_string_section(format_str: Option<&str>) -> TokenStream {
    let fmt = match format_str {
        Some(s) if !s.is_empty() => s,
        _ => return quote! {},
    };

    let hash: u32 = crate::hash::fnv1a_64(fmt) as u32;
    let str_bytes = fmt.as_bytes();
    let str_len = str_bytes.len() as u16;

    let mut entry: Vec<u8> = Vec::new();
    entry.extend_from_slice(&hash.to_le_bytes());
    entry.extend_from_slice(&str_len.to_le_bytes());
    entry.extend_from_slice(&0u16.to_le_bytes()); // _pad
    entry.extend_from_slice(str_bytes);
    while entry.len() % 4 != 0 {
        entry.push(0);
    }

    let entry_len = entry.len();
    let entry_lit = LitByteStr::new(&entry, Span::call_site());
    let section_name = format!(".zfmt_strings.{:08x}", hash);
    let static_name = format_ident!("__ZFMT_STRING_{:08X}", hash);

    quote! {
        #[used]
        #[cfg_attr(    target_os = "none",  link_section = #section_name)]
        #[cfg_attr(not(target_os = "none"), link_section = #section_name)]
        static #static_name: [u8; #entry_len] = *#entry_lit;
    }
}
