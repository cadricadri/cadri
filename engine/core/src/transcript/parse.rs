use std::collections::BTreeMap;

use chrono::{DateTime, FixedOffset, NaiveDateTime, SecondsFormat, TimeZone, Utc};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Assistant,
}

/// A turn the person at the keyboard saw: their prompt or the agent's reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: Role,
    /// For the assistant, the text after the turn's last tool call: the reply itself.
    pub text: String,
    /// Assistant text written before a tool call in the same turn (progress narration).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

impl Message {
    fn demote_text(&mut self) {
        let text = std::mem::take(&mut self.text);
        if !text.is_empty() {
            self.notes.push(text);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolCall {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

/// A design the person and the agent wrote down before building. Tool calls stay
/// hidden in the chat; this is the document those calls were carrying.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    pub body: String,
    /// Tool that produced it (`CreatePlan`, `Write`, `ExitPlanMode`, `apply_patch`).
    pub source: String,
    /// Index of the chat message this plan follows. The card sits in the thread there.
    pub after_message: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub messages: Vec<Message>,
    pub tool_calls: Vec<ToolCall>,
    pub plans: Vec<Plan>,
    pub model: Option<String>,
    /// Earliest and latest timestamps seen on any line, including hidden tool output.
    span: Option<(String, String)>,
    /// A tool call came after the latest assistant text.
    tool_since_text: bool,
}

/// Lowercased tool names that change files on disk.
const WRITE_TOOLS: &[&str] = &[
    "write",
    "edit",
    "multiedit",
    "notebookedit",
    "editnotebook",
    "strreplace",
    "str_replace",
    "str_replace_editor",
    "apply_patch",
    "patch",
    "create_file",
    "delete",
    "delete_file",
];

impl Parsed {
    pub fn prompt(&self) -> Option<&str> {
        self.messages
            .iter()
            .find(|message| message.role == Role::User)
            .map(|message| message.text.as_str())
    }

    pub fn started_at(&self) -> Option<String> {
        self.span.as_ref().map(|(start, _)| start.clone())
    }

    pub fn ended_at(&self) -> Option<String> {
        self.span.as_ref().map(|(_, end)| end.clone())
    }

    /// A turn that ended on a tool call has no reply; its last text was narration too.
    fn settle_turn(&mut self) {
        if std::mem::take(&mut self.tool_since_text) {
            if let Some(last) = self
                .messages
                .last_mut()
                .filter(|last| last.role == Role::Assistant)
            {
                last.demote_text();
            }
        }
    }

    fn saw(&mut self, at: Option<&str>) {
        let Some(at) = at else {
            return;
        };
        match &mut self.span {
            None => self.span = Some((at.to_string(), at.to_string())),
            Some((start, end)) => {
                if at < start.as_str() {
                    *start = at.to_string();
                }
                if at > end.as_str() {
                    *end = at.to_string();
                }
            }
        }
    }

    pub fn turns(&self, role: Role) -> usize {
        self.messages
            .iter()
            .filter(|message| message.role == role)
            .count()
    }

    pub fn tool_counts(&self) -> BTreeMap<String, u64> {
        let mut counts = BTreeMap::new();
        for call in &self.tool_calls {
            *counts.entry(call.name.clone()).or_insert(0) += 1;
        }
        counts
    }

    pub fn has_writes(&self) -> bool {
        self.tool_calls
            .iter()
            .any(|call| WRITE_TOOLS.contains(&call.name.to_ascii_lowercase().as_str()))
    }
}

/// Parses a stored session blob. Each line is recognised by shape rather than by the
/// harness label, so a mislabelled upload still parses. `conversation` (the optional
/// `[{role, text}]` layer a client may send) is used when the blob yields no turns.
pub fn parse(content: &str, conversation: Option<&Value>) -> Parsed {
    let mut parsed = Parsed::default();
    if !parse_opencode(content, &mut parsed) {
        let mut codex = Codex::default();
        for line in content.lines() {
            let line = line.trim();
            if !line.starts_with('{') {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            parse_line(&value, &mut parsed, &mut codex);
        }
        if !codex.saw_response_messages {
            for message in codex.events {
                push_message(&mut parsed.messages, message);
            }
        }
    }
    parsed.settle_turn();
    if parsed.messages.is_empty() {
        if let Some(Value::Array(turns)) = conversation {
            for turn in turns {
                let role = match turn.get("role").and_then(Value::as_str) {
                    Some("user") => Role::User,
                    Some("assistant") => Role::Assistant,
                    _ => continue,
                };
                if let Some(text) = turn.get("text").and_then(Value::as_str) {
                    push_message(&mut parsed.messages, message(role, text, None));
                }
            }
        }
    }
    parsed
}

#[derive(Default)]
struct Codex {
    saw_response_messages: bool,
    /// `event_msg` copies of the turns, used only for files without `response_item`s.
    events: Vec<Message>,
}

fn parse_line(value: &Value, parsed: &mut Parsed, codex: &mut Codex) {
    let kind = value.get("type").and_then(Value::as_str);
    if let Some(payload) = value.get("payload").filter(|payload| payload.is_object()) {
        let at = value.get("timestamp").and_then(normalize_time);
        parsed.saw(at.as_deref());
        parse_codex(kind, payload, at, parsed, codex);
        return;
    }
    match kind {
        Some("user") | Some("assistant") if value.get("message").is_some() => {
            parse_claude(value, parsed)
        }
        Some("message") if value.get("role").is_some() => {
            parse_codex(Some("response_item"), value, None, parsed, codex)
        }
        _ if value.get("role").is_some() && value.get("message").is_some() => {
            parse_cursor(value, parsed)
        }
        _ => {}
    }
}

fn parse_claude(value: &Value, parsed: &mut Parsed) {
    let flag = |name: &str| value.get(name).and_then(Value::as_bool).unwrap_or(false);
    if flag("isSidechain") || flag("isMeta") || flag("isCompactSummary") {
        return;
    }
    let at = value.get("timestamp").and_then(normalize_time);
    parsed.saw(at.as_deref());
    let message = &value["message"];
    let role = match value.get("type").and_then(Value::as_str) {
        Some("user") => Role::User,
        _ => Role::Assistant,
    };
    if role == Role::Assistant {
        if let Some(model) = message.get("model").and_then(Value::as_str) {
            if !model.starts_with('<') {
                parsed.model = Some(model.to_string());
            }
        }
    }
    match &message["content"] {
        Value::String(text) => push_text(parsed, role, text, at),
        Value::Array(blocks) => {
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            push_text(parsed, role, text, at.clone());
                        }
                    }
                    Some("tool_use") | Some("server_tool_use") => {
                        note_tool(parsed, block.get("name"), block.get("input"), at.clone())
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn parse_cursor(value: &Value, parsed: &mut Parsed) {
    let role = match value.get("role").and_then(Value::as_str) {
        Some("user") => Role::User,
        Some("assistant") => Role::Assistant,
        _ => return,
    };
    let Value::Array(blocks) = &value["message"]["content"] else {
        if let Value::String(text) = &value["message"]["content"] {
            let at = if role == Role::User {
                cursor_timestamp(text)
            } else {
                None
            };
            push_text(parsed, role, text, at);
        }
        return;
    };
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    let at = if role == Role::User {
                        cursor_timestamp(text)
                    } else {
                        None
                    };
                    push_text(parsed, role, text, at);
                }
            }
            Some("tool_use") => note_tool(parsed, block.get("name"), block.get("input"), None),
            _ => {}
        }
    }
}

fn parse_codex(
    kind: Option<&str>,
    payload: &Value,
    at: Option<String>,
    parsed: &mut Parsed,
    codex: &mut Codex,
) {
    let payload_kind = payload.get("type").and_then(Value::as_str);
    match (kind, payload_kind) {
        (Some("turn_context"), _) => {
            if let Some(model) = payload.get("model").and_then(Value::as_str) {
                parsed.model = Some(model.to_string());
            }
        }
        (Some("response_item"), Some("message")) => {
            let role = match payload.get("role").and_then(Value::as_str) {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => return,
            };
            let text = match &payload["content"] {
                Value::String(text) => text.clone(),
                Value::Array(parts) => parts
                    .iter()
                    .filter_map(|part| part.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => return,
            };
            codex.saw_response_messages = true;
            push_text(parsed, role, &text, at);
        }
        (Some("response_item"), Some("function_call" | "custom_tool_call")) => note_tool(
            parsed,
            payload.get("name"),
            payload.get("arguments").or_else(|| payload.get("input")),
            at,
        ),
        (Some("response_item"), Some("local_shell_call")) => {
            push_tool(parsed, Some(&Value::from("shell")), at)
        }
        (Some("response_item"), Some("web_search_call")) => {
            push_tool(parsed, Some(&Value::from("web_search")), at)
        }
        (Some("event_msg"), Some(event @ ("user_message" | "agent_message"))) => {
            let role = if event == "user_message" {
                Role::User
            } else {
                Role::Assistant
            };
            if let Some(text) = payload.get("message").and_then(Value::as_str) {
                let text = clean_text(role, text);
                if !text.is_empty() {
                    codex.events.push(message(role, &text, at));
                }
            }
        }
        _ => {}
    }
}

/// OpenCode uploads one JSON document, `{"info": {...}, "messages": [...]}`. Messages come
/// in two shapes: `session_message` rows (2.x) carry no role, so a user turn is `{text}`
/// and an assistant turn is `{model, content: [parts]}`; older `message` rows and
/// `opencode export` carry `{info: {role, modelID}, parts}`.
fn parse_opencode(content: &str, parsed: &mut Parsed) -> bool {
    let trimmed = content.trim_start();
    if !trimmed.starts_with('{')
        || trimmed
            .lines()
            .nth(1)
            .is_some_and(|line| line.trim_start().starts_with('{'))
    {
        return false;
    }
    let Ok(document) = serde_json::from_str::<Value>(trimmed) else {
        return false;
    };
    let Some(messages) = document.get("messages").and_then(Value::as_array) else {
        return false;
    };
    let info = document.get("info").unwrap_or(&Value::Null);
    parsed.model = info.get("model").and_then(opencode_model);
    for entry in messages {
        let meta = entry.get("info").unwrap_or(entry);
        let role = match meta.get("role").and_then(Value::as_str) {
            Some("user") => Role::User,
            Some("assistant") => Role::Assistant,
            Some(_) => continue,
            None if entry.get("content").is_some_and(Value::is_array) => Role::Assistant,
            None if entry.get("text").is_some_and(Value::is_string) => Role::User,
            None => continue,
        };
        let time = meta.get("time");
        let at = time
            .and_then(|time| time.get("created"))
            .and_then(normalize_time);
        parsed.saw(at.as_deref());
        parsed.saw(
            time.and_then(|time| time.get("completed"))
                .and_then(normalize_time)
                .as_deref(),
        );
        if let Some(model) = meta
            .get("modelID")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| meta.get("model").and_then(opencode_model))
        {
            parsed.model = Some(model);
        }
        if let Some(text) = entry
            .get("content")
            .or_else(|| entry.get("text"))
            .and_then(Value::as_str)
        {
            push_text(parsed, role, text, at.clone());
        }
        for part in entry
            .get("parts")
            .or_else(|| entry.get("content"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match part.get("type").and_then(Value::as_str) {
                Some("text")
                    if !part
                        .get("synthetic")
                        .and_then(Value::as_bool)
                        .unwrap_or(false) =>
                {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        push_text(parsed, role, text, at.clone());
                    }
                }
                Some("tool") => {
                    let tool_at = part
                        .get("time")
                        .and_then(|time| time.get("start").or_else(|| time.get("created")))
                        .and_then(normalize_time)
                        .or_else(|| at.clone());
                    note_tool(
                        parsed,
                        part.get("tool").or_else(|| part.get("name")),
                        part.get("state")
                            .and_then(|state| state.get("input"))
                            .or_else(|| part.get("input")),
                        tool_at,
                    );
                }
                _ => {}
            }
        }
    }
    true
}

/// `{"id": "..."}`, or that object serialized into a string (as `session_v2.model` is).
fn opencode_model(value: &Value) -> Option<String> {
    let parsed;
    let object = match value {
        Value::String(text) => {
            parsed = serde_json::from_str::<Value>(text).ok()?;
            &parsed
        }
        other => other,
    };
    object
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

fn message(role: Role, text: &str, at: Option<String>) -> Message {
    Message {
        role,
        text: text.to_string(),
        notes: Vec::new(),
        at,
    }
}

fn push_text(parsed: &mut Parsed, role: Role, raw: &str, at: Option<String>) {
    parsed.saw(at.as_deref());
    let text = clean_text(role, raw);
    if text.is_empty() {
        return;
    }
    if role == Role::User {
        parsed.settle_turn();
    }
    let after_tool = std::mem::take(&mut parsed.tool_since_text);
    push_after(&mut parsed.messages, message(role, &text, at), after_tool);
}

fn push_message(messages: &mut Vec<Message>, next: Message) {
    push_after(messages, next, false);
}

/// Consecutive assistant text blocks read as one turn. When a tool call came between
/// them, the earlier text was narration on the way to the reply, so it becomes a note.
fn push_after(messages: &mut Vec<Message>, next: Message, after_tool: bool) {
    if next.role == Role::Assistant {
        if let Some(last) = messages
            .last_mut()
            .filter(|last| last.role == Role::Assistant)
        {
            if after_tool {
                last.demote_text();
                last.text = next.text;
            } else {
                if !last.text.is_empty() {
                    last.text.push_str("\n\n");
                }
                last.text.push_str(&next.text);
            }
            if last.at.is_none() {
                last.at = next.at;
            }
            return;
        }
    }
    messages.push(next);
}

fn push_tool(parsed: &mut Parsed, name: Option<&Value>, at: Option<String>) {
    parsed.saw(at.as_deref());
    parsed.tool_since_text = true;
    if let Some(name) = name.and_then(Value::as_str).filter(|name| !name.is_empty()) {
        parsed.tool_calls.push(ToolCall {
            name: name.to_string(),
            at,
        });
    }
}

fn note_tool(parsed: &mut Parsed, name: Option<&Value>, input: Option<&Value>, at: Option<String>) {
    push_tool(parsed, name, at);
    let Some(name) = name.and_then(Value::as_str).filter(|name| !name.is_empty()) else {
        return;
    };
    let after_message = plan_anchor(parsed);
    for mut plan in plans_from_call(name, input) {
        plan.after_message = after_message;
        upsert_plan(parsed, plan);
    }
}

/// A later draft of the same title replaces the earlier one.
fn upsert_plan(parsed: &mut Parsed, next: Plan) {
    if let Some(existing) = parsed
        .plans
        .iter_mut()
        .find(|plan| plan.title.eq_ignore_ascii_case(&next.title))
    {
        *existing = next;
        return;
    }
    parsed.plans.push(next);
}

/// Designs an agent writes down for the person to accept. The same shapes are read
/// in `plugin/cadri/plans.py` for the memory digest. A todo checklist (`update_plan`)
/// is not a plan.
fn plans_from_call(name: &str, input: Option<&Value>) -> Vec<Plan> {
    let key = tool_key(name);
    if key == "updateplan" || key == "enterplanmode" {
        return Vec::new();
    }
    let owned = object_input(input);
    let data = owned.as_ref();
    if key == "applypatch" {
        let patch = data
            .and_then(|value| string_field(value, &["command", "input", "patch"]))
            .or_else(|| input.and_then(Value::as_str));
        return patch
            .map(plans_added_in_patch)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(path, body)| plan_document(&path, None, None, &body, name))
            .collect();
    }
    let Some(data) = data else {
        return Vec::new();
    };
    if key == "write" || key == "createfile" {
        let path = string_field(data, &["path", "file_path", "filePath", "filepath"]);
        let body = string_field(data, &["contents", "content", "file_text"]);
        return path
            .zip(body)
            .and_then(|(path, body)| plan_document(path, None, None, body, name))
            .into_iter()
            .collect();
    }
    if is_plan_tool(&key) {
        if let Some(body) = string_field(data, &["plan", "plan_markdown", "markdown"]) {
            let title = string_field(data, &["name", "title"]);
            let overview = string_field(data, &["overview"]);
            return plan_document("", title, overview, body, name)
                .into_iter()
                .collect();
        }
    }
    Vec::new()
}

fn is_plan_tool(key: &str) -> bool {
    matches!(
        key,
        "createplan" | "exitplanmode" | "submitplan" | "presentplan" | "writeplan"
    ) || key.contains("plan")
}

fn tool_key(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn object_input(input: Option<&Value>) -> Option<Value> {
    match input? {
        Value::Object(_) => Some(input?.clone()),
        Value::String(text) => serde_json::from_str::<Value>(text)
            .ok()
            .filter(Value::is_object),
        _ => None,
    }
}

fn string_field<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
    })
}

