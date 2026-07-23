use crate::{hir, source::Span};

use super::generate;

#[test]
fn compiler_lines_fall_back_to_the_module_span() {
    let module = hir::Module {
        name: "example".to_owned(),
        trust_policy_hash: format!("sha256:{}", "0".repeat(64)),
        span: Span::new(3, 20),
        metadata: Vec::new(),
        bindings: Vec::new(),
        aliases: Vec::new(),
        exports: Vec::new(),
        extern_functions: Vec::new(),
        items: Vec::new(),
    };
    let map = generate(
        "example.osr",
        "example.py",
        "from __future__ import annotations\n\nvalue = 1\n",
        &module,
        &[],
        "sha256:test",
    );

    assert_eq!(map.mappings.len(), 3);
    assert!(
        map.mappings
            .iter()
            .all(|mapping| mapping.source_span == module.span)
    );
    assert_eq!(map.mappings[2].generated_end.column, 9);
}
