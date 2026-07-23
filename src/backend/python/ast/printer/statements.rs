use super::*;

impl Printer {
    pub(super) fn print_module(&mut self, module: &Module) -> Result<(), PrintError> {
        let mut previous_was_definition = false;
        for (index, statement) in module.body.iter().enumerate() {
            let is_definition = is_definition(statement);
            if index > 0 && (previous_was_definition || is_definition) {
                self.output.push_str("\n\n");
            }
            self.print_stmt(statement)?;
            previous_was_definition = is_definition;
        }
        Ok(())
    }

    pub(super) fn print_stmt(&mut self, statement: &Stmt) -> Result<(), PrintError> {
        match statement {
            Stmt::Import(import) => self.print_import(import),
            Stmt::Assign(assign) => self.print_assign(assign),
            Stmt::AnnAssign(assign) => self.print_ann_assign(assign),
            Stmt::AugAssign(assign) => self.print_aug_assign(assign),
            Stmt::Expr(expression) => {
                self.start_line();
                self.print_expr(expression, 0)?;
                self.end_line();
                Ok(())
            }
            Stmt::FunctionDef(function) => self.print_function(function),
            Stmt::Return(value) => {
                self.start_line();
                self.output.push_str("return");
                if let Some(value) = value {
                    self.output.push(' ');
                    self.print_expr(value, 0)?;
                }
                self.end_line();
                Ok(())
            }
            Stmt::If(statement) => self.print_if(statement, false),
            Stmt::ClassDef(class) => self.print_class(class),
            Stmt::Try(statement) => self.print_try(statement),
            Stmt::Raise(statement) => self.print_raise(statement),
            Stmt::Assert { test, message } => {
                self.start_line();
                self.output.push_str("assert ");
                self.print_expr(test, 0)?;
                if let Some(message) = message {
                    self.output.push_str(", ");
                    self.print_expr(message, 0)?;
                }
                self.end_line();
                Ok(())
            }
            Stmt::Pass => {
                self.line("pass");
                Ok(())
            }
            Stmt::Break => {
                self.line("break");
                Ok(())
            }
            Stmt::Continue => {
                self.line("continue");
                Ok(())
            }
        }
    }

    pub(super) fn print_import(&mut self, import: &Import) -> Result<(), PrintError> {
        self.start_line();
        match import {
            Import::Direct(names) => {
                if names.is_empty() {
                    return Err(PrintError::new("an import must contain at least one name"));
                }
                self.output.push_str("import ");
                self.print_import_aliases(names);
            }
            Import::From {
                module,
                names,
                level,
            } => {
                if names.is_empty() {
                    return Err(PrintError::new(
                        "a from-import must contain at least one name",
                    ));
                }
                if module.is_none() && *level == 0 {
                    return Err(PrintError::new(
                        "a from-import needs a module or a relative import level",
                    ));
                }
                self.output.push_str("from ");
                for _ in 0..*level {
                    self.output.push('.');
                }
                if let Some(module) = module {
                    self.output.push_str(module);
                }
                self.output.push_str(" import ");
                self.print_import_aliases(names);
            }
        }
        self.end_line();
        Ok(())
    }

    pub(super) fn print_import_aliases(&mut self, aliases: &[ImportAlias]) {
        for (index, alias) in aliases.iter().enumerate() {
            if index > 0 {
                self.output.push_str(", ");
            }
            self.output.push_str(&alias.name);
            if let Some(as_name) = &alias.as_name {
                self.output.push_str(" as ");
                self.output.push_str(as_name);
            }
        }
    }

    pub(super) fn print_assign(&mut self, assign: &Assign) -> Result<(), PrintError> {
        if assign.targets.is_empty() {
            return Err(PrintError::new(
                "an assignment must contain at least one target",
            ));
        }
        self.start_line();
        for target in &assign.targets {
            self.print_expr(target, 0)?;
            self.output.push_str(" = ");
        }
        self.print_expr(&assign.value, 0)?;
        self.end_line();
        Ok(())
    }

    pub(super) fn print_ann_assign(&mut self, assign: &AnnAssign) -> Result<(), PrintError> {
        self.start_line();
        self.print_expr(&assign.target, 0)?;
        self.output.push_str(": ");
        self.print_expr(&assign.annotation, 0)?;
        if let Some(value) = &assign.value {
            self.output.push_str(" = ");
            self.print_expr(value, 0)?;
        }
        self.end_line();
        Ok(())
    }

    pub(super) fn print_aug_assign(&mut self, assign: &AugAssign) -> Result<(), PrintError> {
        self.start_line();
        self.print_expr(&assign.target, 0)?;
        self.output.push(' ');
        self.output.push_str(assign.op.text());
        self.output.push_str("= ");
        self.print_expr(&assign.value, 0)?;
        self.end_line();
        Ok(())
    }

    pub(super) fn print_function(&mut self, function: &FunctionDef) -> Result<(), PrintError> {
        for decorator in &function.decorators {
            self.start_line();
            self.output.push('@');
            self.print_expr(decorator, 0)?;
            self.end_line();
        }
        self.start_line();
        if function.is_async {
            self.output.push_str("async ");
        }
        self.output.push_str("def ");
        self.output.push_str(&function.name);
        self.output.push('(');
        self.print_parameters(&function.parameters, false)?;
        self.output.push(')');
        if let Some(returns) = &function.returns {
            self.output.push_str(" -> ");
            self.print_expr(returns, 0)?;
        }
        self.output.push_str(":\n");
        self.print_suite(&function.body)
    }