fn plans_added_in_patch(patch: &str) -> Vec<(String, String)> {
    let mut path: Option<String> = None;
    let mut lines: Vec<String> = Vec::new();
    let mut found = Vec::new();
    let finish = |path: &str, lines: &[String], found: &mut Vec<(String, String)>| {
        if is_plan_path(path) {
            found.push((path.to_string(), lines.join("\n")));
        }
    };
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("*** Add File:") {
            if let Some(path) = path.take() {
                finish(&path, &lines, &mut found);
            }
            path = Some(rest.trim().to_string());
            lines.clear();
            continue;
        }
        if line.starts_with("*** ") {
            if let Some(path) = path.take() {
                finish(&path, &lines, &mut found);
            }
            lines.clear();
            continue;
        }
        if path.is_some() {
            if let Some(rest) = line.strip_prefix('+') {
                lines.push(rest.to_string());
            }
        }
    }
    if let Some(path) = path {
        finish(&path, &lines, &mut found);
    }
    found
}

fn plan_document(
    path: &str,
    title: Option<&str>,
    overview: Option<&str>,
    raw: &str,
    source: &str,
) -> Option<Plan> {
    if !path.is_empty() && !is_plan_path(path) {
        return None;
    }
    let (matter_name, matter_overview, body) = split_frontmatter(raw);
    let body = body.trim();
    if !is_plan_body(body) {
        return None;
    }
    let title = title
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .or(matter_name)
        .or_else(|| markdown_title(body))
        .or_else(|| path_title(path))
        .unwrap_or_else(|| "Plan".to_string());
    Some(Plan {
        title,
        overview: overview
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
            .or(matter_overview),
        body: body.to_string(),
        source: source.to_string(),
        after_message: 0,
    })
}

