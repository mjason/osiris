use super::*;

impl Printer {
    pub(super) fn print_expr(
        &mut self,
        expression: &Expr,
        parent_precedence: u8,
    ) -> Result<(), PrintError> {
        let precedence = expression_precedence(expression);
        let parenthesize = precedence < parent_precedence;
        if parenthesize {
            self.output.push('(');
        }

        match expression {
            Expr::Name(name) => self.output.push_str(name),
            Expr::Literal(literal) => self.print_literal(literal),
            Expr::Tuple(items) => self.print_tuple(items)?,
            Expr::List(items) => self.print_sequence('[', ']', items)?,
            Expr::Set(items) => {
                if items.is_empty() {
                    self.output.push_str("set()");
                } else {
                    self.print_sequence('{', '}', items)?;
                }
            }
            Expr::Dict(items) => self.print_dict(items)?,
            Expr::Attribute { value, attr } => {
                if needs_attribute_parentheses(value) {
                    self.output.push('(');
                    self.print_expr(value, 0)?;
                    self.output.push(')');
                } else {
                    self.print_expr(value, PREC_PRIMARY)?;
                }
                self.output.push('.');
                self.output.push_str(attr);
            }
            Expr::Subscript { value, slice } => {
                self.print_expr(value, PREC_PRIMARY)?;
                self.output.push('[');
                self.print_subscript_slice(slice)?;
                self.output.push(']');
            }
            Expr::Slice { .. } => {
                return Err(PrintError::new(
                    "a slice expression may only appear inside a subscript",
                ));
            }
            Expr::Call {
                function,
                arguments,
            } => {
                self.print_expr(function, PREC_PRIMARY)?;
                self.output.push('(');
                for (index, argument) in arguments.iter().enumerate() {
                    if index > 0 {
                        self.output.push_str(", ");
                    }
                    self.print_call_argument(argument)?;
                }
                self.output.push(')');
            }
            Expr::BoolOp { op, values } => {
                if values.len() < 2 {
                    return Err(PrintError::new(
                        "a boolean operation needs at least two operands",
                    ));
                }
                for (index, value) in values.iter().enumerate() {
                    if index > 0 {
                        self.output.push(' ');
                        self.output.push_str(op.text());
                        self.output.push(' ');
                    }
                    self.print_expr(value, precedence + 1)?;
                }
            }
            Expr::BinOp { left, op, right } => {
                if *op == BinaryOp::Power {
                    self.print_expr(left, precedence + 1)?;
                    self.output.push_str(" ** ");
                    self.print_expr(right, precedence)?;
                } else {
                    self.print_expr(left, precedence)?;
                    self.output.push(' ');
                    self.output.push_str(op.text());
                    self.output.push(' ');
                    self.print_expr(right, precedence + 1)?;
                }
            }
            Expr::UnaryOp { op, operand } => {
                self.output.push_str(op.text());
                self.print_expr(operand, precedence)?;
            }
            Expr::Compare { left, comparisons } => {
                if comparisons.is_empty() {
                    return Err(PrintError::new(
                        "a comparison needs at least one operator and right operand",
                    ));
                }
                self.print_expr(left, PREC_COMPARE + 1)?;
                for (op, right) in comparisons {
                    self.output.push(' ');
                    self.output.push_str(op.text());
                    self.output.push(' ');
                    self.print_expr(right, PREC_COMPARE + 1)?;
                }
            }
            Expr::IfExp { body, test, orelse } => {
                self.print_expr(body, PREC_IF_EXP + 1)?;
                self.output.push_str(" if ");
                self.print_expr(test, PREC_IF_EXP + 1)?;
                self.output.push_str(" else ");
                self.print_expr(orelse, PREC_IF_EXP)?;
            }
            Expr::Lambda { parameters, body } => {
                self.output.push_str("lambda");
                if !parameters_are_empty(parameters) {
                    self.output.push(' ');
                    self.print_parameters(parameters, true)?;
                }
                self.output.push_str(": ");
                self.print_expr(body, PREC_LAMBDA)?;
            }
            Expr::Starred(value) => {
                self.output.push('*');
                self.print_expr(value, PREC_UNARY)?;
            }
        }

        if parenthesize {
            self.output.push(')');
        }
        Ok(())
    }

    pub(super) fn print_literal(&mut self, literal: &Literal) {
        match literal {
            Literal::None => self.output.push_str("None"),
            Literal::Bool(value) => self.output.push_str(if *value { "True" } else { "False" }),
            Literal::Integer(value) => self.output.push_str(&value.to_string()),
            Literal::IntegerText(value) => self.output.push_str(value),
            Literal::Float(value) => self.print_float(*value),
            Literal::String(value) => self.print_string(value),
            Literal::Bytes(value) => self.print_bytes(value),
            Literal::Ellipsis => self.output.push_str("..."),
        }
    }

    pub(super) fn print_float(&mut self, value: f64) {
        if value.is_nan() {
            self.output.push_str("float(\"nan\")");
        } else if value == f64::INFINITY {
            self.output.push_str("float(\"inf\")");
        } else if value == f64::NEG_INFINITY {
            self.output.push_str("-float(\"inf\")");
        } else {
            let mut rendered = value.to_string();
            if !rendered.contains(['.', 'e', 'E']) {
                rendered.push_str(".0");
            }
            self.output.push_str(&rendered);
        }
    }

