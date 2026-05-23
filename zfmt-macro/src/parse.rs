//! Parse the input to #[derive(Zfmt)] and the #[zfmt(...)] attributes.

use proc_macro2::Span;
use syn::{
    punctuated::Punctuated, spanned::Spanned, Attribute, DeriveInput, Expr, ExprLit, Field,
    Fields, Lit, Meta, Token, Type,
};

/// Parsed representation of a single struct/variant field.
pub struct FieldInfo {
    pub name: String,          // "foo" for named, "0" for tuple
    pub canonical_type: String,
    #[allow(dead_code)]
    pub rust_type: Type,
    #[allow(dead_code)]
    pub span: Span,
}

/// Parsed representation of the whole derive target.
#[allow(dead_code)]
pub enum ParsedInput {
    Struct {
        name: String,
        format_str: Option<String>,
        fields: Vec<FieldInfo>,
    },
}

/// Extract the `format = "..."` value from `#[zfmt(format = "...")]`.
pub fn extract_format_str(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    for attr in attrs {
        if !attr.path().is_ident("zfmt") {
            continue;
        }
        let nested =
            attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
        for meta in nested {
            if let Meta::NameValue(nv) = meta {
                if nv.path.is_ident("format") {
                    if let Expr::Lit(ExprLit { lit: Lit::Str(s), .. }) = &nv.value {
                        return Ok(Some(s.value()));
                    } else {
                        return Err(syn::Error::new(
                            nv.value.span(),
                            "zfmt: `format` value must be a string literal",
                        ));
                    }
                }
            }
        }
    }
    Ok(None)
}

/// Convert a syn::Type to its canonical type name string (§3.3).
pub fn canonical_type_str(ty: &Type) -> String {
    match ty {
        Type::Reference(r) => {
            if let Type::Path(p) = &*r.elem {
                if p.path.is_ident("str") {
                    return "str".to_owned();
                }
            }
            // fall through to stringify
        }
        Type::Path(p) => {
            if p.path.is_ident("String") {
                return "str".to_owned();
            }
            if let Some(ident) = p.path.get_ident() {
                // Simple ident — return as-is (covers u8, u16, ... bool, char, custom)
                return ident.to_string();
            }
        }
        Type::Array(a) => {
            let elem = canonical_type_str(&a.elem);
            // Extract the constant N from Expr::Lit
            if let Expr::Lit(ExprLit { lit: Lit::Int(n), .. }) = &a.len {
                return format!("[{}; {}]", elem, n.base10_digits());
            }
            // Fallback: stringify
            return format!("[{}; ?]", elem);
        }
        _ => {}
    }
    // Generic fallback: quote-stringify
    quote::quote!(#ty).to_string().replace(' ', "")
}

/// Parse fields from a syn::Fields, assigning names.
pub fn parse_fields(fields: &Fields) -> syn::Result<Vec<FieldInfo>> {
    let mut out = Vec::new();
    match fields {
        Fields::Named(named) => {
            for f in &named.named {
                let name = f
                    .ident
                    .as_ref()
                    .map(|i| i.to_string())
                    .unwrap_or_default();
                // Skip padding fields (names starting with `_`)
                if name.starts_with('_') {
                    continue;
                }
                let canonical_type = canonical_type_str(&f.ty);
                out.push(FieldInfo {
                    name,
                    canonical_type,
                    rust_type: f.ty.clone(),
                    span: f.span(),
                });
            }
        }
        Fields::Unnamed(unnamed) => {
            for (i, f) in unnamed.unnamed.iter().enumerate() {
                let canonical_type = canonical_type_str(&f.ty);
                out.push(FieldInfo {
                    name: i.to_string(),
                    canonical_type,
                    rust_type: f.ty.clone(),
                    span: f.span(),
                });
            }
        }
        Fields::Unit => {}
    }
    Ok(out)
}

/// Check whether a field is a padding field (name starts with `_`).
pub fn is_padding_field(field: &Field) -> bool {
    field
        .ident
        .as_ref()
        .map(|i| i.to_string().starts_with('_'))
        .unwrap_or(false)
}

#[allow(dead_code)]
pub fn parse_struct(input: &DeriveInput) -> syn::Result<ParsedInput> {
    let name = input.ident.to_string();
    let format_str = extract_format_str(&input.attrs)?;
    let fields_syn = match &input.data {
        syn::Data::Struct(s) => &s.fields,
        _ => {
            return Err(syn::Error::new(
                input.span(),
                "zfmt: only structs are supported in Phase 2 (enums come in Phase 5)",
            ))
        }
    };
    let fields = parse_fields(fields_syn)?;
    Ok(ParsedInput::Struct { name, format_str, fields })
}
