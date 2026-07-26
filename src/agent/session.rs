use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use oxilangtag::LanguageTag;
use serde::{Deserialize, Serialize};

const MAX_SESSION_BYTES: u64 = 1024 * 1024;
pub(super) const MAX_SESSION_TURNS: usize = 100;
const SESSION_SCHEMA: &str = "osiris-lsa-session/v1";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SessionFile {
    pub(super) schema: String,
    pub(super) session_id: String,
    #[serde(default)]
    pub(super) locale: Option<String>,
    #[serde(default)]
    pub(super) turns: Vec<SessionTurn>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SessionTurn {
    pub(super) role: String,
    pub(super) content: String,
}

pub(super) fn load_session(path: &Path, session_id: &str) -> Result<SessionFile, String> {
    if !path.is_file() {
        return Ok(SessionFile {
            schema: SESSION_SCHEMA.to_owned(),
            session_id: session_id.to_owned(),
            ..SessionFile::default()
        });
    }
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    if metadata.len() > MAX_SESSION_BYTES {
        return Err("LSA session exceeded the 1 MiB limit".to_owned());
    }
    let source = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let session: SessionFile = json5::from_str(&source).map_err(|error| error.to_string())?;
    if session.schema != SESSION_SCHEMA {
        return Err(format!(
            "unsupported LSA session schema `{}`",
            session.schema
        ));
    }
    if session.session_id != session_id {
        return Err("session file id does not match requested session".to_owned());
    }
    validate_session(&session)?;
    Ok(session)
}

pub(super) fn save_session(path: &Path, session: &SessionFile) -> Result<(), String> {
    validate_session(session)?;
    fs::create_dir_all(path.parent().expect("session path parent"))
        .map_err(|error| error.to_string())?;
    let contents = serde_json::to_string_pretty(session).map_err(|error| error.to_string())?;
    if contents.len() as u64 > MAX_SESSION_BYTES {
        return Err("LSA session exceeded the 1 MiB limit".to_owned());
    }
    let temporary = path.with_extension("jsonc.tmp");
    fs::write(&temporary, format!("{contents}\n")).map_err(|error| error.to_string())?;
    fs::rename(temporary, path).map_err(|error| error.to_string())
}

fn validate_session(session: &SessionFile) -> Result<(), String> {
    if session.turns.len() > MAX_SESSION_TURNS {
        return Err(format!(
            "LSA session exceeded the {MAX_SESSION_TURNS}-turn limit"
        ));
    }
    if session
        .turns
        .iter()
        .any(|turn| !matches!(turn.role.as_str(), "user" | "assistant"))
    {
        return Err("LSA session contains an unsupported turn role".to_owned());
    }
    Ok(())
}

pub(super) fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || session_id.len() > 128
        || matches!(session_id, "." | "..")
        || !session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("session id may contain only letters, numbers, '-', '_' and '.'".to_owned());
    }
    Ok(())
}

pub(super) fn new_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("session-{nanos}")
}

pub(super) fn detect_locale(request: &str) -> String {
    if request
        .chars()
        .any(|character| ('\u{3040}'..='\u{30ff}').contains(&character))
    {
        return "ja".to_owned();
    }
    if request
        .chars()
        .any(|character| ('\u{ac00}'..='\u{d7af}').contains(&character))
    {
        return "ko".to_owned();
    }
    if request
        .chars()
        .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
    {
        "zh-CN".to_owned()
    } else {
        "en".to_owned()
    }
}

pub(super) fn normalize_locale(locale: &str) -> Result<String, String> {
    LanguageTag::parse_and_normalize(locale)
        .map(|tag| tag.to_string())
        .map_err(|error| format!("invalid BCP 47 locale `{locale}`: {error}"))
}
