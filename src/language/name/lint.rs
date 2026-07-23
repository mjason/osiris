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

pub(crate) fn contains_cjk(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(
            character as u32,
            0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff
        )
    })
}

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
