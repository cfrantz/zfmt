//! Code generation for `__zfmt_log_text!` (§13.3 unstructured text events).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse::ParseStream, Expr, Ident, LitStr, Token};

struct LogTextArgs {
    logger: Expr,
    severity: Expr,
    format: LitStr,
    bindings: Vec<(Ident, Expr)>,
}

impl syn::parse::Parse for LogTextArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let logger: Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let severity: Expr = input.parse()?;
        input.parse::<Token![,]>()?;
        let format: LitStr = input.parse()?;

        let mut bindings = Vec::new();
        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }
            let name: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let val: Expr = input.parse()?;
            bindings.push((name, val));
        }

        Ok(LogTextArgs { logger, severity, format, bindings })
    }
}

pub fn generate(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let args: LogTextArgs = match syn::parse(input) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };

    let format_str = args.format.value();
    let segments = match crate::fmtstr::parse_format_str(&format_str) {
        Ok(s) => s,
        Err(e) => {
            return syn::Error::new(args.format.span(), format!("zfmt: {}", e))
                .to_compile_error()
                .into();
        }
    };

    // Let-bind named arguments so they can be referenced by format placeholders.
    let binding_stmts: Vec<TokenStream> = args.bindings.iter().map(|(name, val)| {
        quote! { let #name = #val; }
    }).collect();

    // Span of the format string literal — used for all generated identifiers so
    // that they resolve in the user's call-site scope rather than the synthetic
    // hygiene scope created by the enclosing macro_rules! expansion.
    let fmt_span = args.format.span();

    // Generate write statements for each segment.
    let write_stmts: Vec<TokenStream> = segments.iter().map(|seg| {
        match seg {
            crate::fmtstr::Segment::Literal(s) => quote! {
                let _ = _zfmt_buf.write_str(#s);
            },
            crate::fmtstr::Segment::Placeholder(ph) => {
                let name = proc_macro2::Ident::new(&ph.name, fmt_span);
                let spec = crate::format_into::spec_to_expr(&ph.spec);
                quote! {
                    let _ = ::zfmt::Format::fmt(&#name, &mut _zfmt_buf, #spec);
                }
            }
        }
    }).collect();

    let logger = &args.logger;
    let severity = &args.severity;

    // Generate a direct binary send of EventHeader + DebugMessage.
    // This deliberately bypasses log_event! and the output-mode feature flags
    // so that unstructured text events are always emitted as binary DebugMessage
    // records regardless of the output-mode setting.
    quote! {{
        #(#binding_stmts)*
        let mut _zfmt_buf = ::zfmt::FixedBuf::<128>::new();
        {
            use ::zfmt::Write as _;
            #(#write_stmts)*
        }
        let _zfmt_msg = ::zfmt::events::DebugMessage { message: _zfmt_buf.as_str() };
        let ref _logger = #logger;
        let _ts  = ::zfmt::Logger::timestamp(&*_logger);
        let _seq = ::zfmt::Logger::next_seq(&*_logger);
        let _hdr = ::zfmt::events::EventHeader::new(_ts, #severity, _seq);
        let _hpl = ::zfmt::ZfmtEvent::payload_size(&_hdr) as u32;
        let _epl = ::zfmt::ZfmtEvent::payload_size(&_zfmt_msg) as u32;
        let mut _frm = [0u8; 34];
        let mut _n = 0usize;
        _frm[_n.._n + 4].copy_from_slice(&::zfmt::ZfmtEvent::zfmt_tag(&_hdr).to_le_bytes());
        _n += 4;
        _n += ::zfmt::leb128::encode(_hpl, &mut _frm[_n..]);
        ::zfmt::ZfmtEvent::with_payload_bytes(&_hdr, |_hb| {
            _frm[_n.._n + _hb.len()].copy_from_slice(_hb);
            _n += _hb.len();
        });
        _frm[_n.._n + 4].copy_from_slice(&::zfmt::ZfmtEvent::zfmt_tag(&_zfmt_msg).to_le_bytes());
        _n += 4;
        _n += ::zfmt::leb128::encode(_epl, &mut _frm[_n..]);
        let _fl = _n;
        ::zfmt::ZfmtEvent::with_payload_bytes(&_zfmt_msg, |_eb| {
            ::zfmt::Logger::send_vectored(_logger, &[&_frm[.._fl], _eb]);
        });
    }}.into()
}
