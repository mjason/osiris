//! Deterministic generated-Python to Osiris source mappings.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    artifact::{GeneratedPosition, SourceMap, SourceMapping},
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
#[must_use]
pub fn generate(
    source_name: impl Into<String>,
    generated_name: impl Into<String>,
    generated_source: &str,
    module: &hir::Module,
    traces: &[ExpansionTrace],
    build_hash: &str,
) -> SourceMap {
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
        });
    }

    SourceMap {
        version: 1,
        source: source_name.into(),
        generated: generated_name.into(),
        trust_policy_hash: module.trust_policy_hash.clone(),
        build_hash: build_hash.to_owned(),
        mappings,
    }
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
