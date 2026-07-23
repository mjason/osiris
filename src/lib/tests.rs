#[test]
fn version_comes_from_cargo_metadata() {
    assert_eq!(super::version(), env!("CARGO_PKG_VERSION"));
}