/// The plan follows the message that was already on screen when it was written.
fn plan_anchor(parsed: &Parsed) -> usize {
    parsed.messages.len().saturating_sub(1)
}

fn is_plan_path(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    if !path.ends_with(".md") {
        return false;
    }
    path.ends_with(".plan.md")
        || path.contains("/.cursor/plans/")
        || path.contains("/.claude/plans/")
        || path.contains("/.codex/plans/")
        || path.contains("/.opencode/plan/")
}

fn is_plan_body(body: &str) -> bool {
    body.chars().count() >= 80 && body.lines().any(|line| line.trim_start().starts_with("# "))
}

fn markdown_title(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        line.trim()
            .strip_prefix("# ")
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(str::to_string)
    })
}

fn path_title(path: &str) -> Option<String> {
    let name = path.replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or(path);
    let name = name
        .strip_suffix(".plan.md")
        .or_else(|| name.strip_suffix(".md"))?;
    let title = name.replace(['_', '-'], " ").trim().to_string();
    if title.is_empty() {
        None
    } else {
        Some(title)
    }
}

fn split_frontmatter(text: &str) -> (Option<String>, Option<String>, &str) {
    let text = text.trim();
    let Some(rest) = text.strip_prefix("---") else {
        return (None, None, text);
    };
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some((head, body)) = rest.split_once("\n---") else {
        return (None, None, text);
    };
    let mut name = None;
    let mut overview = None;
    for line in head.lines() {
        if let Some(value) = scalar(line, "name:") {
            name = Some(value);
        } else if let Some(value) = scalar(line, "overview:") {
            overview = Some(value);
        }
    }
    let body = body.trim_start_matches('\n');
    (name, overview, body)
}

