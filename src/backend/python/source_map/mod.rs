//! Deterministic generated-Python to Osiris source mappings.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    artifact::{GeneratedPosition, MacroDefinitionOrigin, SourceMap, SourceMapping},
    hir::{self, ItemKind},
    macro_expand::ExpansionTrace,
    source::Span,
};

/// Builds a line-level source map for a generated Python module.
///
/// Top-level declarations are recognized by their canonical Python binding
/// name. Compiler-generated imports and helpers conservatively map to the
/// complete module span. This gives traceback and editor consumers a useful,
/// deterministic baseline while allowing the backend to add expression-level
/// positions later without changing the artifact format.
pub struct GenerateInput<'a> {
    pub source_name: &'a str,
    pub generated_name: &'a str,
    pub generated_source: &'a str,
    pub module: &'a hir::Module,
    pub traces: &'a [ExpansionTrace],
    pub python_target: crate::types::PythonVersion,
    pub source_hash: &'a str,
    pub build_hash: &'a str,
}

#[must_use]
pub fn generate(input: GenerateInput<'_>) -> SourceMap {
    let GenerateInput {
        source_name,
        generated_name,
        generated_source,
        module,
        traces,
        python_target,
        source_hash,
        build_hash,
    } = input;
    let declarations = declaration_markers(module);
    let mut current_span = module.span;
    let mut mappings = Vec::new();

    for (line_index, line) in generated_source.lines().enumerate() {
        if !line.starts_with(char::is_whitespace) {
            current_span = declarations
                .iter()
                .find_map(|(marker, span)| line.starts_with(marker).then_some(*span))
                .unwrap_or(module.span);
        }
        let origin = expansion_origins(current_span, traces);
        let macro_definitions = macro_definition_origins(current_span, traces);
        mappings.push(SourceMapping {
            generated_start: GeneratedPosition {
                line: line_index + 1,
                column: 0,
            },
            generated_end: GeneratedPosition {
                line: line_index + 1,
                column: line.chars().count(),
            },
            source_span: current_span,
            expansion_origin: origin,
            macro_definitions,
        });
    }

    SourceMap {
        version: 3,
        language_version: crate::LANGUAGE_VERSION.to_owned(),
        python_target: python_target.to_string(),
        source: source_name.to_owned(),
        source_hash: source_hash.to_owned(),
        generated: generated_name.to_owned(),
        trust_policy_hash: module.trust_policy_hash.clone(),
        build_hash: build_hash.to_owned(),
        mappings,
    }
}

fn macro_definition_origins(span: Span, traces: &[ExpansionTrace]) -> Vec<MacroDefinitionOrigin> {
    traces
        .iter()
        .filter(|trace| {
            spans_overlap(trace.call_span, span) || spans_overlap(trace.expansion_span, span)
        })
        .filter_map(|trace| {
            let binding = crate::stdlib::NAMESPACES
                .iter()
                .flat_map(|namespace| crate::stdlib::exports(namespace))
                .find(|binding| binding.id().as_str() == trace.macro_binding_id)?;
            let source = crate::stdlib::api_record(binding).source;
            Some(MacroDefinitionOrigin {
                binding_id: trace.macro_binding_id.clone(),
                source: source.uri,
                line: source.line,
                column: source.column,
            })
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn spans_overlap(left: Span, right: Span) -> bool {
    left.start <= right.end && right.start <= left.end
}

fn declaration_markers(module: &hir::Module) -> Vec<(String, Span)> {
    let bindings = module
        .bindings
        .iter()
        .map(|binding| (binding.name.id.clone(), binding.name.python.as_str()))
        .collect::<BTreeMap<_, _>>();
    let mut markers = Vec::new();
    for item in &module.items {
        match &item.kind {
            ItemKind::Function(function) => {
                if let Some(name) = bindings.get(&function.binding) {
                    markers.push((format!("def {name}("), item.span));
                    markers.push((format!("async def {name}("), item.span));
                }
            }
            ItemKind::Struct(structure) => {
                if let Some(name) = bindings.get(&structure.binding) {
                    markers.push((format!("class {name}"), item.span));
                }
            }
            ItemKind::Value(value) => {
                if let Some(name) = bindings.get(&value.binding) {
                    markers.push((format!("{name}:"), item.span));
                    markers.push((format!("{name} ="), item.span));
                }
            }
            ItemKind::Import(import) => {
                markers.push((
                    format!("import {}", import.module.replace('/', ".")),
                    item.span,
                ));
            }
            ItemKind::Expr(_) | ItemKind::StaticSchema(_) | ItemKind::StaticRecord(_) => {}
        }
    }
    markers.sort_by(|left, right| {
        right
            .0
            .len()
            .cmp(&left.0.len())
            .then_with(|| left.0.cmp(&right.0))
    });
    markers
}

fn expansion_origins(span: Span, traces: &[ExpansionTrace]) -> Vec<Span> {
    let mut origins = BTreeSet::new();
    for trace in traces {
        if trace.call_span.start >= span.start && trace.call_span.end <= span.end {
            origins.extend(trace.origin.iter().map(|origin| (origin.start, origin.end)));
        }
    }
    origins
        .into_iter()
        .map(|(start, end)| Span::new(start, end))
        .collect()
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
