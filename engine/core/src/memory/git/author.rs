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

pub(super) fn display_message(message: &str) -> String {
    let mut lines: Vec<&str> = message.trim_end().lines().collect();
    while lines
        .last()
        .is_some_and(|line| line.is_empty() || line.starts_with("Cadri-Author-"))
    {
        lines.pop();
    }
    lines.join("\n").trim().to_string()
}

