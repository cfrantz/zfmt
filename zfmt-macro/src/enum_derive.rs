//! Code generation for top-level enum events and inline enums (§5.3, §4.5, Phase 5).

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{
    spanned::Spanned, Attribute, DataEnum, DeriveInput, Fields, Ident, LitByteStr,
    Variant,
};

use crate::{
    bytecode::{item, operand, opcode},
    codegen::{
        gen_bytecode, gen_payload_and_serialize, total_size_with_tail_padding,
        FieldPlan,
    },
    fmtstr::{parse_format_str, Align, FmtType, ParsedSpec, Segment},
    hash::{fnv1a_64, tag_of, variant_hash_input},
    parse::{canonical_type_str, extract_format_str, parse_fields},
};

// ---------------------------------------------------------------------------
// Entry point

pub fn derive_enum(input: &DeriveInput) -> syn::Result<TokenStream> {
    let data = match &input.data {
        syn::Data::Enum(e) => e,
        _ => unreachable!(),
    };

    match extract_inline_repr(&input.attrs) {
        Some(discrim_item) => derive_inline_enum(input, data, discrim_item),
        None => derive_toplevel_enum(input, data),
    }
}

// ---------------------------------------------------------------------------
// Detect #[repr(C, u8/u16/u32/u64)]

fn extract_inline_repr(attrs: &[Attribute]) -> Option<u8> {
    for attr in attrs {
        if !attr.path().is_ident("repr") {
            continue;
        }
        if let Ok(ml) = attr.meta.require_list() {
            let tokens = ml.tokens.to_string().replace(' ', "");
            // Accept bare repr(u8/u16/u32/u64) — correct for unit-variant enums.
            // Also accept repr(C, u8/…) — valid for data-carrying enums.
            let parts: Vec<&str> = tokens.split(',').collect();
            for part in &parts {
                match *part {
                    "u8"  => return Some(crate::bytecode::item::U8),
                    "u16" => return Some(crate::bytecode::item::U16),
                    "u32" => return Some(crate::bytecode::item::U32),
                    "u64" => return Some(crate::bytecode::item::U64),
                    _ => {}
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Top-level enum derive

fn derive_toplevel_enum(input: &DeriveInput, data: &DataEnum) -> syn::Result<TokenStream> {
    let enum_ident = &input.ident;
    let enum_name_str = enum_ident.to_string();
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let mut tag_consts: Vec<TokenStream> = Vec::new();
    let mut tag_arms: Vec<TokenStream> = Vec::new();
    let mut size_arms: Vec<TokenStream> = Vec::new();
    let mut ser_arms: Vec<TokenStream> = Vec::new();
    let mut fmt_arms: Vec<TokenStream> = Vec::new();
    let mut linker_statics: Vec<TokenStream> = Vec::new();

    for variant in &data.variants {
        let vname = &variant.ident;
        let vname_str = vname.to_string();
        let vformat_str = extract_format_str(&variant.attrs)?;
        let vfields = parse_fields(&variant.fields)?;

        let field_pairs: Vec<(&str, &str)> = vfields
            .iter()
            .map(|f| (f.name.as_str(), f.canonical_type.as_str()))
            .collect();

        let hash_input = variant_hash_input(
            &enum_name_str,
            &vname_str,
            vformat_str.as_deref(),
            &field_pairs,
        );
        let full_hash = fnv1a_64(&hash_input);
        let tag = tag_of(full_hash);

        let tag_const_name = format_ident!("{}_ZFMT_TAG", vname_str.to_uppercase());
        let hash_const_name = format_ident!("{}_ZFMT_FULL_HASH", vname_str.to_uppercase());
        tag_consts.push(quote! {
            pub const #tag_const_name: u32 = #tag;
            pub const #hash_const_name: u64 = #full_hash;
        });

        // Full pattern (binds all fields) for methods that need field values.
        let (pat, access_plans) = variant_pattern_and_plans(enum_ident, variant)?;
        // Wildcard pattern for methods that only need the variant identity.
        let wild_pat = variant_wildcard_pattern(enum_ident, variant);

        // zfmt_tag() arm — no field values needed, use wildcard to avoid unused-var warnings.
        tag_arms.push(quote! { #wild_pat => Self::#tag_const_name, });

        // payload_size() arm
        let (size_expr, ser_stmts) = gen_payload_and_serialize(&access_plans);
        // For Tier-1 variants the size is a compile-time literal; use wildcard
        // pattern so bound field names don't produce unused-variable warnings.
        let size_with_tail = total_size_with_tail_padding(&access_plans);
        let (size_pat, final_size_expr) = if let Some(total) = size_with_tail {
            (wild_pat.clone(), quote! { #total })
        } else {
            (pat.clone(), size_expr)
        };
        size_arms.push(quote! { #size_pat => { #final_size_expr } });

        // serialize_into() arm
        ser_arms.push(quote! {
            #pat => {
                let mut _pos: usize = 0;
                #(#ser_stmts)*
            }
        });

        // format_into() arm
        let fmt_arm_body = variant_format_arm_body(&access_plans, vformat_str.as_deref(), &vfields, variant.span())?;
        fmt_arms.push(quote! { #pat => { #fmt_arm_body } });

        // Linker section static
        let suffix = format!("{}_{}", enum_name_str, vname_str);
        let ls = build_linker_static(tag, full_hash, &access_plans, vformat_str.as_deref(), Some(&suffix))?;
        linker_statics.push(ls);
    }

    Ok(quote! {
        impl #impl_generics #enum_ident #ty_generics #where_clause {
            #(#tag_consts)*

            pub fn zfmt_tag(&self) -> u32 {
                match self { #(#tag_arms)* }
            }

            pub fn payload_size(&self) -> usize {
                match self { #(#size_arms)* }
            }

            pub fn serialize_into(&self, buf: &mut [u8]) {
                match self { #(#ser_arms)* }
            }

        }

        impl #impl_generics ::zfmt::FormatInto for #enum_ident #ty_generics #where_clause {
            fn format_into<W: ::zfmt::Write>(
                &self,
                writer: &mut W,
            ) -> ::core::result::Result<(), ::zfmt::Error> {
                use ::zfmt::Format as _;
                match self { #(#fmt_arms)* }
            }
        }

        impl #impl_generics ::zfmt::ZfmtEvent for #enum_ident #ty_generics #where_clause {
            fn zfmt_tag(&self) -> u32 { self.zfmt_tag() }
            fn payload_size(&self) -> usize { self.payload_size() }
            fn with_payload_bytes<F: ::core::ops::FnOnce(&[u8])>(&self, f: F) {
                const ZFMT_MAX_PAYLOAD: usize = 256;
                let sz = ::zfmt::ZfmtEvent::payload_size(self);
                let mut buf = [0u8; ZFMT_MAX_PAYLOAD];
                self.serialize_into(&mut buf);
                f(&buf[..sz]);
            }
        }

        #(#linker_statics)*
    })
}

// ---------------------------------------------------------------------------
// Inline enum derive

fn derive_inline_enum(
    input: &DeriveInput,
    data: &DataEnum,
    discrim_item: u8,
) -> syn::Result<TokenStream> {
    let enum_ident = &input.ident;
    let enum_name_str = enum_ident.to_string();
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    // Find the union body size = size of the largest variant's payload.
    let union_body_size = compute_union_body_size(data)?;

    let mut tag_consts: Vec<TokenStream> = Vec::new();
    let mut linker_statics: Vec<TokenStream> = Vec::new();

    for variant in &data.variants {
        let vname = &variant.ident;
        let vname_str = vname.to_string();
        let vformat_str = extract_format_str(&variant.attrs)?;
        let vfields = parse_fields(&variant.fields)?;

        let field_pairs: Vec<(&str, &str)> = vfields
            .iter()
            .map(|f| (f.name.as_str(), f.canonical_type.as_str()))
            .collect();

        let hash_input = variant_hash_input(
            &enum_name_str,
            &vname_str,
            vformat_str.as_deref(),
            &field_pairs,
        );
        let full_hash = fnv1a_64(&hash_input);
        let tag = tag_of(full_hash);

        let tag_const_name = format_ident!("{}_ZFMT_TAG", vname_str.to_uppercase());
        let hash_const_name = format_ident!("{}_ZFMT_FULL_HASH", vname_str.to_uppercase());
        tag_consts.push(quote! {
            pub const #tag_const_name: u32 = #tag;
            pub const #hash_const_name: u64 = #full_hash;
        });

        // Build subroutine bytecode: variant fields + tail skip to fill union_body_size.
        let variant_plans = fields_to_plans(&variant.fields, enum_ident);
        let variant_total = total_size_with_tail_padding(&variant_plans)
            .unwrap_or(0);
        let tail_skip = union_body_size.saturating_sub(variant_total);

        let mut bc = gen_bytecode(&variant_plans).map_err(|e| {
            syn::Error::new(variant.span(), e)
        })?;
        // Remove the END instruction gen_bytecode appended so we can add tail skip.
        if bc.last() == Some(&opcode(item::END, operand::SINGLE)) {
            bc.pop();
        }
        if tail_skip > 0 {
            crate::bytecode::push_skip(&mut bc, tail_skip);
        }
        bc.push(opcode(item::END, operand::SINGLE));

        // Pad bytecode to 4-byte boundary.
        let bc_len = bc.len() as u32;
        while bc.len() % 4 != 0 { bc.push(0); }

        let format_hash: u32 = vformat_str.as_deref()
            .map(|s| fnv1a_64(s) as u32)
            .unwrap_or(0);

        let section_name = format!(".zfmt_events.{:08x}", tag);
        let static_name = format_ident!("__ZFMT_EVENT_{:08X}", tag);

        let mut entry: Vec<u8> = Vec::new();
        entry.extend_from_slice(&tag.to_le_bytes());
        entry.extend_from_slice(&0u32.to_le_bytes());
        entry.extend_from_slice(&full_hash.to_le_bytes());
        entry.extend_from_slice(&format_hash.to_le_bytes());
        entry.extend_from_slice(&0u32.to_le_bytes());
        entry.extend_from_slice(&bc_len.to_le_bytes());
        entry.extend_from_slice(&bc);
        let entry_len = entry.len();
        let entry_lit = LitByteStr::new(&entry, Span::call_site());

        let suffix = format!("{}_{}", enum_name_str, vname_str);
        let string_section = crate::codegen::gen_string_section(vformat_str.as_deref(), Some(&suffix));
        linker_statics.push(quote! {
            #[used]
            #[cfg_attr(target_os = "none", link_section = #section_name)]
            #[cfg_attr(not(target_os = "none"), link_section = #section_name)]
            static #static_name: [u8; #entry_len] = *#entry_lit;

            #string_section
        });
    }

    let discrim_item_val = discrim_item;

    Ok(quote! {
        impl #impl_generics #enum_ident #ty_generics #where_clause {
            /// Item type value for this enum's discriminant (§4.2).
            pub const ZFMT_DISCRIMINANT_ITEM_TYPE: u8 = #discrim_item_val;

            #(#tag_consts)*
        }

        #(#linker_statics)*
    })
}

// ---------------------------------------------------------------------------
// Union body size (for inline enum variant padding)

fn compute_union_body_size(data: &DataEnum) -> syn::Result<usize> {
    let mut max: usize = 0;
    for variant in &data.variants {
        let plans = fields_to_plans(&variant.fields, &syn::Ident::new("_", Span::call_site()));
        if let Some(total) = total_size_with_tail_padding(&plans) {
            if total > max { max = total; }
        }
    }
    Ok(max)
}

// ---------------------------------------------------------------------------
// Pattern + field-plan builder

/// Build a match pattern for a variant and a FieldPlan list for its fields.
fn variant_pattern_and_plans(
    enum_ident: &Ident,
    variant: &Variant,
) -> syn::Result<(TokenStream, Vec<FieldPlan>)> {
    match &variant.fields {
        Fields::Named(named) => {
            let vname = &variant.ident;
            let mut bind_idents: Vec<TokenStream> = Vec::new();
            let mut plans: Vec<FieldPlan> = Vec::new();
            for f in &named.named {
                let fname = f.ident.as_ref().unwrap();
                let fname_str = fname.to_string();
                let is_pad = fname_str.starts_with('_');
                let canon = canonical_type_str(&f.ty);
                bind_idents.push(quote! { #fname });
                plans.push(FieldPlan {
                    canon,
                    ty: f.ty.clone(),
                    access: quote! { (*#fname) },
                    is_pad,
                });
            }
            let pat = quote! { #enum_ident::#vname { #(#bind_idents),* } };
            Ok((pat, plans))
        }
        Fields::Unnamed(unnamed) => {
            let vname = &variant.ident;
            let mut bind_idents: Vec<TokenStream> = Vec::new();
            let mut plans: Vec<FieldPlan> = Vec::new();
            for (i, f) in unnamed.unnamed.iter().enumerate() {
                let binding = format_ident!("_f{}", i);
                let canon = canonical_type_str(&f.ty);
                bind_idents.push(quote! { #binding });
                plans.push(FieldPlan {
                    canon,
                    ty: f.ty.clone(),
                    access: quote! { (*#binding) },
                    is_pad: false,
                });
            }
            let pat = quote! { #enum_ident::#vname(#(#bind_idents),*) };
            Ok((pat, plans))
        }
        Fields::Unit => {
            let vname = &variant.ident;
            Ok((quote! { #enum_ident::#vname }, Vec::new()))
        }
    }
}

/// Build a pattern that matches a variant but ignores all its fields.
/// Used in methods that only need variant identity (e.g. zfmt_tag).
fn variant_wildcard_pattern(enum_ident: &Ident, variant: &Variant) -> TokenStream {
    let vname = &variant.ident;
    match &variant.fields {
        Fields::Named(_)   => quote! { #enum_ident::#vname { .. } },
        Fields::Unnamed(_) => quote! { #enum_ident::#vname(..) },
        Fields::Unit       => quote! { #enum_ident::#vname },
    }
}

/// Build a FieldPlan list from syn::Fields for bytecode/size-only purposes
/// (no access expression needed since we're not generating serialize code).
fn fields_to_plans(fields: &Fields, _: &Ident) -> Vec<FieldPlan> {
    match fields {
        Fields::Named(n) => n
            .named
            .iter()
            .map(|f| {
                let fname = f.ident.as_ref().unwrap().to_string();
                let is_pad = fname.starts_with('_');
                FieldPlan {
                    canon: canonical_type_str(&f.ty),
                    ty: f.ty.clone(),
                    access: quote! { () }, // not used for bytecode-only path
                    is_pad,
                }
            })
            .collect(),
        Fields::Unnamed(u) => u
            .unnamed
            .iter()
            .map(|f| FieldPlan {
                canon: canonical_type_str(&f.ty),
                ty: f.ty.clone(),
                access: quote! { () },
                is_pad: false,
            })
            .collect(),
        Fields::Unit => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Format arm body

fn variant_format_arm_body(
    plans: &[FieldPlan],
    format_str: Option<&str>,
    parsed_fields: &[crate::parse::FieldInfo],
    span: proc_macro2::Span,
) -> syn::Result<TokenStream> {
    let fmt_str = match format_str {
        Some(s) => s,
        None => return Ok(quote! { Ok(()) }),
    };
    let segments = parse_format_str(fmt_str)
        .map_err(|e| syn::Error::new(span, format!("zfmt format string: {}", e)))?;

    // Build a map from field name → access expression.
    let field_accesses: std::collections::HashMap<&str, &TokenStream> = plans
        .iter()
        .zip(parsed_fields.iter())
        .filter(|(p, _)| !p.is_pad)
        .map(|(p, fi)| (fi.name.as_str(), &p.access))
        .collect();

    let mut stmts: Vec<TokenStream> = Vec::new();
    for seg in &segments {
        match seg {
            Segment::Literal(s) => stmts.push(quote! { writer.write_str(#s)?; }),
            Segment::Placeholder(ph) => {
                let access = field_accesses.get(ph.name.as_str()).ok_or_else(|| {
                    syn::Error::new(
                        span,
                        format!(
                            "zfmt: unknown field `{}` in format string",
                            ph.name
                        ),
                    )
                })?;
                let spec = spec_expr(&ph.spec);
                stmts.push(quote! {
                    ::zfmt::Format::fmt(&#access, writer, #spec)?;
                });
            }
        }
    }
    Ok(quote! { #(#stmts)* Ok(()) })
}

fn spec_expr(spec: &ParsedSpec) -> TokenStream {
    let ty = match spec.fmt_type {
        FmtType::Display  => quote! { ::zfmt::FormatType::Display },
        FmtType::LowerHex => quote! { ::zfmt::FormatType::LowerHex },
        FmtType::UpperHex => quote! { ::zfmt::FormatType::UpperHex },
        FmtType::Binary   => quote! { ::zfmt::FormatType::Binary },
        FmtType::Octal    => quote! { ::zfmt::FormatType::Octal },
        FmtType::Char     => quote! { ::zfmt::FormatType::Char },
    };
    let align = match spec.align {
        Align::None  => quote! { ::zfmt::Align::None },
        Align::Left  => quote! { ::zfmt::Align::Left },
        Align::Right => quote! { ::zfmt::Align::Right },
    };
    let sign      = spec.sign;
    let alternate = spec.alternate;
    let zero_pad  = spec.zero_pad;
    let width     = spec.width.unwrap_or(0);
    let precision = match spec.precision {
        Some(p) => quote! { Some(#p) },
        None    => quote! { None },
    };
    quote! {
        ::zfmt::FormatSpec {
            ty: #ty, alternate: #alternate, sign: #sign,
            zero_pad: #zero_pad, width: #width, precision: #precision, align: #align,
        }
    }
}

// ---------------------------------------------------------------------------
// Linker section static builder

fn build_linker_static(
    tag: u32,
    full_hash: u64,
    plans: &[FieldPlan],
    format_str: Option<&str>,
    suffix: Option<&str>,
) -> syn::Result<TokenStream> {
    let mut bc = gen_bytecode(plans).map_err(|e| {
        syn::Error::new(Span::call_site(), e)
    })?;
    let bc_len = bc.len() as u32;
    while bc.len() % 4 != 0 { bc.push(0); }

    let format_hash: u32 = format_str.map(|s| fnv1a_64(s) as u32).unwrap_or(0);

    let section_name = format!(".zfmt_events.{:08x}", tag);
    let static_name = format_ident!("__ZFMT_EVENT_{:08X}", tag);

    let mut entry: Vec<u8> = Vec::new();
    entry.extend_from_slice(&tag.to_le_bytes());
    entry.extend_from_slice(&0u32.to_le_bytes());
    entry.extend_from_slice(&full_hash.to_le_bytes());
    entry.extend_from_slice(&format_hash.to_le_bytes());
    entry.extend_from_slice(&0u32.to_le_bytes());
    entry.extend_from_slice(&bc_len.to_le_bytes());
    entry.extend_from_slice(&bc);
    let entry_len = entry.len();
    let entry_lit = LitByteStr::new(&entry, Span::call_site());

    let string_section = crate::codegen::gen_string_section(format_str, suffix);

    Ok(quote! {
        #[used]
        #[cfg_attr(target_os = "none", link_section = #section_name)]
        #[cfg_attr(not(target_os = "none"), link_section = #section_name)]
        static #static_name: [u8; #entry_len] = *#entry_lit;

        #string_section
    })
}
