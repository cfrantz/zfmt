//! Format string lexer and specifier parser (§10, Phase 3).
//!
//! A format string consists of literal segments and `{name}` / `{name:spec}`
//! placeholders.  This module splits a format string into a `Vec<Segment>`.


/// One segment of a parsed format string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Literal(String),
    Placeholder(Placeholder),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placeholder {
    pub name: String,
    pub spec: ParsedSpec,
}

/// The parsed contents of a `:spec` clause (all fields have defaults).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParsedSpec {
    pub align: Align,
    pub sign: bool,
    pub alternate: bool,
    pub zero_pad: bool,
    pub width: Option<u8>,
    pub precision: Option<u8>,
    pub fmt_type: FmtType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    None,
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FmtType {
    #[default]
    Display,
    LowerHex,
    UpperHex,
    Binary,
    Octal,
    /// FourCC character display (§10.2): bytes in little-endian order,
    /// printable ASCII (0x20–0x7E) as characters, others as `\xNN`.
    Char,
}

// ---------------------------------------------------------------------------
// Lexer

/// Parse `input` into a list of `Segment`s.  Returns an error string on
/// invalid syntax (caller converts to a `syn::Error` using the format string's span).
pub fn parse_format_str(input: &str) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut chars = input.char_indices().peekable();
    let mut literal = String::new();

    while let Some((i, ch)) = chars.next() {
        match ch {
            '{' => {
                // `{{` is an escaped brace.
                if chars.peek().map(|(_, c)| *c) == Some('{') {
                    chars.next();
                    literal.push('{');
                    continue;
                }
                // Start of placeholder: collect until `}`.
                if !literal.is_empty() {
                    segments.push(Segment::Literal(core::mem::take(&mut literal)));
                }
                let mut inner = String::new();
                let mut found_close = false;
                for (_, c) in chars.by_ref() {
                    if c == '}' {
                        found_close = true;
                        break;
                    }
                    inner.push(c);
                }
                if !found_close {
                    return Err(format!("unclosed `{{` in format string at byte {}", i));
                }
                segments.push(Segment::Placeholder(parse_placeholder(&inner)?));
            }
            '}' => {
                // `}}` is an escaped brace.
                if chars.peek().map(|(_, c)| *c) == Some('}') {
                    chars.next();
                    literal.push('}');
                } else {
                    return Err("unexpected `}` in format string (escape as `}}`".to_owned());
                }
            }
            _ => literal.push(ch),
        }
    }

    if !literal.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    Ok(segments)
}

/// Parse the inside of `{...}` — either `name` or `name:spec`.
fn parse_placeholder(inner: &str) -> Result<Placeholder, String> {
    let (name, spec_str) = match inner.find(':') {
        Some(pos) => (&inner[..pos], Some(&inner[pos + 1..])),
        None => (inner, None),
    };
    let name = name.trim().to_owned();
    if name.is_empty() {
        return Err("placeholder name must not be empty (positional `{}` is not supported; use a field name)".to_owned());
    }
    let spec = match spec_str {
        Some(s) => parse_spec(s)?,
        None => ParsedSpec::default(),
    };
    Ok(Placeholder { name, spec })
}

