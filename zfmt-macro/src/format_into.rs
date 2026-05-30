//! Generator for `FormatInto` trait impls (§11.4, Phase 3).
//!
//! Every `#[derive(Zfmt)]` type receives an `impl ::zfmt::FormatInto` block.
//! Events without a `#[zfmt(format = "...")]` attribute use the default no-op
//! implementation inherited from the trait.  Events with a format string get a
//! real implementation that renders each segment.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{spanned::Spanned, DeriveInput, Fields, Type};

use crate::{
    fmtstr::{parse_format_str, Align, FmtType, ParsedSpec, Segment},
    parse::{extract_format_str, is_nested_zfmt_type, parse_fields},
};

/// Generate the `impl ::zfmt::FormatInto` block for a derived type.
///
/// - If the type has a `#[zfmt(format = "...")]` attribute: generates a full
///   impl that renders each segment.
/// - Otherwise: generates an empty impl that uses the default no-op from the
///   trait.
///
/// Always returns `Some` — every derived type gets a `FormatInto` impl so that
/// `log_event!` can call `output::send_event` (which requires `E: FormatInto`).
pub fn generate(input: &DeriveInput) -> syn::Result<TokenStream> {
    let struct_name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let format_str = match extract_format_str(&input.attrs)? {
        Some(s) => s,
        None => {
            // No format string: emit an empty impl (uses the default no-op).
            return Ok(quote! {
                impl #impl_generics ::zfmt::FormatInto for #struct_name #ty_generics
                #where_clause {}
            });
        }
    };

    let fields_syn = match &input.data {
        syn::Data::Struct(s) => &s.fields,
        _ => {
            // Enums: no inherent format_into; each variant is its own event.
            return Ok(quote! {
                impl #impl_generics ::zfmt::FormatInto for #struct_name #ty_generics
                #where_clause {}
            });
        }
    };

    let parsed_fields = parse_fields(fields_syn)?;
    let field_names: Vec<&str> = parsed_fields.iter().map(|f| f.name.as_str()).collect();

    let segments = parse_format_str(&format_str).map_err(|e| {
        syn::Error::new(input.span(), format!("zfmt format string error: {}", e))
    })?;

    // Validate that all placeholder names refer to known fields.
    for seg in &segments {
        if let Segment::Placeholder(ph) = seg {
            if !field_names.contains(&ph.name.as_str()) {
                return Err(syn::Error::new(
                    input.span(),
                    format!(
                        "zfmt: unknown field `{}` in format string; known fields: {}",
                        ph.name,
                        field_names.join(", ")
                    ),
                ));
            }
        }
    }

    let stmts = segments
        .iter()
        .map(|seg| segment_to_stmt(seg, fields_syn))
        .collect::<syn::Result<Vec<_>>>()?;

    Ok(quote! {
        impl #impl_generics ::zfmt::FormatInto for #struct_name #ty_generics #where_clause {
            fn format_into<W: ::zfmt::Write>(
                &self,
                writer: &mut W,
            ) -> ::core::result::Result<(), ::zfmt::Error> {
                use ::zfmt::Format as _;
                #(#stmts)*
                Ok(())
            }
        }
    })
}

fn segment_to_stmt(seg: &Segment, fields: &Fields) -> syn::Result<TokenStream> {
    match seg {
        Segment::Literal(s) => Ok(quote! {
            writer.write_str(#s)?;
        }),
        Segment::Placeholder(ph) => {
            let field_access = field_access_expr(&ph.name, fields)?;
            // Nested Zfmt types implement FormatInto, not Format; call format_into.
            if field_ty_for_name(&ph.name, fields)
                .map(is_nested_zfmt_type)
                .unwrap_or(false)
            {
                Ok(quote! {
                    ::zfmt::FormatInto::format_into(&#field_access, writer)?;
                })
            } else {
                let spec_expr = spec_to_expr(&ph.spec);
                Ok(quote! {
                    ::zfmt::Format::fmt(&#field_access, writer, #spec_expr)?;
                })
            }
        }
    }
}

/// Look up the syn::Type of a field by name (or tuple index string).
fn field_ty_for_name<'a>(name: &str, fields: &'a Fields) -> Option<&'a Type> {
    match fields {
        Fields::Named(named) => named
            .named
            .iter()
            .find(|f| f.ident.as_ref().map(|i| i == name).unwrap_or(false))
            .map(|f| &f.ty),
        Fields::Unnamed(unnamed) => {
            let idx = name.parse::<usize>().ok()?;
            unnamed.unnamed.iter().nth(idx).map(|f| &f.ty)
        }
        Fields::Unit => None,
    }
}

/// Build a `self.field_name` or `self.0` token stream for a named or tuple field.
fn field_access_expr(name: &str, fields: &Fields) -> syn::Result<TokenStream> {
    match fields {
        Fields::Named(_) => {
            let ident = syn::Ident::new(name, proc_macro2::Span::call_site());
            Ok(quote! { self.#ident })
        }
        Fields::Unnamed(_) => {
            let idx: syn::Index = name.parse::<usize>()
                .map(syn::Index::from)
                .map_err(|_| syn::Error::new(
                    proc_macro2::Span::call_site(),
                    format!("zfmt: tuple field name `{}` is not a valid index", name),
                ))?;
            Ok(quote! { self.#idx })
        }
        Fields::Unit => Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "zfmt: unit structs cannot have format placeholders",
        )),
    }
}

/// Convert a `ParsedSpec` into a `::zfmt::FormatSpec { ... }` token stream.
pub(crate) fn spec_to_expr(spec: &ParsedSpec) -> TokenStream {
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
            ty:        #ty,
            alternate: #alternate,
            sign:      #sign,
            zero_pad:  #zero_pad,
            width:     #width,
            precision: #precision,
            align:     #align,
        }
    }
}
