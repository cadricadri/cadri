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
        if line.starts_with(SESSION_TRAILER) || line.starts_with(ORIGIN_TRAILER) {
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
pub(super) const ORIGIN_TRAILER: &str = "Cadri-Origin:";

/// `seed` when the commit carries `Cadri-Origin: seed`. Every other commit is steady use.
pub(super) fn commit_origin(message: &str) -> &'static str {
    let seed = message.lines().any(|line| {
        line.trim()
            .strip_prefix(ORIGIN_TRAILER)
            .is_some_and(|value| value.trim() == "seed")
    });
    if seed {
        "seed"
    } else {
        "steady"
    }
}

pub(super) fn is_trailer(line: &str) -> bool {
    line.starts_with("Cadri-Author-")
        || line.starts_with(SESSION_TRAILER)
        || line.starts_with(ORIGIN_TRAILER)
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

/// Sessions the Sessions page should list this commit under.
///
/// A `Sources:` line names the sessions the learning came from. Those win over the
/// session that wrote the commit, and every id on the line is included. A repo
/// briefing from seed or join has no `Sources:` and is not attached to the install
/// session. The quiet hook's "left uncommitted" placeholder is not a learning.
pub(crate) fn linked_sessions(
    message: &str,
    session_id: Option<&str>,
    author: &Author,
) -> Vec<String> {
    let cited = cited_sessions(message);
    if !cited.is_empty() {
        return cited;
    }
    if message.contains("Left uncommitted by session ") {
        return Vec::new();
    }
    if let Author::Robot { id } = author {
        if id.ends_with("cadri-memory-seed") || id.ends_with("cadri-memory-join") {
            return Vec::new();
        }
    }
    session_id
        .filter(|id| valid_session(id))
        .map(|id| vec![id.to_string()])
        .unwrap_or_default()
}

/// Every session id on `Sources:` lines, harness prefix removed, in order, without duplicates.
pub(crate) fn cited_sessions(message: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for line in message.lines() {
        let Some(rest) = line.trim().strip_prefix("Sources:") else {
            continue;
        };
        for token in rest.split(|ch: char| ch == ',' || ch.is_whitespace()) {
            if token.is_empty() {
                continue;
            }
            let id = HARNESSES
                .iter()
                .find_map(|harness| token.strip_prefix(harness)?.strip_prefix(':'))
                .unwrap_or(token);
            if valid_session(id) && !ids.iter().any(|seen| seen == id) {
                ids.push(id.to_string());
            }
        }
    }
    ids
}

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

    use crate::memory::types::Author;

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

    #[test]
    fn prose_sources_link_the_first_id_and_stay_in_the_message() {
        let message = format!(
            "Deploys happen on Wednesday, before the release cut.\n\n\
             The Thursday rule was leftover from an older runbook.\n\n\
             Sources: cursor:7a9e, claude_code:2f1c\n\
             - cursor:7a9e (2026-11-13): the deploy day was corrected to Wednesday\n\n\
             {TRAILERS}\n"
        );
        assert_eq!(session_from(&message).as_deref(), Some("7a9e"));
        assert_eq!(
            display_message(&message),
            "Deploys happen on Wednesday, before the release cut.\n\n\
             The Thursday rule was leftover from an older runbook.\n\n\
             Sources: cursor:7a9e, claude_code:2f1c\n\
             - cursor:7a9e (2026-11-13): the deploy day was corrected to Wednesday"
        );
        let linked = format!("{message}Cadri-Session-Id: parent-session\n");
        assert_eq!(session_from(&linked).as_deref(), Some("parent-session"));
        assert_eq!(
            cited_sessions(&message),
            vec!["7a9e".to_string(), "2f1c".to_string()]
        );
        assert_eq!(
            linked_sessions(
                &message,
                Some("parent-session"),
                &Author::Robot {
                    id: "cursor-cadri-memory-seed".into()
                }
            ),
            vec!["7a9e".to_string(), "2f1c".to_string()]
        );
    }

    #[test]
    fn a_repo_briefing_and_a_placeholder_are_not_tied_to_the_writer() {
        let robot = Author::Robot {
            id: "cursor-cadri-memory-seed".into(),
        };
        assert_eq!(
            linked_sessions("The page is one file.", Some("install"), &robot),
            Vec::<String>::new()
        );
        let keeper = Author::Robot {
            id: "cursor-main".into(),
        };
        assert_eq!(
            linked_sessions("Remember the palette.", Some("install"), &keeper),
            vec!["install".to_string()]
        );
        assert_eq!(
            linked_sessions(
                "Update 4 memory files\n\nLeft uncommitted by session install.",
                Some("install"),
                &keeper
            ),
            Vec::<String>::new()
        );
    }
}
