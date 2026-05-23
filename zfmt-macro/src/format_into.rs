//! Generator for `format_into<W: Write>` methods (§11.4, Phase 3).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{spanned::Spanned, DeriveInput, Fields};

use crate::{
    fmtstr::{parse_format_str, Align, FmtType, ParsedSpec, Segment},
    parse::{extract_format_str, parse_fields},
};

/// Generate the `format_into` impl block for a struct that carries a format string.
/// Returns `None` if there is no `#[zfmt(format = "...")]` attribute.
pub fn maybe_generate(input: &DeriveInput) -> syn::Result<Option<TokenStream>> {
    let format_str = match extract_format_str(&input.attrs)? {
        Some(s) => s,
        None => return Ok(None),
    };

    let struct_name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let fields_syn = match &input.data {
        syn::Data::Struct(s) => &s.fields,
        _ => return Ok(None),
    };

    let parsed_fields = parse_fields(fields_syn)?;
    let field_names: Vec<&str> = parsed_fields.iter().map(|f| f.name.as_str()).collect();

    let segments = parse_format_str(&format_str).map_err(|e| {
        syn::Error::new(input.span(), format!("zfmt format string error: {}", e))
    })?;

    // Validate all placeholder names refer to known fields.
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

    Ok(Some(quote! {
        impl #impl_generics #struct_name #ty_generics #where_clause {
            pub fn format_into<W: ::zfmt::Write>(
                &self,
                writer: &mut W,
            ) -> ::core::result::Result<(), ::zfmt::Error> {
                use ::zfmt::Format as _;
                #(#stmts)*
                Ok(())
            }
        }
    }))
}

fn segment_to_stmt(seg: &Segment, fields: &Fields) -> syn::Result<TokenStream> {
    match seg {
        Segment::Literal(s) => Ok(quote! {
            writer.write_str(#s)?;
        }),
        Segment::Placeholder(ph) => {
            let field_access = field_access_expr(&ph.name, fields)?;
            let spec_expr = spec_to_expr(&ph.spec);
            Ok(quote! {
                ::zfmt::Format::fmt(&#field_access, writer, #spec_expr)?;
            })
        }
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
fn spec_to_expr(spec: &ParsedSpec) -> TokenStream {
    let ty = match spec.fmt_type {
        FmtType::Display  => quote! { ::zfmt::FormatType::Display },
        FmtType::LowerHex => quote! { ::zfmt::FormatType::LowerHex },
        FmtType::UpperHex => quote! { ::zfmt::FormatType::UpperHex },
        FmtType::Binary   => quote! { ::zfmt::FormatType::Binary },
        FmtType::Octal    => quote! { ::zfmt::FormatType::Octal },
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