    pub(super) fn print_string(&mut self, value: &str) {
        self.output.push('"');
        for character in value.chars() {
            match character {
                '\\' => self.output.push_str("\\\\"),
                '"' => self.output.push_str("\\\""),
                '\n' => self.output.push_str("\\n"),
                '\r' => self.output.push_str("\\r"),
                '\t' => self.output.push_str("\\t"),
                '\u{08}' => self.output.push_str("\\b"),
                '\u{0c}' => self.output.push_str("\\f"),
                character if character <= '\u{1f}' || character == '\u{7f}' => {
                    self.output.push_str("\\x");
                    push_hex_byte(&mut self.output, character as u8);
                }
                character => self.output.push(character),
            }
        }
        self.output.push('"');
    }

    pub(super) fn print_bytes(&mut self, value: &[u8]) {
        self.output.push_str("b\"");
        for byte in value {
            match byte {
                b'\\' => self.output.push_str("\\\\"),
                b'"' => self.output.push_str("\\\""),
                b'\n' => self.output.push_str("\\n"),
                b'\r' => self.output.push_str("\\r"),
                b'\t' => self.output.push_str("\\t"),
                0x20..=0x7e => self.output.push(char::from(*byte)),
                _ => {
                    self.output.push_str("\\x");
                    push_hex_byte(&mut self.output, *byte);
                }
            }
        }
        self.output.push('"');
    }

    pub(super) fn print_tuple(&mut self, items: &[Expr]) -> Result<(), PrintError> {
        self.output.push('(');
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.output.push_str(", ");
            }
            self.print_expr(item, 0)?;
        }
        if items.len() == 1 {
            self.output.push(',');
        }
        self.output.push(')');
        Ok(())
    }

    pub(super) fn print_sequence(
        &mut self,
        open: char,
        close: char,
        items: &[Expr],
    ) -> Result<(), PrintError> {
        self.output.push(open);
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.output.push_str(", ");
            }
            self.print_expr(item, 0)?;
        }
        self.output.push(close);
        Ok(())
    }

    pub(super) fn print_dict(&mut self, items: &[DictItem]) -> Result<(), PrintError> {
        self.output.push('{');
        for (index, item) in items.iter().enumerate() {
            if index > 0 {
                self.output.push_str(", ");
            }
            match item {
                DictItem::Pair { key, value } => {
                    self.print_expr(key, 0)?;
                    self.output.push_str(": ");
                    self.print_expr(value, 0)?;
                }
                DictItem::Unpack(value) => {
                    self.output.push_str("**");
                    self.print_expr(value, 0)?;
                }
            }
        }
        self.output.push('}');
        Ok(())
    }

    pub(super) fn print_subscript_slice(&mut self, slice: &Expr) -> Result<(), PrintError> {
        if let Expr::Tuple(items) = slice {
            if items.is_empty() {
                self.output.push_str("()");
                return Ok(());
            }
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    self.output.push_str(", ");
                }
                self.print_slice_item(item)?;
            }
            if items.len() == 1 {
                self.output.push(',');
            }
            Ok(())
        } else {
            self.print_slice_item(slice)
        }
    }

    pub(super) fn print_slice_item(&mut self, slice: &Expr) -> Result<(), PrintError> {
        if let Expr::Slice { lower, upper, step } = slice {
            if let Some(lower) = lower {
                self.print_expr(lower, 0)?;
            }
            self.output.push(':');
            if let Some(upper) = upper {
                self.print_expr(upper, 0)?;
            }
            if let Some(step) = step {
                self.output.push(':');
                self.print_expr(step, 0)?;
            }
            Ok(())
        } else {
            self.print_expr(slice, 0)
        }
    }

    pub(super) fn print_call_argument(
        &mut self,
        argument: &CallArgument,
    ) -> Result<(), PrintError> {
        match argument {
            CallArgument::Positional(value) => self.print_expr(value, 0),
            CallArgument::Starred(value) => {
                self.output.push('*');
                self.print_expr(value, 0)
            }
            CallArgument::Keyword(keyword) => self.print_keyword(keyword),
        }
    }

    pub(super) fn print_keyword(&mut self, keyword: &KeywordArgument) -> Result<(), PrintError> {
        match keyword {
            KeywordArgument::Named { name, value } => {
                self.output.push_str(name);
                self.output.push('=');
                self.print_expr(value, 0)
            }
            KeywordArgument::Unpack(value) => {
                self.output.push_str("**");
                self.print_expr(value, 0)
            }
        }
    }

    pub(super) fn start_line(&mut self) {
        for _ in 0..self.indent {
            self.output.push_str("    ");
        }
    }

    pub(super) fn end_line(&mut self) {
        self.output.push('\n');
    }

    pub(super) fn line(&mut self, text: &str) {
        self.start_line();
        self.output.push_str(text);
        self.end_line();
    }
}
