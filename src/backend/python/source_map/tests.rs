use crate::{hir, macro_expand::ExpansionTrace, source::Span};

use super::{GenerateInput, generate};

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
    let map = generate(GenerateInput {
        source_name: "example.osr",
        generated_name: "example.py",
        generated_source: "from __future__ import annotations\n\nvalue = 1\n",
        module: &module,
        traces: &[],
        python_target: crate::project::PythonVersion::DEFAULT_TARGET,
        source_hash: "sha256:source",
        build_hash: "sha256:test",
    });

    assert_eq!(map.mappings.len(), 3);
    assert!(
        map.mappings
            .iter()
            .all(|mapping| mapping.source_span == module.span)
    );
    assert_eq!(map.mappings[2].generated_end.column, 9);
    assert_eq!(map.source_hash, "sha256:source");
    assert_eq!(map.language_version, crate::LANGUAGE_VERSION);
    assert_eq!(map.python_target, "3.11");
}

#[test]
fn standard_macro_definitions_remain_navigable_from_generated_python() {
    let module = hir::Module {
        name: "example".to_owned(),
        trust_policy_hash: format!("sha256:{}", "0".repeat(64)),
        span: Span::new(0, 20),
        metadata: Vec::new(),
        bindings: Vec::new(),
        aliases: Vec::new(),
        exports: Vec::new(),
        extern_functions: Vec::new(),
        items: Vec::new(),
    };
    let binding = crate::stdlib::find("osiris.core", "when").expect("standard when macro");
    let traces = [ExpansionTrace {
        macro_name: "when".to_owned(),
        macro_binding_id: binding.id().as_str().to_owned(),
        call_span: Span::new(4, 16),
        expansion_span: Span::new(4, 16),
        depth: 0,
        origin: vec![Span::new(4, 16)],
    }];
    let map = generate(GenerateInput {
        source_name: "example.osr",
        generated_name: "example.py",
        generated_source: "value = 1\n",
        module: &module,
        traces: &traces,
        python_target: crate::project::PythonVersion::DEFAULT_TARGET,
        source_hash: "sha256:source",
        build_hash: "sha256:test",
    });

    let origin = &map.mappings[0].macro_definitions[0];
    assert_eq!(origin.binding_id, binding.id().as_str());
    assert_eq!(origin.source, "osiris-stdlib:///osiris/core.osr");
    assert!(origin.line > 0);
}
