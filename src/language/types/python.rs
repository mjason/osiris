use super::*;

/// A Python interpreter target used for compatibility-sensitive typing names.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PythonVersion {
    pub major: u8,
    pub minor: u8,
}

impl PythonVersion {
    pub const PYTHON_3_9: Self = Self::new(3, 9);

    #[must_use]
    pub const fn new(major: u8, minor: u8) -> Self {
        Self { major, minor }
    }

    #[must_use]
    pub const fn at_least(self, major: u8, minor: u8) -> bool {
        self.major > major || (self.major == major && self.minor >= minor)
    }
}

/// One `from module import name` required by a generated annotation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PythonTypingImport {
    pub module: &'static str,
    pub name: &'static str,
}

impl PythonTypingImport {
    #[must_use]
    pub const fn new(module: &'static str, name: &'static str) -> Self {
        Self { module, name }
    }

    #[must_use]
    pub const fn typing(name: &'static str) -> Self {
        Self::new("typing", name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PythonTypeError {
    Unresolved(Box<Type>),
}

impl fmt::Display for PythonTypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolved(ty) => write!(formatter, "cannot emit unresolved type `{ty}`"),
        }
    }
}

impl Error for PythonTypeError {}
