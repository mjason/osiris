//! Stable binding identities and deterministic Python identifier allocation.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use unicode_normalization::UnicodeNormalization;

use crate::{
    diagnostic::Diagnostic,
    source::Span,
    syntax::{Form, FormKind},
};

pub const INVISIBLE_IDENTIFIER: &str = "OSR-N0100";
pub const CONFUSABLE_IDENTIFIER: &str = "OSR-N0101";
pub const MIXED_SCRIPT_IDENTIFIER: &str = "OSR-N0102";

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IdentifierLintKind {
    Invisible,
    Confusable,
    MixedScript,
}

/// An opt-in strict Unicode warning. These lints never participate in name
/// resolution; NFC/NFKC collisions remain the existing hard diagnostics.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IdentifierLint {
    pub code: &'static str,
    pub kind: IdentifierLintKind,
    pub message: String,
    pub span: Span,
}

/// Lints one source spelling according to the strict Unicode identifier
/// policy. Normal Chinese and the common Latin + East Asian combinations are
/// deliberately accepted.
#[must_use]
pub fn lint_identifier_strict(spelling: &str, span: Span) -> Vec<IdentifierLint> {
    let mut lints = Vec::new();
    if let Some((offset, character)) = spelling
        .char_indices()
        .find(|(_, character)| is_invisible_identifier_character(*character))
    {
        lints.push(IdentifierLint {
            code: INVISIBLE_IDENTIFIER,
            kind: IdentifierLintKind::Invisible,
            message: format!(
                "identifier contains invisible character U+{:04X}",
                u32::from(character)
            ),
            span: character_span(span, offset, character),
        });
    }
    if let Some((offset, character, ascii)) = spelling
        .char_indices()
        .filter(|(_, character)| !character.is_ascii())
        .find_map(|(offset, character)| {
            confusable_ascii(character).map(|ascii| (offset, character, ascii))
        })
    {
        lints.push(IdentifierLint {
            code: CONFUSABLE_IDENTIFIER,
            kind: IdentifierLintKind::Confusable,
            message: format!(
                "identifier character U+{:04X} is visually confusable with ASCII `{ascii}`",
                u32::from(character)
            ),
            span: character_span(span, offset, character),
        });
    }
    let scripts = spelling
        .chars()
        .filter_map(identifier_script)
        .collect::<BTreeSet<_>>();
    if is_high_risk_script_mix(&scripts) {
        lints.push(IdentifierLint {
            code: MIXED_SCRIPT_IDENTIFIER,
            kind: IdentifierLintKind::MixedScript,
            message: format!(
                "identifier mixes high-risk Unicode scripts: {}",
                scripts
                    .iter()
                    .map(ScriptGroup::name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            span,
        });
    }
    lints
}

/// Lints every retained name form, including names inside metadata.
#[must_use]
pub fn lint_forms_strict(forms: &[Form]) -> Vec<IdentifierLint> {
    let mut lints = Vec::new();
    for form in forms {
        collect_identifier_lints(form, &mut lints);
    }
    lints.sort_by(|left, right| {
        (left.span.start, left.span.end, left.code, left.kind).cmp(&(
            right.span.start,
            right.span.end,
            right.code,
            right.kind,
        ))
    });
    lints.dedup_by(|left, right| {
        left.code == right.code && left.span == right.span && left.kind == right.kind
    });
    lints
}

fn collect_identifier_lints(form: &Form, lints: &mut Vec<IdentifierLint>) {
    for entry in &form.metadata {
        collect_identifier_lints(&entry.key, lints);
        collect_identifier_lints(&entry.value, lints);
    }
    match &form.kind {
        FormKind::Keyword(name) | FormKind::Symbol(name) => {
            lints.extend(lint_identifier_strict(&name.spelling, form.datum_span));
        }
        FormKind::List(items)
        | FormKind::Vector(items)
        | FormKind::Map(items)
        | FormKind::Set(items) => {
            for item in items {
                collect_identifier_lints(item, lints);
            }
        }
        FormKind::ReaderMacro { form, .. } => collect_identifier_lints(form, lints),
        FormKind::None
        | FormKind::Bool(_)
        | FormKind::Integer(_)
        | FormKind::Float(_)
        | FormKind::String(_)
        | FormKind::Error(_) => {}
    }
}

fn character_span(span: Span, offset: usize, character: char) -> Span {
    let start = span.start.saturating_add(offset).min(span.end);
    Span::new(
        start,
        start.saturating_add(character.len_utf8()).min(span.end),
    )
}

fn is_invisible_identifier_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{034f}'
                | '\u{061c}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{fe00}'..='\u{fe0f}'
                | '\u{feff}'
                | '\u{e0100}'..='\u{e01ef}'
        )
}

