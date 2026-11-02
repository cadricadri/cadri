//! Path and object-id helpers shared across the memory git modules.

pub(super) const MAX_FILE_BYTES: usize = 1024 * 1024;
pub(super) const ZERO: &str = "0000000000000000000000000000000000000000";

pub(super) fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

/// Object-store key for a book. Derived from the verified identity so SQLite does
/// not need a map. WorkOS `user_*` / `org_*` ids are stable for the lifetime of
/// that object. Core/localhost uses `default` (or the configured token identity).
///
/// - Team: `{org_id}_repo_0`
/// - Personal: `{org_id}_{user_id}_repo_0` (org is included so the same person in
///   two orgs keeps two personal books)
pub fn memory_repo_id(org_id: &str, scope: &str, user_id: &str) -> Option<String> {
    let org = sanitize_id(org_id)?;
    match scope {
        "team" => Some(format!("{org}_repo_0")),
        "user" => {
            let user = sanitize_id(user_id)?;
            Some(format!("{org}_{user}_repo_0"))
        }
        _ => None,
    }
}

fn sanitize_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 {
        return None;
    }
    if !value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return None;
    }
    Some(value.to_string())
}

/// Derived ids look like `org_01…_repo_0` or `org_01…_user_01…_repo_0`.
pub(super) fn valid_repo_id(repo_id: &str) -> bool {
    let Some(stem) = repo_id.strip_suffix("_repo_0") else {
        return false;
    };
    !stem.is_empty()
        && stem.len() <= 200
        && stem
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

pub(super) fn is_keep(path: &str) -> bool {
    path.rsplit_once('/')
        .is_some_and(|(_, name)| name == ".keep")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_repo_id_is_deterministic() {
        assert_eq!(
            memory_repo_id("org_01ABC", "team", "").as_deref(),
            Some("org_01ABC_repo_0")
        );
        assert_eq!(
            memory_repo_id("org_01ABC", "user", "user_01ADA").as_deref(),
            Some("org_01ABC_user_01ADA_repo_0")
        );
        assert_eq!(
            memory_repo_id("default", "team", "").as_deref(),
            Some("default_repo_0")
        );
        assert_ne!(
            memory_repo_id("org_a", "user", "ada"),
            memory_repo_id("org_b", "user", "ada")
        );
        assert!(memory_repo_id("org/a", "team", "").is_none());
        assert!(memory_repo_id("org_a", "other", "ada").is_none());
    }
}
