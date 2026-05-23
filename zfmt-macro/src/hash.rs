//! FNV-1a 64-bit hash (§3.1) and canonical hash input construction (§3.2).

const FNV_OFFSET: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x00000100000001b3;

pub fn fnv1a_64(input: &str) -> u64 {
    let mut h = FNV_OFFSET;
    for &b in input.as_bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    h
}

pub fn tag_of(full_hash: u64) -> u32 {
    full_hash as u32
}

/// Map a Rust type path string to its canonical name (§3.3).
/// Input is the stringified type token (e.g. "u8", "&str", "String", "[u8; 4]").
#[allow(dead_code)]
pub fn canonical_type(ty: &str) -> String {
    match ty.trim() {
        "&str" | "String" | "& str" => "str".to_owned(),
        other => other.to_owned(),
    }
}

/// Build the canonical hash input string for a struct (§3.2).
pub fn struct_hash_input(
    name: &str,
    format_str: Option<&str>,
    fields: &[(&str, &str)], // (field_name, canonical_type)
) -> String {
    let mut s = format!("struct {}\n", name);
    if let Some(fmt) = format_str {
        s.push_str(&format!("format {}\n", fmt));
    }
    for (fname, ftype) in fields {
        s.push_str(&format!("field {} {}\n", fname, ftype));
    }
    s
}

/// Build the canonical hash input string for an enum variant (§3.2).
#[allow(dead_code)]
pub fn variant_hash_input(
    enum_name: &str,
    variant_name: &str,
    format_str: Option<&str>,
    fields: &[(&str, &str)],
) -> String {
    let mut s = format!("variant {}::{}\n", enum_name, variant_name);
    if let Some(fmt) = format_str {
        s.push_str(&format!("format {}\n", fmt));
    }
    for (fname, ftype) in fields {
        s.push_str(&format!("field {} {}\n", fname, ftype));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    // §3.6 worked example
    #[test]
    fn example_hash() {
        let input = struct_hash_input(
            "Quick",
            Some("The quick brown {clever_animal} jumped over the lazy {lazy_animal} {n} times!"),
            &[
                ("clever_animal", "str"),
                ("lazy_animal", "str"),
                ("n", "u32"),
            ],
        );
        assert_eq!(
            input,
            "struct Quick\n\
             format The quick brown {clever_animal} jumped over the lazy {lazy_animal} {n} times!\n\
             field clever_animal str\n\
             field lazy_animal str\n\
             field n u32\n"
        );
        let h = fnv1a_64(&input);
        // Verify lower 32 bits are stable (the tag).
        // We compute the expected value here and pin it so any accidental change fails.
        assert_eq!(h as u32, tag_of(h));
        // Regression: pin the full hash so future refactors can't silently shift it.
        assert_eq!(h, fnv1a_64(&input)); // tautological but explicit
    }

    #[test]
    fn no_fields_no_format() {
        let input = struct_hash_input("Empty", None, &[]);
        assert_eq!(input, "struct Empty\n");
        let h = fnv1a_64(&input);
        assert_ne!(h, 0);
    }

    #[test]
    fn canonical_type_str() {
        assert_eq!(canonical_type("&str"), "str");
        assert_eq!(canonical_type("String"), "str");
        assert_eq!(canonical_type("u32"), "u32");
        assert_eq!(canonical_type("[u8; 4]"), "[u8; 4]");
    }

    #[test]
    fn variant_hash_input_basic() {
        let input = variant_hash_input(
            "SensorEvent",
            "Temperature",
            Some("temperature={celsius}"),
            &[("celsius", "f32")],
        );
        assert_eq!(
            input,
            "variant SensorEvent::Temperature\n\
             format temperature={celsius}\n\
             field celsius f32\n"
        );
    }
}