fn scalar(line: &str, prefix: &str) -> Option<String> {
    let value = line.trim().strip_prefix(prefix)?.trim();
    let value = value.trim_matches('"').trim_matches('\'').trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// User turns carry harness-injected context in tag blocks (`<system_reminder>`,
/// `<environment_context>`, `<command-name>`…). Keep only what the person typed.
fn clean_text(role: Role, text: &str) -> String {
    if role == Role::Assistant {
        return text.trim().to_string();
    }
    let queries = tag_bodies(text, "user_query");
    let kept = if queries.is_empty() {
        strip_tag_blocks(text)
    } else {
        queries.join("\n\n")
    };
    let kept = kept.trim();
    if kept.starts_with("# AGENTS.md") || kept.starts_with("Caveat: The messages below") {
        return String::new();
    }
    kept.to_string()
}

fn tag_bodies<'a>(text: &'a str, tag: &str) -> Vec<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut bodies = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else {
            break;
        };
        bodies.push(after[..end].trim());
        rest = &after[end + close.len()..];
    }
    bodies
}

/// Removes `<tag ...>...</tag>` blocks that start a line. Inline markup is left alone.
fn strip_tag_blocks(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let Some(start) = next_line_tag(rest) else {
            out.push_str(rest);
            break;
        };
        let after = &rest[start + 1..];
        let name: String = after
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
            .collect();
        let close = format!("</{name}>");
        let opener_ok = !name.is_empty() && after[name.len()..].starts_with(['>', ' ', '\n']);
        match after.find(&close).filter(|_| opener_ok) {
            Some(end) => {
                out.push_str(&rest[..start]);
                rest = &after[end + close.len()..];
            }
            None => {
                out.push_str(&rest[..=start]);
                rest = after;
            }
        }
    }
    out
}

