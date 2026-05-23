# zfmt: Low overhead event logging for embedded systems

`zfmt` is a rust-based low-overhead event logging library for embedded systems.
`zfmt` events are rust structs that can be serialized and deserialized from a
byte stream.  The `zfmt` crate allows you to describe the formatting (printing)
rules for these structs so that firmware or a higher-level log receiver can print
the events in a human readable form.

To achieve this, `zfmt` includes a proc-macro that derives the formatting trait
for structs and build string tables and formatting bytecode into separate read-only
linker sections.  Building the string and bytecode tables into separate linker sections
allows a firmware image to discard those sections if it does not wish to perform
the formatting in the firmware.

The `zfmt` proc-macro allows you to declare your printing rules in a rust-idomatic style:

```rust
#[derive(zfmt)]
#[zfmt(format = "The quick brown {clever_animal} jumped over the lazy {lazy_animal} {n} times!")
pub struct Quick {
    pub clever_animal: String,
    pub lazy_animal: String,
    pub n: u32,
}


#[derive(zfmt)]
pub enum Pets {
    #[zfmt(format = "There were {0} cats")]
    Cats(u32),
    #[zfmt(format = "There were {0} dogs")]
    Dogs(u32),
}
```

The proc-macro generates an identifier tag for each struct which is typically a hash of
TO-BE-DETERMINED as well as byte-code that describes the serialized form.

## Byte Code

The byte code consists of a opcode followed by an optional operand describing the item
to be displayed.

The opcode is a byte, split into two 4-bit fields: an item-type field and an operand-type field:

### Item Types

- 0: end of byte-code subroutine (ie: `end` or `return`)
- 1: u8
- 2: u16
- 3: u32
- 4: u64
- 5: i8
- 6: i16
- 7: i32
- 8: i64
- 9: utf-8 byte
- 10: undefined
- 11: undefined
- 12: undefined
- 13: undefined
- 14: undefined
- 15: call another byte-code subroutine

### Operand types

- 0: item-type refers to a single item of that type; no operand.
- 1: item-type is an array of items; operand is the length of the array.
- 2: item-type is a zero-terminated array of items.  The array has a fixed size, but the first zero in the array reprsents a terminator for display purposes.  The operand is the length of the array.

### Encoding of operands

In the bytecode stream, operands are encoded as LEB128 bytes.
