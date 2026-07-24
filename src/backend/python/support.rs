use super::*;

#[derive(Clone, Debug)]
pub(super) struct Lowered {
    pub(super) prefix: Vec<py::Stmt>,
    pub(super) value: Option<py::Expr>,
}

impl Lowered {
    pub(super) fn value(value: py::Expr) -> Self {
        Self {
            prefix: Vec::new(),
            value: Some(value),
        }
    }
}

pub(super) fn unary(
    mut values: Vec<py::Expr>,
    op: py::UnaryOp,
    _prefix: &mut Vec<py::Stmt>,
    name: &str,
) -> Result<py::Expr, BackendError> {
    if values.len() != 1 {
        return Err(BackendError::new(
            format!("{name} expects one operand"),
            None,
        ));
    }
    Ok(py::Expr::UnaryOp {
        op,
        operand: Box::new(values.remove(0)),
    })
}

pub(super) fn is_safe_dataclass_default(value: &py::Expr) -> bool {
    match value {
        py::Expr::Literal(
            py::Literal::None
            | py::Literal::Bool(_)
            | py::Literal::Integer(_)
            | py::Literal::Float(_)
            | py::Literal::String(_),
        ) => true,
        py::Expr::Tuple(items) => items.iter().all(is_safe_dataclass_default),
        _ => false,
    }
}