fn confusable_ascii(character: char) -> Option<char> {
    let normalized = character.to_string().nfkc().collect::<String>();
    if normalized.len() == 1 {
        let normalized = normalized.chars().next()?;
        if normalized.is_ascii_alphanumeric() && normalized != character {
            return Some(normalized);
        }
    }
    Some(match character {
        'Α' | 'А' => 'A',
        'Β' | 'В' => 'B',
        'Ϲ' | 'С' => 'C',
        'Ε' | 'Е' => 'E',
        'Η' | 'Н' => 'H',
        'Ι' | 'І' => 'I',
        'Ј' => 'J',
        'Κ' | 'К' => 'K',
        'Μ' | 'М' => 'M',
        'Ν' => 'N',
        'Ο' | 'О' => 'O',
        'Ρ' | 'Р' => 'P',
        'Ѕ' => 'S',
        'Τ' | 'Т' => 'T',
        'Υ' | 'У' => 'Y',
        'Χ' | 'Х' => 'X',
        'α' | 'а' => 'a',
        'с' => 'c',
        'е' => 'e',
        'ι' | 'і' => 'i',
        'ј' => 'j',
        'κ' | 'к' => 'k',
        'ο' | 'о' => 'o',
        'ρ' | 'р' => 'p',
        'ѕ' => 's',
        'τ' => 't',
        'υ' | 'у' => 'y',
        'χ' | 'х' => 'x',
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ScriptGroup {
    Latin,
    Greek,
    Cyrillic,
    Han,
    Hiragana,
    Katakana,
    Hangul,
    Bopomofo,
    Arabic,
    Hebrew,
    Devanagari,
    Other,
}

impl ScriptGroup {
    const fn name(&self) -> &'static str {
        match self {
            Self::Latin => "Latin",
            Self::Greek => "Greek",
            Self::Cyrillic => "Cyrillic",
            Self::Han => "Han",
            Self::Hiragana => "Hiragana",
            Self::Katakana => "Katakana",
            Self::Hangul => "Hangul",
            Self::Bopomofo => "Bopomofo",
            Self::Arabic => "Arabic",
            Self::Hebrew => "Hebrew",
            Self::Devanagari => "Devanagari",
            Self::Other => "Other",
        }
    }
}

fn identifier_script(character: char) -> Option<ScriptGroup> {
    if !character.is_alphabetic() {
        return None;
    }
    let value = u32::from(character);
    Some(
        if character.is_ascii_alphabetic()
            || in_ranges(
                value,
                &[
                    (0x00c0, 0x02af),
                    (0x1d00, 0x1dbf),
                    (0x1e00, 0x1eff),
                    (0xab30, 0xab6f),
                    (0xff21, 0xff3a),
                    (0xff41, 0xff5a),
                ],
            )
        {
            ScriptGroup::Latin
        } else if in_ranges(value, &[(0x0370, 0x03ff), (0x1f00, 0x1fff)]) {
            ScriptGroup::Greek
        } else if in_ranges(
            value,
            &[
                (0x0400, 0x052f),
                (0x1c80, 0x1c8f),
                (0x2de0, 0x2dff),
                (0xa640, 0xa69f),
            ],
        ) {
            ScriptGroup::Cyrillic
        } else if in_ranges(
            value,
            &[
                (0x2e80, 0x2fff),
                (0x3400, 0x4dbf),
                (0x4e00, 0x9fff),
                (0xf900, 0xfaff),
                (0x20000, 0x323af),
            ],
        ) || matches!(value, 0x3005 | 0x3007 | 0x303b)
        {
            ScriptGroup::Han
        } else if in_ranges(value, &[(0x3040, 0x309f)]) {
            ScriptGroup::Hiragana
        } else if in_ranges(
            value,
            &[(0x30a0, 0x30ff), (0x31f0, 0x31ff), (0xff66, 0xff9d)],
        ) {
            ScriptGroup::Katakana
        } else if in_ranges(
            value,
            &[
                (0x1100, 0x11ff),
                (0x3130, 0x318f),
                (0xa960, 0xa97f),
                (0xac00, 0xd7af),
                (0xd7b0, 0xd7ff),
            ],
        ) {
            ScriptGroup::Hangul
        } else if in_ranges(value, &[(0x3100, 0x312f), (0x31a0, 0x31bf)]) {
            ScriptGroup::Bopomofo
        } else if in_ranges(
            value,
            &[
                (0x0600, 0x06ff),
                (0x0750, 0x077f),
                (0x08a0, 0x08ff),
                (0xfb50, 0xfdff),
                (0xfe70, 0xfeff),
            ],
        ) {
            ScriptGroup::Arabic
        } else if in_ranges(value, &[(0x0590, 0x05ff), (0xfb1d, 0xfb4f)]) {
            ScriptGroup::Hebrew
        } else if in_ranges(value, &[(0x0900, 0x097f), (0xa8e0, 0xa8ff)]) {
            ScriptGroup::Devanagari
        } else {
            ScriptGroup::Other
        },
    )
}

fn in_ranges(value: u32, ranges: &[(u32, u32)]) -> bool {
    ranges
        .iter()
        .any(|(start, end)| *start <= value && value <= *end)
}

fn is_high_risk_script_mix(scripts: &BTreeSet<ScriptGroup>) -> bool {
    if scripts.len() <= 1 {
        return false;
    }
    !scripts.iter().all(|script| {
        matches!(
            script,
            ScriptGroup::Latin
                | ScriptGroup::Han
                | ScriptGroup::Hiragana
                | ScriptGroup::Katakana
                | ScriptGroup::Hangul
                | ScriptGroup::Bopomofo
        )
    })
}

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
mod tests {
    use super::{
        BindingKind, CONFUSABLE_IDENTIFIER, INVISIBLE_IDENTIFIER, MIXED_SCRIPT_IDENTIFIER,
        NameAllocator, lint_forms_strict, lint_identifier_strict, python_identifier,
    };
    use crate::{reader::read, source::Span};

    #[test]
    fn maps_lisp_and_unicode_names_deterministically() {
        assert_eq!(python_identifier("rolling-mean"), "rolling_mean");
        assert_eq!(python_identifier("empty?"), "empty_p");
        assert_eq!(python_identifier("归一化数据"), "归一化数据");
        assert_eq!(python_identifier("class"), "class_");
    }

    #[test]
    fn aliases_share_the_target_binding() {
        let mut allocator = NameAllocator::default();
        let target = allocator
            .declare(
                "example",
                "rolling-mean",
                BindingKind::Function,
                Span::default(),
            )
            .expect("canonical declaration should succeed");
        allocator
            .alias("时序均值", &target, Span::default())
            .expect("alias should succeed");

        assert_eq!(allocator.resolve("时序均值"), Some(&target.id));
    }

    #[test]
    fn rejects_nfc_collisions() {
        let mut allocator = NameAllocator::default();
        allocator
            .declare("example", "e\u{301}", BindingKind::Value, Span::default())
            .expect("first spelling should succeed");
        let error = allocator
            .declare("example", "é", BindingKind::Value, Span::default())
            .expect_err("NFC-equivalent spelling must collide");
        assert_eq!(error.code, "OSR-N0001");
    }

    #[test]
    fn rejects_python_nfkc_collisions() {
        let mut allocator = NameAllocator::default();
        allocator
            .declare("example", "K", BindingKind::Value, Span::default())
            .expect("first spelling should succeed");
        let error = allocator
            .declare("example", "Ｋ", BindingKind::Value, Span::default())
            .expect_err("Python NFKC-equivalent spelling must collide");
        assert_eq!(error.code, "OSR-N0002");
    }

    #[test]
    fn strict_unicode_lint_accepts_chinese_and_east_asian_latin_names() {
        assert!(lint_identifier_strict("数据处理流程", Span::new(0, 18)).is_empty());
        assert!(lint_identifier_strict("API接口", Span::new(0, 9)).is_empty());
        assert!(lint_identifier_strict("価格Series", Span::new(0, 12)).is_empty());
    }

    #[test]
    fn strict_unicode_lint_reports_confusable_and_mixed_scripts() {
        let spelling = "pаypal"; // The second character is Cyrillic small a.
        let lints = lint_identifier_strict(spelling, Span::new(10, 10 + spelling.len()));
        let codes = lints.iter().map(|lint| lint.code).collect::<Vec<_>>();
        assert!(codes.contains(&CONFUSABLE_IDENTIFIER));
        assert!(codes.contains(&MIXED_SCRIPT_IDENTIFIER));
        assert_eq!(
            lints
                .iter()
                .find(|lint| lint.code == CONFUSABLE_IDENTIFIER)
                .expect("confusable warning")
                .span,
            Span::new(11, 13)
        );
    }

    #[test]
    fn strict_unicode_lint_reports_invisible_characters() {
        let spelling = "alpha\u{200d}";
        let lints = lint_identifier_strict(spelling, Span::new(4, 4 + spelling.len()));
        let invisible = lints
            .iter()
            .find(|lint| lint.code == INVISIBLE_IDENTIFIER)
            .expect("invisible warning");
        assert_eq!(invisible.span, Span::new(9, 12));
    }

    #[test]
    fn strict_unicode_lint_walks_recovered_source_forms() {
        let document = read("(def pаypal alpha\u{200d})");
        let lints = lint_forms_strict(&document.forms);
        assert!(lints.iter().any(|lint| lint.code == CONFUSABLE_IDENTIFIER));
        assert!(
            lints
                .iter()
                .any(|lint| lint.code == MIXED_SCRIPT_IDENTIFIER)
        );
        assert!(lints.iter().any(|lint| lint.code == INVISIBLE_IDENTIFIER));
    }
}
