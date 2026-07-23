use super::*;

/// Closed scalar operators supplied by the core prelude. Extension-owned
/// nominal instances live in interfaces and are not added to this table.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScalarOperator {
    Add,
    Subtract,
    Multiply,
    TrueDivide,
    FloorDivide,
    Remainder,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
    Negate,
    Positive,
    Abs,
}

impl ScalarOperator {
    /// Stable wire spelling used by `.osri` and the `:osiris/operator`
    /// declaration metadata.  This is intentionally independent of Rust's
    /// enum debug representation.
    #[must_use]
    pub const fn stable_name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Subtract => "subtract",
            Self::Multiply => "multiply",
            Self::TrueDivide => "true-divide",
            Self::FloorDivide => "floor-divide",
            Self::Remainder => "remainder",
            Self::Less => "less",
            Self::LessEqual => "less-equal",
            Self::Greater => "greater",
            Self::GreaterEqual => "greater-equal",
            Self::Equal => "equal",
            Self::NotEqual => "not-equal",
            Self::Negate => "negate",
            Self::Positive => "positive",
            Self::Abs => "abs",
        }
    }

    /// Parse the closed metadata vocabulary.  `divide`/`/` are accepted as a
    /// convenience spelling but normalize to the canonical `true-divide` id.
    #[must_use]
    pub fn from_stable_name(name: &str) -> Option<Self> {
        Some(match name.trim_start_matches(':') {
            "add" | "+" => Self::Add,
            "subtract" | "sub" | "-" => Self::Subtract,
            "multiply" | "mul" | "*" => Self::Multiply,
            "true-divide" | "divide" | "div" | "/" => Self::TrueDivide,
            "floor-divide" | "floor" | "//" => Self::FloorDivide,
            "remainder" | "mod" | "%" => Self::Remainder,
            "less" | "<" => Self::Less,
            "less-equal" | "<=" => Self::LessEqual,
            "greater" | ">" => Self::Greater,
            "greater-equal" | ">=" => Self::GreaterEqual,
            "equal" | "=" | "==" => Self::Equal,
            "not-equal" | "not=" | "!=" => Self::NotEqual,
            "negate" => Self::Negate,
            "positive" => Self::Positive,
            "abs" => Self::Abs,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OperatorSignature {
    pub operator: ScalarOperator,
    pub operands: Vec<Type>,
    pub result: Type,
    pub summaries: CallSummaries,
}

/// A closed, statically selected operator implementation published by a
/// module.  `binding` identifies the implementation callable; `owner_binding`
/// identifies a public nominal type owned by the declaring module and enforces
/// the orphan rule.  No runtime dispatch is implied by this data structure.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OperatorInstance {
    pub id: String,
    pub binding: String,
    pub owner_binding: String,
    pub operator: ScalarOperator,
    pub operands: Vec<Type>,
    pub result: Type,
    pub summaries: CallSummaries,
}

impl OperatorInstance {
    #[must_use]
    pub fn new(
        binding: impl Into<String>,
        owner_binding: impl Into<String>,
        operator: ScalarOperator,
        operands: Vec<Type>,
        result: Type,
        summaries: CallSummaries,
    ) -> Self {
        let binding = binding.into();
        let owner_binding = owner_binding.into();
        let id = format!("{}::operator::{}", binding, operator.stable_name());
        Self {
            id,
            binding,
            owner_binding,
            operator,
            operands,
            result,
            summaries,
        }
    }

    #[must_use]
    pub fn function_type(&self) -> FunctionType {
        FunctionType::new(self.operands.clone(), self.result.clone())
            .with_summaries(self.summaries.clone())
    }
}

impl OperatorSignature {
    #[must_use]
    fn pure(operator: ScalarOperator, operands: Vec<Type>, result: Type) -> Self {
        Self {
            operator,
            operands,
            result,
            summaries: CallSummaries::pure_scalar(),
        }
    }

    #[must_use]
    pub fn function_type(&self) -> FunctionType {
        FunctionType::new(self.operands.clone(), self.result.clone())
            .with_summaries(self.summaries.clone())
    }
}

/// Returns the deterministic core scalar overload table for one operator.
#[must_use]
pub fn scalar_operator_signatures(operator: ScalarOperator) -> Vec<OperatorSignature> {
    use ScalarOperator::{
        Abs, Add, Equal, FloorDivide, Greater, GreaterEqual, Less, LessEqual, Multiply, Negate,
        NotEqual, Positive, Remainder, Subtract, TrueDivide,
    };

    match operator {
        Add | Subtract | Multiply | FloorDivide | Remainder => {
            let mut signatures = numeric_binary_signatures(operator, false);
            if operator == Add {
                signatures.push(OperatorSignature::pure(
                    operator,
                    vec![Type::Str, Type::Str],
                    Type::Str,
                ));
                signatures.push(OperatorSignature::pure(
                    operator,
                    vec![Type::Bytes, Type::Bytes],
                    Type::Bytes,
                ));
            }
            signatures
        }
        TrueDivide => numeric_binary_signatures(operator, true),
        Less | LessEqual | Greater | GreaterEqual => {
            let mut signatures = numeric_comparison_signatures(operator);
            for ty in [Type::Str, Type::Bytes] {
                signatures.push(OperatorSignature::pure(
                    operator,
                    vec![ty.clone(), ty],
                    Type::Bool,
                ));
            }
            signatures
        }
        Equal | NotEqual => {
            let mut signatures = numeric_comparison_signatures(operator);
            for ty in [Type::Bool, Type::Str, Type::Bytes, Type::None] {
                signatures.push(OperatorSignature::pure(
                    operator,
                    vec![ty.clone(), ty],
                    Type::Bool,
                ));
            }
            signatures
        }
        Negate | Positive | Abs => vec![
            OperatorSignature::pure(operator, vec![Type::Int], Type::Int),
            OperatorSignature::pure(operator, vec![Type::Float], Type::Float),
        ],
    }
}

fn numeric_binary_signatures(
    operator: ScalarOperator,
    always_float: bool,
) -> Vec<OperatorSignature> {
    [
        (Type::Int, Type::Int),
        (Type::Int, Type::Float),
        (Type::Float, Type::Int),
        (Type::Float, Type::Float),
    ]
    .into_iter()
    .map(|(left, right)| {
        let result = if always_float || left == Type::Float || right == Type::Float {
            Type::Float
        } else {
            Type::Int
        };
        OperatorSignature::pure(operator, vec![left, right], result)
    })
    .collect()
}

fn numeric_comparison_signatures(operator: ScalarOperator) -> Vec<OperatorSignature> {
    numeric_binary_signatures(operator, false)
        .into_iter()
        .map(|mut signature| {
            signature.result = Type::Bool;
            signature
        })
        .collect()
}