fn next_line_tag(text: &str) -> Option<usize> {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        if line[indent..].starts_with('<') {
            return Some(offset + indent);
        }
        offset += line.len();
    }
    None
}

fn utc(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn normalize_time(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|time| utc(time.with_timezone(&Utc))),
        Value::Number(number) => {
            let raw = number.as_f64()?;
            let millis = if raw > 100_000_000_000.0 {
                raw
            } else {
                raw * 1000.0
            };
            Utc.timestamp_millis_opt(millis as i64).single().map(utc)
        }
        _ => None,
    }
}

/// Cursor stamps user turns with `<timestamp>Thursday, Oct 29, 2026, 9:47 PM (UTC-7)</timestamp>`.
fn cursor_timestamp(text: &str) -> Option<String> {
    let body = *tag_bodies(text, "timestamp").first()?;
    let (local, zone) = body.split_once(" (UTC")?;
    let local = NaiveDateTime::parse_from_str(local.trim(), "%A, %b %d, %Y, %I:%M %p").ok()?;
    let zone = zone.trim_end_matches(')');
    let (hours, minutes) = zone.split_once(':').unwrap_or((zone, "0"));
    let hours: i32 = if hours.is_empty() {
        0
    } else {
        hours.parse().ok()?
    };
    let minutes: i32 = minutes.parse().ok()?;
    let seconds = hours * 3600 + hours.signum() * minutes * 60;
    let offset = FixedOffset::east_opt(seconds)?;
    let time = offset.from_local_datetime(&local).single()?;
    Some(utc(time.with_timezone(&Utc)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(value: serde_json::Value) -> String {
        value.to_string()
    }

    const CLAUDE: &str = include_str!("fixtures/claude_code.jsonl");
    const CURSOR: &str = include_str!("fixtures/cursor.jsonl");
    const CODEX: &str = include_str!("fixtures/codex.jsonl");
    const OPENCODE: &str = include_str!("fixtures/opencode.json");
    const OPENCODE_V2: &str = include_str!("fixtures/opencode_v2.json");

    fn roles(parsed: &Parsed) -> Vec<Role> {
        parsed.messages.iter().map(|message| message.role).collect()
    }

    #[test]
    fn claude_code_keeps_typed_turns_and_hides_tools() {
        let parsed = parse(CLAUDE, None);
        assert_eq!(parsed.prompt(), Some("install the vercel cli"));
        assert_eq!(roles(&parsed), vec![Role::User, Role::Assistant]);
        assert_eq!(parsed.messages[1].text, "The Vercel CLI is installed.");
        assert_eq!(parsed.messages[1].notes, vec!["Checking npm first."]);
        let names: Vec<&str> = parsed
            .tool_calls
            .iter()
            .map(|call| call.name.as_str())
            .collect();
        assert_eq!(names, vec!["Bash", "Write"]);
        assert!(parsed.has_writes());
        assert_eq!(parsed.model.as_deref(), Some("claude-opus-4"));
        assert_eq!(
            parsed.started_at().as_deref(),
            Some("2026-10-07T18:43:41.432Z")
        );
        assert_eq!(
            parsed.ended_at().as_deref(),
            Some("2026-10-07T18:47:11.135Z")
        );
    }

    #[test]
    fn cursor_unwraps_user_query_and_reads_the_timestamp() {
        let parsed = parse(CURSOR, None);
        assert_eq!(parsed.prompt(), Some("Design the memory store."));
        assert_eq!(
            roles(&parsed),
            vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
        assert_eq!(
            parsed.messages[0].at.as_deref(),
            Some("2026-10-30T04:47:00.000Z")
        );
        assert_eq!(
            parsed.messages[1].text,
            "Each memory repo stays small on purpose."
        );
        assert_eq!(
            parsed.messages[1].notes,
            vec!["I'll read the architecture notes first."]
        );
        assert_eq!(parsed.messages[2].text, "Ship it");
        assert!(parsed.messages[3].notes.is_empty());
        assert_eq!(parsed.tool_counts().get("Read"), Some(&2));
        assert!(parsed.has_writes());
        assert_eq!(
            parsed.ended_at().as_deref(),
            Some("2026-10-30T05:02:00.000Z")
        );
    }

    #[test]
    fn codex_skips_environment_context_and_counts_tools() {
        let parsed = parse(CODEX, None);
        assert_eq!(parsed.prompt(), Some("fix the failing test"));
        assert_eq!(roles(&parsed), vec![Role::User, Role::Assistant]);
        assert_eq!(parsed.model.as_deref(), Some("gpt-5-codex"));
        let names: Vec<&str> = parsed
            .tool_calls
            .iter()
            .map(|call| call.name.as_str())
            .collect();
        assert_eq!(names, vec!["shell", "apply_patch"]);
        assert!(parsed.has_writes());
    }

    #[test]
    fn opencode_reads_parts() {
        let parsed = parse(OPENCODE, None);
        assert_eq!(parsed.prompt(), Some("rename the button"));
        assert_eq!(roles(&parsed), vec![Role::User, Role::Assistant]);
        assert_eq!(parsed.tool_counts().get("edit"), Some(&1));
        assert_eq!(parsed.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(
            parsed.started_at().as_deref(),
            Some("2026-10-01T12:00:00.000Z")
        );
    }

    #[test]
    fn opencode_session_message_rows_infer_roles_from_shape() {
        let parsed = parse(OPENCODE_V2, None);
        assert_eq!(parsed.prompt(), Some("Fetch the setup doc and follow it."));
        assert_eq!(
            roles(&parsed),
            vec![Role::User, Role::Assistant, Role::User, Role::Assistant]
        );
        assert_eq!(
            parsed.messages[1].text,
            "I've fetched the document. Which option?"
        );
        let names: Vec<&str> = parsed
            .tool_calls
            .iter()
            .map(|call| call.name.as_str())
            .collect();
        assert_eq!(names, vec!["webfetch", "write"]);
        assert!(parsed.has_writes());
        assert_eq!(parsed.model.as_deref(), Some("flint-40b-free"));
        assert_eq!(
            parsed.started_at().as_deref(),
            Some("2026-10-22T22:05:39.984Z")
        );
        assert_eq!(
            parsed.ended_at().as_deref(),
            Some("2026-10-22T22:06:09.133Z")
        );
    }

    #[test]
    fn conversation_layer_is_the_fallback() {
        let conversation = serde_json::json!([
            {"role": "user", "text": "hi"},
            {"role": "system", "text": "ignored"},
            {"role": "assistant", "text": "hello"}
        ]);
        let parsed = parse("not json", Some(&conversation));
        assert_eq!(roles(&parsed), vec![Role::User, Role::Assistant]);
        assert!(!parsed.has_writes());
    }

    #[test]
    fn a_turn_that_ends_on_a_tool_call_has_only_notes() {
        let blob = concat!(
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>go</user_query>"}]}}"#,
            "\n",
            r#"{"role":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","name":"Read"}]}}"#,
            "\n",
            r#"{"role":"user","message":{"content":[{"type":"text","text":"<user_query>stop</user_query>"}]}}"#,
            "\n",
            r#"{"role":"assistant","message":{"content":[{"type":"text","text":"Stopping."},{"type":"tool_use","name":"Shell"}]}}"#,
        );
        let parsed = parse(blob, None);
        assert_eq!(parsed.messages.len(), 4);
        for index in [1, 3] {
            assert_eq!(parsed.messages[index].text, "");
            assert_eq!(parsed.messages[index].notes.len(), 1);
        }
    }

    #[test]
    fn a_cursor_plan_is_a_document_not_a_chat_message() {
        let draft = "# Transcripts page\n\n## Decisions\n\nFilters live in the URL, and memory commits point back at the session that wrote them.\n";
        let revised = "# Transcripts page\n\n## Decisions\n\nThe chart counts transcripts per day, and a second series counts memories.\n";
        let blob = [
            line(json!({
                "role": "user",
                "message": {"content": [{"type": "text", "text": "<user_query>Redesign the page.</user_query>"}]}
            })),
            line(json!({
                "role": "assistant",
                "message": {"content": [
                    {"type": "text", "text": "Here is the design."},
                    {"type": "tool_use", "name": "CreatePlan", "input": {
                        "name": "Transcripts page",
                        "overview": "A filterable list and a chat detail page.",
                        "plan": draft
                    }}
                ]}
            })),
            line(json!({
                "role": "assistant",
                "message": {"content": [{"type": "tool_use", "name": "CreatePlan", "input": {
                    "name": "Transcripts page",
                    "overview": "Revised.",
                    "plan": revised
                }}]}
            })),
        ]
        .join("\n");
        let parsed = parse(&blob, None);
        assert_eq!(parsed.plans.len(), 1, "a revision replaces the draft");
        assert_eq!(parsed.plans[0].title, "Transcripts page");
        assert_eq!(parsed.plans[0].overview.as_deref(), Some("Revised."));
        assert!(parsed.plans[0].body.contains("second series"));
        assert_eq!(parsed.plans[0].source, "CreatePlan");
        assert_eq!(parsed.plans[0].after_message, 1);
        assert!(!parsed
            .messages
            .iter()
            .any(|message| message.text.contains("second series")));
        assert!(parsed.tool_counts().contains_key("CreatePlan"));
    }

    #[test]
    fn claude_writes_its_plan_file_and_codex_adds_one() {
        let body = "---\nname: Checkout\noverview: Charge after the cart locks.\n---\n# Checkout\n\nThe cart locks before the charge, because a price change mid-payment was the bug.\n";
        let claude = line(json!({
            "type": "assistant",
            "message": {"content": [
                {"type": "tool_use", "name": "Write", "input": {
                    "file_path": "/Users/me/.claude/plans/checkout.md",
                    "content": body
                }},
                {"type": "tool_use", "name": "ExitPlanMode", "input": {"allowedPrompts": []}}
            ]}
        }));
        let parsed = parse(&claude, None);
        assert_eq!(parsed.plans.len(), 1);
        assert_eq!(parsed.plans[0].title, "Checkout");
        assert_eq!(
            parsed.plans[0].overview.as_deref(),
            Some("Charge after the cart locks.")
        );
        assert!(parsed.plans[0].body.contains("cart locks"));
        assert!(!parsed.plans[0].body.contains("name: Checkout"));
        assert_eq!(parsed.plans[0].source, "Write");

        let patch = "*** Begin Patch\n*** Add File: docs/checkout.plan.md\n+# Checkout\n+\n+Charge only after the cart locks, so a price change cannot land mid-payment.\n*** End Patch\n";
        let codex = [
            line(json!({
                "timestamp": "2026-09-12T10:00:00.000Z",
                "type": "response_item",
                "payload": {
                    "type": "function_call",
                    "name": "update_plan",
                    "arguments": "{\"plan\":[{\"step\":\"read the tests\",\"status\":\"pending\"}]}"
                }
            })),
            line(json!({
                "timestamp": "2026-09-12T10:00:01.000Z",
                "type": "response_item",
                "payload": {"type": "custom_tool_call", "name": "apply_patch", "input": patch}
            })),
        ]
        .join("\n");
        let parsed = parse(&codex, None);
        assert_eq!(
            parsed.plans.len(),
            1,
            "update_plan is a checklist, not a design"
        );
        assert_eq!(parsed.plans[0].title, "Checkout");
        assert!(parsed.plans[0].body.contains("price change"));
        assert_eq!(parsed.plans[0].source, "apply_patch");
    }

    #[test]
    fn opencode_plan_tool_is_read_from_the_part() {
        let blob = line(json!({
            "info": {"id": "ses_1"},
            "messages": [{"info": {"role": "assistant"}, "parts": [{
                "type": "tool",
                "tool": "submit_plan",
                "state": {"input": {
                    "name": "Rename",
                    "plan": "# Rename\n\nThe button says Save, because the old label described the implementation.\n"
                }}
            }]}]
        }));
        let parsed = parse(&blob, None);
        assert_eq!(parsed.plans.len(), 1);
        assert_eq!(parsed.plans[0].title, "Rename");
        assert_eq!(parsed.plans[0].source, "submit_plan");
        assert!(parsed.tool_counts().contains_key("submit_plan"));
    }

    #[test]
    fn an_ordinary_write_is_not_a_plan() {
        let blob = line(json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "name": "Write", "input": {
                "file_path": "/work/src/app.py",
                "content": "# not a heading in a plan file\n\nprint('hello')\n"
            }}]}
        }));
        assert!(parse(&blob, None).plans.is_empty());
    }

    #[test]
    fn an_opencode_plan_file_is_the_singular_plan_directory() {
        let body = "# Rename\n\nThe button says Save, because the old label described the implementation and confused people.\n";
        let hit = line(json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "name": "Write", "input": {
                "file_path": "/Users/me/.opencode/plan/rename.md",
                "content": body
            }}]}
        }));
        let parsed = parse(&hit, None);
        assert_eq!(parsed.plans.len(), 1);
        assert_eq!(parsed.plans[0].title, "Rename");
        let miss = hit.replace("/.opencode/plan/", "/.opencode/plans/");
        assert!(parse(&miss, None).plans.is_empty());
    }

    #[test]
    fn strip_keeps_inline_markup() {
        assert_eq!(
            strip_tag_blocks("<system_reminder>x</system_reminder>\nuse <b>bold</b> here"),
            "\nuse <b>bold</b> here"
        );
        assert_eq!(strip_tag_blocks("<unclosed>\nhello"), "<unclosed>\nhello");
    }
}
