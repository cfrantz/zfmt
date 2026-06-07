use zfmt::Zfmt;

#[derive(Zfmt)]
#[zfmt(format = "{0:c}")]
pub struct Foo(pub u32);

#[derive(Zfmt)]
#[zfmt(format = "{0:c}")]
pub struct Bar(pub u32);

#[test]
fn test_duplicate_format() {
    let foo = Foo(0x41424344);
    let bar = Bar(0x45464748);
    assert_eq!(foo.0, 0x41424344);
    assert_eq!(bar.0, 0x45464748);
}
