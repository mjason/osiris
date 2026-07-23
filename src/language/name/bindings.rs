#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BindingKind {
    Module,
    Value,
    Function,
    Type,
    Field,
    Parameter,
    Macro,
    PythonModule,
}

impl BindingKind {
    const fn stable_tag(self) -> &'static str {
        match self {
            Self::Module => "module",
            Self::Value => "value",
            Self::Function => "function",
            Self::Type => "type",
            Self::Field => "field",
            Self::Parameter => "parameter",
            Self::Macro => "macro",
            Self::PythonModule => "python-module",
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BindingId(String);

impl BindingId {
    #[must_use]
    pub fn new(module: &str, canonical_name: &str, kind: BindingKind) -> Self {
        let module = module.nfc().collect::<String>();
        let name = canonical_name.nfc().collect::<String>();
        Self(format!("{module}::{}::{name}", kind.stable_tag()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Rehydrate a binding identity carried by a validated compilation
    /// interface.  Interface readers have already checked the canonical id;
    /// this constructor deliberately does not reinterpret or normalize it.
    #[must_use]
    pub fn from_interface(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BindingName {
    pub id: BindingId,
    pub canonical: String,
    pub python: String,
    pub kind: BindingKind,
    pub span: Span,
}

#[derive(Default)]
pub struct NameAllocator {
    source_names: BTreeMap<String, BindingId>,
    python_names: BTreeMap<String, BindingId>,
}

impl NameAllocator {
    pub fn declare(
        &mut self,
        module: &str,
        spelling: &str,
        kind: BindingKind,
        span: Span,
    ) -> Result<BindingName, Diagnostic> {
        let canonical = spelling.nfc().collect::<String>();
        let id = BindingId::new(module, &canonical, kind);
        if let Some(existing) = self.source_names.get(&canonical) {
            return Err(Diagnostic::error(
                "OSR-N0001",
                format!(
                    "name `{spelling}` collides after Unicode NFC normalization with `{}`",
                    existing.as_str()
                ),
                span,
            ));
        }

        let python = python_identifier(&canonical);
        let python_key = python.nfkc().collect::<String>();
        if let Some(existing) = self.python_names.get(&python_key) {
            return Err(Diagnostic::error(
                "OSR-N0002",
                format!(
                    "name `{spelling}` maps to Python identifier `{python}`, already used by `{}`",
                    existing.as_str()
                ),
                span,
            ));
        }

        self.source_names.insert(canonical.clone(), id.clone());
        self.python_names.insert(python_key, id.clone());
        Ok(BindingName {
            id,
            canonical,
            python,
            kind,
            span,
        })
    }

    pub fn alias(
        &mut self,
        spelling: &str,
        target: &BindingName,
        span: Span,
    ) -> Result<(), Diagnostic> {
        let canonical = spelling.nfc().collect::<String>();
        if let Some(existing) = self.source_names.get(&canonical) {
            if existing == &target.id {
                return Ok(());
            }
            return Err(Diagnostic::error(
                "OSR-N0003",
                format!("alias `{spelling}` conflicts with `{}`", existing.as_str()),
                span,
            ));
        }
        self.source_names.insert(canonical, target.id.clone());
        Ok(())
    }

    #[must_use]
    pub fn resolve(&self, spelling: &str) -> Option<&BindingId> {
        self.source_names.get(&spelling.nfc().collect::<String>())
    }
}

/// Maps an Osiris identifier to a deterministic Python identifier.
#[must_use]
pub fn python_identifier(name: &str) -> String {
    let mut result = String::new();
    for character in name.nfc() {
        match character {
            '-' => result.push('_'),
            '?' => result.push_str("_p"),
            '!' => result.push_str("_bang"),
            character if character == '_' || character.is_alphanumeric() => {
                result.push(character);
            }
            character => {
                use std::fmt::Write;
                let _ = write!(result, "_u{:x}_", u32::from(character));
            }
        }
    }

    if result.is_empty() {
        result.push_str("_osiris_empty");
    }
    if result
        .chars()
        .next()
        .is_some_and(|character| character.is_numeric())
    {
        result.insert(0, '_');
    }
    if is_python_keyword(&result) {
        result.push('_');
    }
    result
}

fn is_python_keyword(name: &str) -> bool {
    matches!(
        name,
        "False"
            | "None"
            | "True"
            | "and"
            | "as"
            | "assert"
            | "async"
            | "await"
            | "break"
            | "class"
            | "continue"
            | "def"
            | "del"
            | "elif"
            | "else"
            | "except"
            | "finally"
            | "for"
            | "from"
            | "global"
            | "if"
            | "import"
            | "in"
            | "is"
            | "lambda"
            | "nonlocal"
            | "not"
            | "or"
            | "pass"
            | "raise"
            | "return"
            | "try"
            | "while"
            | "with"
            | "yield"
    )
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
