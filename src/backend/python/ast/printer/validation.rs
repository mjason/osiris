use super::*;

pub(super) fn is_definition(statement: &Stmt) -> bool {
    matches!(statement, Stmt::FunctionDef(_) | Stmt::ClassDef(_))
}

pub(super) fn validate_parameters(parameters: &Parameters, lambda: bool) -> Result<(), PrintError> {
    let mut saw_default = false;
    for parameter in parameters
        .positional_only
        .iter()
        .chain(&parameters.positional)
    {
        if parameter.default.is_some() {
            saw_default = true;
        } else if saw_default {
            return Err(PrintError::new(
                "a required positional parameter cannot follow one with a default",
            ));
        }
    }

    for special in [parameters.vararg.as_ref(), parameters.kwarg.as_ref()]
        .into_iter()
        .flatten()
    {
        if special.default.is_some() {
            return Err(PrintError::new(
                "variadic parameters cannot have default values",
            ));
        }
    }

    if lambda
        && parameters
            .positional_only
            .iter()
            .chain(&parameters.positional)
            .chain(parameters.vararg.iter())
            .chain(&parameters.keyword_only)
            .chain(parameters.kwarg.iter())
            .any(|parameter| parameter.annotation.is_some())
    {
        return Err(PrintError::new(
            "lambda parameters cannot contain annotations",
        ));
    }
    Ok(())
}

pub(super) fn parameters_are_empty(parameters: &Parameters) -> bool {
    parameters.positional_only.is_empty()
        && parameters.positional.is_empty()
        && parameters.vararg.is_none()
        && parameters.keyword_only.is_empty()
        && parameters.kwarg.is_none()
}

pub(super) fn expression_precedence(expression: &Expr) -> u8 {
    match expression {
        Expr::Lambda { .. } => PREC_LAMBDA,
        Expr::IfExp { .. } => PREC_IF_EXP,
        Expr::BoolOp {
            op: BooleanOp::Or, ..
        } => PREC_OR,
        Expr::BoolOp {
            op: BooleanOp::And, ..
        } => PREC_AND,
        Expr::UnaryOp {
            op: UnaryOp::Not, ..
        } => PREC_NOT,
        Expr::Compare { .. } => PREC_COMPARE,
        Expr::BinOp { op, .. } => op.precedence(),
        Expr::UnaryOp { .. } | Expr::Starred(_) => PREC_UNARY,
        Expr::Attribute { .. } | Expr::Subscript { .. } | Expr::Call { .. } => PREC_PRIMARY,
        Expr::Literal(Literal::Integer(value)) if *value < 0 => PREC_UNARY,
        Expr::Literal(Literal::IntegerText(value)) if value.starts_with('-') => PREC_UNARY,
        Expr::Literal(Literal::Float(value)) if value.is_sign_negative() => PREC_UNARY,
        Expr::Name(_)
        | Expr::Literal(_)
        | Expr::Tuple(_)
        | Expr::List(_)
        | Expr::Set(_)
        | Expr::Dict(_)
        | Expr::Slice { .. } => PREC_ATOM,
    }
}

pub(super) fn needs_attribute_parentheses(expression: &Expr) -> bool {
    matches!(
        expression,
        Expr::Literal(Literal::Integer(_) | Literal::IntegerText(_))
    )
}

pub(super) fn push_hex_byte(output: &mut String, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    output.push(char::from(HEX[usize::from(byte >> 4)]));
    output.push(char::from(HEX[usize::from(byte & 0x0f)]));
}

impl BooleanOp {
    pub(super) const fn text(self) -> &'static str {
        match self {
            Self::And => "and",
            Self::Or => "or",
        }
    }
}

impl BinaryOp {
    pub(super) const fn text(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::MatrixMultiply => "@",
            Self::Divide => "/",
            Self::FloorDivide => "//",
            Self::Modulo => "%",
            Self::Power => "**",
            Self::LeftShift => "<<",
            Self::RightShift => ">>",
            Self::BitAnd => "&",
            Self::BitXor => "^",
            Self::BitOr => "|",
        }
    }

    const fn precedence(self) -> u8 {
        match self {
            Self::BitOr => PREC_BIT_OR,
            Self::BitXor => PREC_BIT_XOR,
            Self::BitAnd => PREC_BIT_AND,
            Self::LeftShift | Self::RightShift => PREC_SHIFT,
            Self::Add | Self::Subtract => PREC_ADD,
            Self::Multiply
            | Self::MatrixMultiply
            | Self::Divide
            | Self::FloorDivide
            | Self::Modulo => PREC_MULTIPLY,
            Self::Power => PREC_POWER,
        }
    }
}

impl UnaryOp {
    pub(super) const fn text(self) -> &'static str {
        match self {
            Self::Positive => "+",
            Self::Negative => "-",
            Self::Not => "not ",
            Self::Invert => "~",
        }
    }
}

impl CompareOp {
    pub(super) const fn text(self) -> &'static str {
        match self {
            Self::Equal => "==",
            Self::NotEqual => "!=",
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Greater => ">",
            Self::GreaterEqual => ">=",
            Self::In => "in",
            Self::NotIn => "not in",
            Self::Is => "is",
            Self::IsNot => "is not",
        }
    }
}