/// Parse a format specifier string of the form `[align][sign][#][0][width][.precision][type]`.
fn parse_spec(s: &str) -> Result<ParsedSpec, String> {
    let mut spec = ParsedSpec::default();
    let mut it = s.chars().peekable();

    // align: `<` or `>`
    if let Some(&c) = it.peek() {
        if c == '<' || c == '>' {
            spec.align = if c == '<' { Align::Left } else { Align::Right };
            it.next();
        }
    }

    // sign: `+`
    if it.peek() == Some(&'+') {
        spec.sign = true;
        it.next();
    }

    // alternate: `#`
    if it.peek() == Some(&'#') {
        spec.alternate = true;
        it.next();
    }

    // zero-pad: `0`
    if it.peek() == Some(&'0') {
        // peek ahead — if followed by a digit it's zero-pad+width, else just width starts
        spec.zero_pad = true;
        it.next();
    }

    // width: digits
    let mut width_str = String::new();
    while matches!(it.peek(), Some(&c) if c.is_ascii_digit()) {
        width_str.push(it.next().unwrap());
    }
    if !width_str.is_empty() {
        spec.width = Some(
            width_str
                .parse::<u8>()
                .map_err(|_| format!("width `{}` exceeds 255", width_str))?,
        );
    }

    // precision: `.N`
    if it.peek() == Some(&'.') {
        it.next();
        let mut prec_str = String::new();
        while matches!(it.peek(), Some(&c) if c.is_ascii_digit()) {
            prec_str.push(it.next().unwrap());
        }
        if prec_str.is_empty() {
            return Err("`.` in format spec must be followed by a digit".to_owned());
        }
        spec.precision = Some(
            prec_str
                .parse::<u8>()
                .map_err(|_| format!("precision `{}` exceeds 255", prec_str))?,
        );
    }

    // type char
    match it.next() {
        None => {}
        Some('x') => spec.fmt_type = FmtType::LowerHex,
        Some('X') => spec.fmt_type = FmtType::UpperHex,
        Some('b') => spec.fmt_type = FmtType::Binary,
        Some('o') => spec.fmt_type = FmtType::Octal,
        Some('c') => spec.fmt_type = FmtType::Char,
        Some(c) => return Err(format!("unknown format type `{}`", c)),
    }

    // Nothing should remain.
    if it.next().is_some() {
        return Err(format!("unexpected characters after type in format spec `{}`", s));
    }

    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(s: &str) -> Segment {
        Segment::Literal(s.to_owned())
    }
    fn ph(name: &str) -> Segment {
        Segment::Placeholder(Placeholder { name: name.to_owned(), spec: ParsedSpec::default() })
    }
    #[allow(dead_code)]
    fn ph_spec(name: &str, spec: ParsedSpec) -> Segment {
        Segment::Placeholder(Placeholder { name: name.to_owned(), spec })
    }

    #[test]
    fn all_literal() {
        assert_eq!(parse_format_str("hello").unwrap(), vec![lit("hello")]);
    }

    #[test]
    fn single_placeholder() {
        assert_eq!(parse_format_str("{x}").unwrap(), vec![ph("x")]);
    }

    #[test]
    fn mixed() {
        assert_eq!(
            parse_format_str("val={x} ok").unwrap(),
            vec![lit("val="), ph("x"), lit(" ok")]
        );
    }

    #[test]
    fn escaped_braces() {
        assert_eq!(parse_format_str("{{}}").unwrap(), vec![lit("{}")] );
    }

    #[test]
    fn spec_hex_lower() {
        let segs = parse_format_str("{addr:x}").unwrap();
        if let Segment::Placeholder(p) = &segs[0] {
            assert_eq!(p.spec.fmt_type, FmtType::LowerHex);
        } else {
            panic!("expected placeholder");
        }
    }

    #[test]
    fn spec_full() {
        let segs = parse_format_str("{n:>+#08.3x}").unwrap();
        if let Segment::Placeholder(p) = &segs[0] {
            assert_eq!(p.spec.align, Align::Right);
            assert!(p.spec.sign);
            assert!(p.spec.alternate);
            assert!(p.spec.zero_pad);
            assert_eq!(p.spec.width, Some(8));
            assert_eq!(p.spec.precision, Some(3));
            assert_eq!(p.spec.fmt_type, FmtType::LowerHex);
        } else {
            panic!("expected placeholder");
        }
    }

    #[test]
    fn empty_name_is_error() {
        assert!(parse_format_str("{}").is_err());
    }

    #[test]
    fn unclosed_brace_is_error() {
        assert!(parse_format_str("{x").is_err());
    }

    #[test]
    fn spec_char_fourcc() {
        let segs = parse_format_str("{tag:c}").unwrap();
        if let Segment::Placeholder(p) = &segs[0] {
            assert_eq!(p.spec.fmt_type, FmtType::Char);
        } else {
            panic!("expected placeholder");
        }
    }

    #[test]
    fn unknown_type_is_error() {
        assert!(parse_format_str("{x:z}").is_err());
    }
}
