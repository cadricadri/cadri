use std::collections::{HashMap, HashSet};

use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use crate::ApiError;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Author {
    Human { user_id: String, label: String },
    Robot { session_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlameEntry {
    pub author: Author,
    pub commit_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryFile {
    pub path: String,
    pub content: String,
    pub blame: Vec<BlameEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCommit {
    pub id: String,
    pub at: String,
    pub author: Author,
    pub message: String,
    pub files: Vec<String>,
}

/// Stand-in memory store. Each organization gets a copy of the sample in process memory.
/// A later git-backed store replaces this.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryStore {
    pub files: Vec<MemoryFile>,
    pub commits: Vec<MemoryCommit>,
}

pub type MemoryBooks = HashMap<String, MemoryStore>;

impl MemoryStore {
    pub fn seed() -> Self {
        let ada = Author::Human {
            user_id: "user_ada".to_string(),
            label: "ada@example.com".to_string(),
        };
        let sam = Author::Human {
            user_id: "user_sam".to_string(),
            label: "sam@example.com".to_string(),
        };
        let oncall_bot = Author::Robot {
            session_id: "sess_7k2m9qpl".to_string(),
        };
        let deploy_bot = Author::Robot {
            session_id: "sess_4hn8cwra".to_string(),
        };
        MemoryStore {
            files: vec![
                file(
                    "README.md",
                    &[block(
                        sam.clone(),
                        "c_index04",
                        &[
                            "# Shared memory",
                            "",
                            "Cadri keeps the notes agents and people write for the rest of the team.",
                            "",
                            "Open a file to read it, or edit it when a note is wrong.",
                        ],
                    )],
                ),
                file(
                    "conventions/editors.md",
                    &[
                        block(
                            ada.clone(),
                            "c_conv02",
                            &[
                                "# Editors",
                                "",
                                "Agents should match the surrounding file.",
                                "",
                                "- Run the formatter the repo already uses",
                                "- Leave unrelated lines alone",
                                "",
                            ],
                        ),
                        block(
                            sam.clone(),
                            "c_index04",
                            &[
                                "Humans editing in Cadri should write a short commit message.",
                                "The message shows up in Memories Committed.",
                            ],
                        ),
                    ],
                ),
                file(
                    "conventions/rust.md",
                    &[block(
                        ada.clone(),
                        "c_conv02",
                        &[
                            "# Rust",
                            "",
                            "Engine code lives under `engine/`. The core never imports `ee/`.",
                            "",
                            "```rust",
                            "fn health() -> &'static str {",
                            "    \"ok\"",
                            "}",
                            "```",
                            "",
                            "Prefer a focused module over a new crate.",
                        ],
                    )],
                ),
                file(
                    "conventions/webui.md",
                    &[block(
                        sam.clone(),
                        "c_index04",
                        &[
                            "# Web UI",
                            "",
                            "The dashboard is a Vite app in `webui/`. Shared color tokens live in `style.css`.",
                            "",
                            "- Inter for reading",
                            "- JetBrains Mono for code and transcripts",
                            "- Silkscreen for the nav",
                            "",
                            "Use `.btn` and `.btn.ghost` for actions.",
                        ],
                    )],
                ),
                file(
                    "product/memory-units.md",
                    &[block(
                        oncall_bot.clone(),
                        "c_oncall01",
                        &[
                            "# Memory units",
                            "",
                            "A memory is a small note another session can reuse.",
                            "",
                            "Each unit should say:",
                            "",
                            "- What was learned",
                            "- Where it applies",
                            "- Which session it came from",
                        ],
                    )],
                ),
                file(
                    "product/sessions.md",
                    &[block(
                        deploy_bot.clone(),
                        "c_deploy03",
                        &[
                            "# Sessions",
                            "",
                            "Robot edits link back to the session that wrote them.",
                            "",
                            "Open the author mark to see that session in Transcripts.",
                            "",
                            "Human edits stay attached to the person's WorkOS identity.",
                        ],
                    )],
                ),
                file(
                    "runbooks/deploys.md",
                    &[block(
                        deploy_bot,
                        "c_deploy03",
                        &[
                            "# Deploys",
                            "",
                            "Ship the engine before the web UI. The UI proxies `/api` to the engine.",
                            "",
                            "## Order",
                            "",
                            "1. Engine",
                            "2. Web UI",
                            "3. Confirm `/healthz`",
                            "",
                            "Roll back the UI first if a release goes wrong.",
                        ],
                    )],
                ),
                file(
                    "runbooks/oncall.md",
                    &[
                        block(
                            oncall_bot,
                            "c_oncall01",
                            &[
                                "# On-call",
                                "",
                                "A 2am latency spike came from upstream load tests, not from the API.",
                                "",
                                "## Check first",
                                "",
                                "1. Confirm the error budget window",
                                "2. Look at upstream before restarting the service",
                                "",
                            ],
                        ),
                        block(
                            ada.clone(),
                            "c_conv02",
                            &["Ada added: page a human only after the finding is written here."],
                        ),
                    ],
                ),
            ],
            commits: vec![
                commit(
                    "c_oncall01",
                    "2026-10-12T15:04:00.000Z",
                    oncall_author(),
                    "Record the on-call latency finding",
                    &["runbooks/oncall.md", "product/memory-units.md"],
                ),
                commit(
                    "c_conv02",
                    "2026-10-18T09:30:00.000Z",
                    ada,
                    "Write the Rust and editor conventions",
                    &[
                        "conventions/rust.md",
                        "conventions/editors.md",
                        "runbooks/oncall.md",
                    ],
                ),
                commit(
                    "c_deploy03",
                    "2026-10-22T18:12:00.000Z",
                    deploy_author(),
                    "Capture the deploy order from the release session",
                    &["runbooks/deploys.md", "product/sessions.md"],
                ),
                commit(
                    "c_index04",
                    "2026-10-26T11:45:00.000Z",
                    sam,
                    "Add the web UI notes and the index",
                    &[
                        "README.md",
                        "conventions/webui.md",
                        "conventions/editors.md",
                    ],
                ),
            ],
        }
    }

    pub fn check(&self) -> Result<(), ApiError> {
        if self.files.is_empty() {
            return Err(ApiError::new(StatusCode::BAD_REQUEST, "files are required"));
        }
        let mut seen = HashSet::new();
        for file in &self.files {
            if !valid_path(&file.path) || !seen.insert(file.path.as_str()) {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "invalid memory path",
                ));
            }
            if file.blame.len() != file.content.split('\n').count() {
                return Err(ApiError::new(
                    StatusCode::BAD_REQUEST,
                    "blame does not match the file",
                ));
            }
        }
        Ok(())
    }
}

fn oncall_author() -> Author {
    Author::Robot {
        session_id: "sess_7k2m9qpl".to_string(),
    }
}

fn deploy_author() -> Author {
    Author::Robot {
        session_id: "sess_4hn8cwra".to_string(),
    }
}

struct Block {
    author: Author,
    commit_id: &'static str,
    rows: &'static [&'static str],
}

fn block(author: Author, commit_id: &'static str, rows: &'static [&'static str]) -> Block {
    Block {
        author,
        commit_id,
        rows,
    }
}

fn file(path: &str, blocks: &[Block]) -> MemoryFile {
    let mut lines = Vec::new();
    let mut blame = Vec::new();
    for block in blocks {
        for row in block.rows {
            lines.push((*row).to_string());
            blame.push(BlameEntry {
                author: block.author.clone(),
                commit_id: block.commit_id.to_string(),
            });
        }
    }
    MemoryFile {
        path: path.to_string(),
        content: lines.join("\n"),
        blame,
    }
}

fn commit(id: &str, at: &str, author: Author, message: &str, files: &[&str]) -> MemoryCommit {
    MemoryCommit {
        id: id.to_string(),
        at: at.to_string(),
        author,
        message: message.to_string(),
        files: files.iter().map(|path| (*path).to_string()).collect(),
    }
}

fn valid_path(path: &str) -> bool {
    if path.is_empty() || path.starts_with('/') || path.contains('\\') {
        return false;
    }
    path.split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..")
}
