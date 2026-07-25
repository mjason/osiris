use super::*;

/// Lower a reader document into the surface AST.
#[must_use]
pub fn lower_document(document: &Document) -> LowerResult {
    let (forms, embedded_runtime_references, embedded_content_references, mut embedded_diagnostics) =
        embedded::prepare_forms(&document.forms);
    let mut lowerer = Lowerer {
        diagnostics: document
            .diagnostics
            .iter()
            .cloned()
            .chain(embedded_diagnostics.drain(..))
            .collect(),
        next_pattern_parameter: 0,
        embedded_runtime_references,
    };
    let mut module = Module {
        span: if document.forms.is_empty() {
            Span::new(0, document.source_len)
        } else {
            document
                .forms
                .iter()
                .map(|form| form.span)
                .reduce(Span::cover)
                .unwrap_or(Span::new(0, document.source_len))
        },
        metadata: Vec::new(),
        name: None,
        items: Vec::new(),
        embedded_content_references,
    };
    let mut saw_non_header = false;
    let mut saw_module_header = false;

    for form in &forms {
        if lowerer.is_head(form, "module") {
            if saw_module_header {
                lowerer.error(
                    AST_WRONG_SHAPE,
                    "module header may appear only once",
                    form.span,
                );
                continue;
            }
            saw_module_header = true;
            if saw_non_header {
                lowerer.error(
                    AST_WRONG_SHAPE,
                    "module header must precede top-level items",
                    form.span,
                );
            }
            let (name, metadata) = lowerer.lower_module_header(form);
            module.name = name;
            module.metadata = metadata;
            continue;
        }
        saw_non_header = true;
        module.items.push(lowerer.lower_item(form));
    }

    LowerResult {
        module,
        diagnostics: lowerer.diagnostics,
    }
}

struct Lowerer {
    diagnostics: Vec<Diagnostic>,
    next_pattern_parameter: usize,
    embedded_runtime_references: BTreeSet<String>,
}

#[derive(Default)]
struct MetadataTypeAnnotation {
    present: bool,
    annotation: Option<TypeExpr>,
}

mod bindings;
mod common;
mod contracts;
mod declarations;
mod embedded;
mod expressions;
mod static_declarations;
