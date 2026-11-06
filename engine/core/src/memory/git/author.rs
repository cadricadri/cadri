use axum::http::StatusCode;

use crate::memory::types::Author;

use super::error::Fail;

pub(super) fn author_from(message: &str) -> Author {
    match required_author(message) {
        Ok(Parsed::Human { id, label }) => Author::Human { user_id: id, label },
        Ok(Parsed::Robot { id }) => Author::Robot { id },
        Ok(Parsed::Dream { id, label }) => Author::Human {
            user_id: id,
            label: format!("dream:{label}"),
        },
        Err(_) => Author::Human {
            user_id: "unknown".to_string(),
            label: "unknown".to_string(),
        },
    }
}

pub(super) enum Parsed {
    Human { id: String, label: String },
    Robot { id: String },
    Dream { id: String, label: String },
}

pub(super) fn required_author(message: &str) -> Result<Parsed, Fail> {
    let mut kind = None;
    let mut id = None;
    let mut label = None;
    for line in message.lines().rev() {
        if line.is_empty() {
            if kind.is_some() || id.is_some() || label.is_some() {
                break;
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("Cadri-Author-Kind:") {
            kind = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = line.strip_prefix("Cadri-Author-Id:") {
            id = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = line.strip_prefix("Cadri-Author-Label:") {
            label = Some(value.trim().to_string());
            continue;
        }
        if line.starts_with(SESSION_TRAILER) {
            continue;
        }
        break;
    }
    let id = id.filter(|value| !value.is_empty());
    let label = label.filter(|value| !value.is_empty());
    match (kind.as_deref(), id, label) {
        (Some("human"), Some(id), Some(label)) => Ok(Parsed::Human { id, label }),
        (Some("robot"), Some(id), Some(_)) => Ok(Parsed::Robot { id }),
        (Some("dream"), Some(id), Some(label)) => Ok(Parsed::Dream { id, label }),
        _ => Err(Fail::api(StatusCode::BAD_REQUEST, "missing author trailer")),
    }
}

pub(super) const SESSION_TRAILER: &str = "Cadri-Session-Id:";

pub(super) fn is_trailer(line: &str) -> bool {
    line.starts_with("Cadri-Author-") || line.starts_with(SESSION_TRAILER)
}

pub(super) fn display_message(message: &str) -> String {
    let mut lines: Vec<&str> = message.trim_end().lines().collect();
    while lines
        .last()
        .is_some_and(|line| line.is_empty() || is_trailer(line))
    {
        lines.pop();
    }
    lines.join("\n").trim().to_string()
}

/// The agent session that wrote a commit: the `Cadri-Session-Id` trailer, or the first
/// id on a `Sources:` line that older memory skills put in the message body.
const HARNESSES: &[&str] = &["claude_code", "cursor", "codex", "opencode"];

pub(super) fn session_from(message: &str) -> Option<String> {
    let trailer = message
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(SESSION_TRAILER));
    // `Sources: <id>, …` or `Sources: <harness>:<id>, …`; the first source wins.
    let legacy = || {
        message.lines().find_map(|line| {
            let first = line
                .trim()
                .strip_prefix("Sources:")?
                .split(|ch: char| ch == ',' || ch.is_whitespace())
                .find(|token| !token.is_empty())?;
            Some(
                HARNESSES
                    .iter()
                    .find_map(|harness| first.strip_prefix(harness)?.strip_prefix(':'))
                    .unwrap_or(first),
            )
        })
    };
    trailer
        .map(str::trim)
        .or_else(legacy)
        .filter(|id| valid_session(id))
        .map(str::to_string)
}

/// `^[A-Za-z0-9._:-]{1,128}$`, wide enough for every harness's session ids.
pub(crate) fn valid_session(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRAILERS: &str =
        "Cadri-Author-Kind: robot\nCadri-Author-Id: cursor-1a2b\nCadri-Author-Label: Ada";

    #[test]
    fn session_trailer_is_read_and_hidden() {
        let message = format!("Add runbook\n\n{TRAILERS}\nCadri-Session-Id: 53ac8e3c-80ae\n");
        assert_eq!(session_from(&message).as_deref(), Some("53ac8e3c-80ae"));
        assert_eq!(display_message(&message), "Add runbook");
        assert!(matches!(
            required_author(&message),
            Ok(Parsed::Robot { .. })
        ));
    }

    #[test]
    fn legacy_sources_line_names_the_session() {
        let message = format!("Seed conventions\n\nSources: abc-123, def-456\n\n{TRAILERS}\n");
        assert_eq!(session_from(&message).as_deref(), Some("abc-123"));
        let prefixed = "Add preferences\n\nSources: claude_code:2f1c-9a, cursor:7a9e\n";
        assert_eq!(session_from(prefixed).as_deref(), Some("2f1c-9a"));
        assert_eq!(session_from(&format!("Plain\n\n{TRAILERS}")), None);
        assert_eq!(session_from("x\n\nCadri-Session-Id: not valid!"), None);
    }
}