    pub(super) fn print_parameters(
        &mut self,
        parameters: &Parameters,
        lambda: bool,
    ) -> Result<(), PrintError> {
        validate_parameters(parameters, lambda)?;
        let mut needs_separator = false;

        for parameter in &parameters.positional_only {
            self.parameter_separator(&mut needs_separator);
            self.print_parameter(parameter, lambda)?;
        }
        if !parameters.positional_only.is_empty() {
            self.parameter_separator(&mut needs_separator);
            self.output.push('/');
        }
        for parameter in &parameters.positional {
            self.parameter_separator(&mut needs_separator);
            self.print_parameter(parameter, lambda)?;
        }
        if let Some(vararg) = &parameters.vararg {
            self.parameter_separator(&mut needs_separator);
            self.output.push('*');
            self.print_parameter(vararg, lambda)?;
        } else if !parameters.keyword_only.is_empty() {
            self.parameter_separator(&mut needs_separator);
            self.output.push('*');
        }
        for parameter in &parameters.keyword_only {
            self.parameter_separator(&mut needs_separator);
            self.print_parameter(parameter, lambda)?;
        }
        if let Some(kwarg) = &parameters.kwarg {
            self.parameter_separator(&mut needs_separator);
            self.output.push_str("**");
            self.print_parameter(kwarg, lambda)?;
        }
        Ok(())
    }

    pub(super) fn parameter_separator(&mut self, needs_separator: &mut bool) {
        if *needs_separator {
            self.output.push_str(", ");
        }
        *needs_separator = true;
    }

    pub(super) fn print_parameter(
        &mut self,
        parameter: &Parameter,
        lambda: bool,
    ) -> Result<(), PrintError> {
        self.output.push_str(&parameter.name);
        if let Some(annotation) = &parameter.annotation {
            if lambda {
                return Err(PrintError::new(
                    "lambda parameters cannot contain annotations",
                ));
            }
            self.output.push_str(": ");
            self.print_expr(annotation, 0)?;
        }
        if let Some(default) = &parameter.default {
            self.output.push_str(" = ");
            self.print_expr(default, 0)?;
        }
        Ok(())
    }

    pub(super) fn print_if(&mut self, statement: &IfStmt, elif: bool) -> Result<(), PrintError> {
        self.start_line();
        self.output.push_str(if elif { "elif " } else { "if " });
        self.print_expr(&statement.test, 0)?;
        self.output.push_str(":\n");
        self.print_suite(&statement.body)?;

        if let [Stmt::If(nested)] = statement.orelse.as_slice() {
            self.print_if(nested, true)?;
        } else if !statement.orelse.is_empty() {
            self.line("else:");
            self.print_suite(&statement.orelse)?;
        }
        Ok(())
    }

    pub(super) fn print_class(&mut self, class: &ClassDef) -> Result<(), PrintError> {
        for decorator in &class.decorators {
            self.start_line();
            self.output.push('@');
            self.print_expr(decorator, 0)?;
            self.end_line();
        }
        self.start_line();
        self.output.push_str("class ");
        self.output.push_str(&class.name);
        if !class.bases.is_empty() || !class.keywords.is_empty() {
            self.output.push('(');
            let mut separator = false;
            for base in &class.bases {
                if separator {
                    self.output.push_str(", ");
                }
                self.print_expr(base, 0)?;
                separator = true;
            }
            for keyword in &class.keywords {
                if separator {
                    self.output.push_str(", ");
                }
                self.print_keyword(keyword)?;
                separator = true;
            }
            self.output.push(')');
        }
        self.output.push_str(":\n");
        self.print_suite(&class.body)
    }

    pub(super) fn print_try(&mut self, statement: &Try) -> Result<(), PrintError> {
        if statement.handlers.is_empty() && statement.finalbody.is_empty() {
            return Err(PrintError::new(
                "a try statement needs an except or finally clause",
            ));
        }
        self.line("try:");
        self.print_suite(&statement.body)?;
        for handler in &statement.handlers {
            self.start_line();
            self.output.push_str("except");
            match (&handler.exception_type, &handler.name) {
                (Some(exception_type), name) => {
                    self.output.push(' ');
                    self.print_expr(exception_type, 0)?;
                    if let Some(name) = name {
                        self.output.push_str(" as ");
                        self.output.push_str(name);
                    }
                }
                (None, Some(_)) => {
                    return Err(PrintError::new("a bare except handler cannot bind a name"));
                }
                (None, None) => {}
            }
            self.output.push_str(":\n");
            self.print_suite(&handler.body)?;
        }
        if !statement.orelse.is_empty() {
            self.line("else:");
            self.print_suite(&statement.orelse)?;
        }
        if !statement.finalbody.is_empty() {
            self.line("finally:");
            self.print_suite(&statement.finalbody)?;
        }
        Ok(())
    }

    pub(super) fn print_raise(&mut self, statement: &Raise) -> Result<(), PrintError> {
        if statement.exception.is_none() && statement.cause.is_some() {
            return Err(PrintError::new("a raise cause needs an exception"));
        }
        self.start_line();
        self.output.push_str("raise");
        if let Some(exception) = &statement.exception {
            self.output.push(' ');
            self.print_expr(exception, 0)?;
        }
        if let Some(cause) = &statement.cause {
            self.output.push_str(" from ");
            self.print_expr(cause, 0)?;
        }
        self.end_line();
        Ok(())
    }

    pub(super) fn print_suite(&mut self, statements: &[Stmt]) -> Result<(), PrintError> {
        self.indent += 1;
        if statements.is_empty() {
            self.line("pass");
        } else {
            for statement in statements {
                self.print_stmt(statement)?;
            }
        }
        self.indent -= 1;
        Ok(())
    }
}
