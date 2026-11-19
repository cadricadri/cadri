//! A Cursor canvas is a TSX file the agent writes under `canvases/`. The session
//! page cannot run that component, so the upload keeps a markdown reading of it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

const BODY_LIMIT: usize = 24_000;

/// The canvas as the session page shows it. `slug` is the file name, so a later
/// edit of the same canvas replaces the earlier card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canvas {
    pub slug: String,
    pub title: String,
    pub body: String,
    pub source: String,
    #[serde(default)]
    pub after_message: usize,
}

pub(super) fn from_call(name: &str, input: Option<&Value>) -> Vec<Canvas> {
    let key = tool_key(name);
    let owned = object_input(input);
    let Some(data) = owned.as_ref() else {
        return Vec::new();
    };
    if key == "write" || key == "createfile" {
        let path = string_field(data, &["path", "file_path", "filePath", "filepath"]);
        let body = string_field(data, &["contents", "content", "file_text"]);
        return path
            .zip(body)
            .and_then(|(path, body)| from_source(path, body, name))
            .into_iter()
            .collect();
    }
    Vec::new()
}

pub(super) fn from_source(path: &str, source: &str, tool: &str) -> Option<Canvas> {
    if !is_canvas_path(path) {
        return None;
    }
    let (title, body) = project(source)?;
    let slug = slug_of(path);
    let title = if title.is_empty() {
        slug_title(&slug)
    } else {
        title
    };
    if body.trim().is_empty() {
        return None;
    }
    Some(Canvas {
        slug,
        title,
        body: clip(&body),
        source: tool.to_string(),
        after_message: 0,
    })
}

pub(super) fn is_canvas_path(path: &str) -> bool {
    let path = path.replace('\\', "/").to_ascii_lowercase();
    path.contains("/canvases/") && path.ends_with(".canvas.tsx")
}

fn slug_of(path: &str) -> String {
    let name = path.replace('\\', "/");
    let name = name.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".canvas.tsx")
        .or_else(|| name.strip_suffix(".CANVAS.TSX"))
        .unwrap_or(name)
        .to_string()
}

fn slug_title(slug: &str) -> String {
    let title = slug.replace(['_', '-'], " ");
    let title = title.trim();
    if title.is_empty() {
        "Canvas".to_string()
    } else {
        title.to_string()
    }
}

fn clip(body: &str) -> String {
    let count = body.chars().count();
    if count <= BODY_LIMIT {
        return body.to_string();
    }
    let clipped: String = body.chars().take(BODY_LIMIT).collect();
    format!("{}\n\n…(canvas truncated)", clipped.trim_end())
}

/// Headings, prose, callouts, stats, tables, and charts. The component shell is dropped.
fn project(source: &str) -> Option<(String, String)> {
    let mut title = String::new();
    let mut blocks = Vec::new();
    let mut index = 0;
    while let Some(tag) = next_tag(source, index) {
        if is_container(&tag.name) {
            index = if tag.self_closing {
                tag.end
            } else {
                tag.open_end
            };
            continue;
        }
        index = tag.end;
        match tag.name.as_str() {
            "H1" => {
                let text = plain(&tag.inner);
                if text.is_empty() {
                    continue;
                }
                if title.is_empty() {
                    title = text;
                } else {
                    blocks.push(format!("# {text}"));
                }
            }
            "H2" => push_heading(&mut blocks, "##", &tag.inner),
            "H3" => push_heading(&mut blocks, "###", &tag.inner),
            "Text" => {
                let text = plain(&tag.inner);
                if !text.is_empty() {
                    blocks.push(text);
                }
            }
            "Callout" => {
                let text = plain(&tag.inner);
                if text.is_empty() && attr(&tag.attrs, "title").is_none() {
                    continue;
                }
                let mut lines = Vec::new();
                if let Some(heading) = attr(&tag.attrs, "title") {
                    lines.push(format!("> **{heading}**"));
                    lines.push(">".to_string());
                }
                for line in text.split('\n') {
                    let line = line.trim();
                    if line.is_empty() {
                        lines.push(">".to_string());
                    } else {
                        lines.push(format!("> {line}"));
                    }
                }
                blocks.push(lines.join("\n"));
            }
            "Stat" => {
                let value = attr(&tag.attrs, "value").unwrap_or_default();
                let label = attr(&tag.attrs, "label").unwrap_or_default();
                if value.is_empty() && label.is_empty() {
                    continue;
                }
                blocks.push(format!("**{value}** — {label}"));
            }
            "Table" => {
                if let Some(table) = markdown_table(&tag.attrs) {
                    blocks.push(table);
                }
            }
            "BarChart" => {
                if let Some(table) = markdown_chart(&tag.attrs) {
                    blocks.push(table);
                }
            }
            _ => {}
        }
    }
    if title.is_empty() && blocks.is_empty() {
        return None;
    }
    let mut body = String::new();
    if !title.is_empty() {
        body.push_str("# ");
        body.push_str(&title);
        body.push_str("\n\n");
    }
    body.push_str(&blocks.join("\n\n"));
    let body = body.trim().to_string();
    if body.is_empty() {
        return None;
    }
    Some((title, body))
}

fn push_heading(blocks: &mut Vec<String>, marks: &str, inner: &str) {
    let text = plain(inner);
    if !text.is_empty() {
        blocks.push(format!("{marks} {text}"));
    }
}

fn is_container(name: &str) -> bool {
    matches!(name, "Stack" | "Grid" | "div" | "Fragment")
}

struct Tag {
    name: String,
    attrs: String,
    inner: String,
    open_end: usize,
    end: usize,
    self_closing: bool,
}

fn next_tag(source: &str, from: usize) -> Option<Tag> {
    let mut index = from;
    while index < source.len() {
        let rest = &source[index..];
        if !rest.starts_with('<') {
            index += rest.chars().next()?.len_utf8();
            continue;
        }
        let after = &source[index + 1..];
        let name = jsx_name(after);
        if name.is_empty() || after.starts_with('/') || after.starts_with('!') {
            index += 1;
            continue;
        }
        let attr_start = index + 1 + name.len();
        let (attrs, open_end, self_closing) = scan_open(&source[attr_start..])?;
        let open_end = attr_start + open_end;
        if self_closing {
            return Some(Tag {
                name,
                attrs,
                inner: String::new(),
                open_end,
                end: open_end,
                self_closing: true,
            });
        }
        let (inner, end) = matching_close(source, open_end, &name)?;
        return Some(Tag {
            name,
            attrs,
            inner,
            open_end,
            end,
            self_closing: false,
        });
    }
    None
}

fn jsx_name(source: &str) -> String {
    source
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect()
}

/// Attributes and the index just past the opening tag. Respects quoted strings.
fn scan_open(source: &str) -> Option<(String, usize, bool)> {
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in source.char_indices() {
        if let Some(mark) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == mark {
                quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '/' if source[index..].starts_with("/>") => {
                return Some((source[..index].to_string(), index + 2, true));
            }
            '>' => return Some((source[..index].to_string(), index + 1, false)),
            _ => {}
        }
    }
    None
}

fn matching_close(source: &str, from: usize, name: &str) -> Option<(String, usize)> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let mut depth = 1;
    let mut index = from;
    while index < source.len() {
        let rest = &source[index..];
        if rest.starts_with(&close) {
            depth -= 1;
            if depth == 0 {
                return Some((source[from..index].to_string(), index + close.len()));
            }
            index += close.len();
            continue;
        }
        if rest.starts_with(&open) && tag_boundary(source, index + open.len()) {
            depth += 1;
            index += open.len();
            continue;
        }
        index += rest.chars().next()?.len_utf8();
    }
    None
}

fn tag_boundary(source: &str, index: usize) -> bool {
    source[index..]
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_whitespace() || ch == '>' || ch == '/')
}

fn plain(inner: &str) -> String {
    let mut stripped = String::new();
    let mut index = 0;
    let chars: Vec<char> = inner.chars().collect();
    while index < chars.len() {
        if chars[index] == '{' {
            if let Some(end) = balanced(&chars, index, '{', '}') {
                index = end;
                continue;
            }
        }
        if chars[index] == '<' {
            let rest: String = chars[index + 1..].iter().collect();
            if rest.starts_with('/')
                || jsx_name(&rest)
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphabetic())
            {
                if let Some(end) = balanced_angle(&chars, index) {
                    index = end;
                    continue;
                }
            }
        }
        stripped.push(chars[index]);
        index += 1;
    }
    whitespace(&stripped)
}

fn balanced(chars: &[char], start: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0;
    let mut quote = None;
    let mut escaped = false;
    for (offset, ch) in chars[start..].iter().enumerate() {
        if let Some(mark) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if *ch == '\\' {
                escaped = true;
                continue;
            }
            if *ch == mark {
                quote = None;
            }
            continue;
        }
        match *ch {
            '"' | '\'' | '`' => quote = Some(*ch),
            ch if ch == open => depth += 1,
            ch if ch == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(start + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn balanced_angle(chars: &[char], start: usize) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (offset, ch) in chars[start..].iter().enumerate() {
        if let Some(mark) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if *ch == '\\' {
                escaped = true;
                continue;
            }
            if *ch == mark {
                quote = None;
            }
            continue;
        }
        match *ch {
            '"' | '\'' => quote = Some(*ch),
            '>' => return Some(start + offset + 1),
            _ => {}
        }
    }
    None
}

fn whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn attr(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let start = attrs.find(&needle)?;
    let rest = attrs[start + needle.len()..].trim_start();
    if let Some(stripped) = rest.strip_prefix('"').or_else(|| rest.strip_prefix('\'')) {
        let mark = rest.chars().next()?;
        let end = stripped.find(mark)?;
        return Some(stripped[..end].to_string());
    }
    if let Some(expr) = rest.strip_prefix('{') {
        let chars: Vec<char> = expr.chars().collect();
        let end = balanced_from_open(&chars)?;
        let inner: String = chars[..end.saturating_sub(1)].iter().collect();
        return js_string(inner.trim()).or_else(|| Some(whitespace(inner.trim())));
    }
    None
}

fn balanced_from_open(chars: &[char]) -> Option<usize> {
    let mut depth = 1;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in chars.iter().enumerate() {
        if let Some(mark) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if *ch == '\\' {
                escaped = true;
                continue;
            }
            if *ch == mark {
                quote = None;
            }
            continue;
        }
        match *ch {
            '"' | '\'' | '`' => quote = Some(*ch),
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn markdown_table(attrs: &str) -> Option<String> {
    let headers = js_strings(&attr_expr(attrs, "headers")?)?;
    let rows = js_rows(&attr_expr(attrs, "rows")?)?;
    if headers.is_empty() {
        return None;
    }
    Some(render_table(&headers, &rows))
}

fn markdown_chart(attrs: &str) -> Option<String> {
    let categories = js_strings(&attr_expr(attrs, "categories")?)?;
    let series = js_series(&attr_expr(attrs, "series")?)?;
    if categories.is_empty() || series.is_empty() {
        return None;
    }
    let mut headers = vec![String::new()];
    headers.extend(categories);
    let rows = series
        .into_iter()
        .map(|(name, data)| {
            let mut row = vec![name];
            row.extend(data);
            row
        })
        .collect::<Vec<_>>();
    Some(render_table(&headers, &rows))
}

fn render_table(headers: &[String], rows: &[Vec<String>]) -> String {
    let width = headers.len();
    let line = |cells: &[String]| {
        let mut padded = cells
            .iter()
            .map(|cell| cell.replace('|', "\\|"))
            .collect::<Vec<_>>();
        padded.resize(width, String::new());
        format!("| {} |", padded.join(" | "))
    };
    let mut out = vec![
        line(headers),
        format!("| {} |", vec!["---"; width].join(" | ")),
    ];
    for row in rows {
        out.push(line(row));
    }
    out.join("\n")
}

fn attr_expr(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{name}={{");
    let start = attrs.find(&needle)?;
    let expr = &attrs[start + needle.len() - 1..];
    let chars: Vec<char> = expr.chars().collect();
    let end = balanced(&chars, 0, '{', '}')?;
    Some(chars[1..end - 1].iter().collect())
}

#[derive(Debug, Clone)]
enum Js {
    Str(String),
    Num(String),
    Arr(Vec<Js>),
    Obj(Vec<(String, Js)>),
    Skip,
}

struct Parser<'a> {
    chars: &'a [char],
    index: usize,
}

fn js_strings(expr: &str) -> Option<Vec<String>> {
    let value = parse_js(expr)?;
    let Js::Arr(items) = value else {
        return None;
    };
    Some(items.into_iter().filter_map(js_text).collect())
}

fn js_rows(expr: &str) -> Option<Vec<Vec<String>>> {
    let value = parse_js(expr)?;
    let Js::Arr(items) = value else {
        return None;
    };
    Some(
        items
            .into_iter()
            .filter_map(|item| match item {
                Js::Arr(cells) => Some(cells.into_iter().filter_map(js_text).collect()),
                _ => None,
            })
            .collect(),
    )
}

fn js_series(expr: &str) -> Option<Vec<(String, Vec<String>)>> {
    let value = parse_js(expr)?;
    let Js::Arr(items) = value else {
        return None;
    };
    let mut series = Vec::new();
    for item in items {
        let Js::Obj(fields) = item else {
            continue;
        };
        let name = fields
            .iter()
            .find(|(key, _)| key == "name")
            .and_then(|(_, value)| js_text(value.clone()))
            .unwrap_or_default();
        let data = fields
            .into_iter()
            .find(|(key, _)| key == "data")
            .and_then(|(_, value)| match value {
                Js::Arr(items) => Some(items.into_iter().filter_map(js_text).collect()),
                _ => None,
            })
            .unwrap_or_default();
        series.push((name, data));
    }
    Some(series)
}

fn js_text(value: Js) -> Option<String> {
    match value {
        Js::Str(text) => Some(text),
        Js::Num(text) => Some(text),
        _ => None,
    }
}

fn js_string(expr: &str) -> Option<String> {
    match parse_js(expr)? {
        Js::Str(text) => Some(text),
        _ => None,
    }
}

fn parse_js(expr: &str) -> Option<Js> {
    let chars: Vec<char> = expr.chars().collect();
    let mut parser = Parser {
        chars: &chars,
        index: 0,
    };
    parser.parse_value()
}

impl<'a> Parser<'a> {
    fn parse_value(&mut self) -> Option<Js> {
        self.skip();
        let ch = self.peek()?;
        match ch {
            '"' | '\'' => Some(Js::Str(self.parse_string()?)),
            '[' => Some(Js::Arr(self.parse_array()?)),
            '{' => Some(Js::Obj(self.parse_object()?)),
            ch if ch == '-' || ch.is_ascii_digit() => Some(Js::Num(self.parse_number())),
            _ => {
                self.skip_ident();
                Some(Js::Skip)
            }
        }
    }

    fn parse_array(&mut self) -> Option<Vec<Js>> {
        self.bump();
        let mut items = Vec::new();
        loop {
            self.skip();
            if self.peek()? == ']' {
                self.bump();
                return Some(items);
            }
            items.push(self.parse_value()?);
            self.skip();
            if self.peek() == Some(',') {
                self.bump();
            }
        }
    }

    fn parse_object(&mut self) -> Option<Vec<(String, Js)>> {
        self.bump();
        let mut fields = Vec::new();
        loop {
            self.skip();
            if self.peek()? == '}' {
                self.bump();
                return Some(fields);
            }
            let key = if matches!(self.peek(), Some('"' | '\'')) {
                self.parse_string()?
            } else {
                self.parse_ident()
            };
            self.skip();
            if self.peek() == Some(':') {
                self.bump();
            }
            fields.push((key, self.parse_value()?));
            self.skip();
            if self.peek() == Some(',') {
                self.bump();
            }
        }
    }

    fn parse_string(&mut self) -> Option<String> {
        let mark = self.bump()?;
        let mut out = String::new();
        while let Some(ch) = self.bump() {
            if ch == '\\' {
                if let Some(escaped) = self.bump() {
                    out.push(match escaped {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                }
                continue;
            }
            if ch == mark {
                return Some(out);
            }
            out.push(ch);
        }
        None
    }

    fn parse_number(&mut self) -> String {
        let start = self.index;
        if self.peek() == Some('-') {
            self.bump();
        }
        while self
            .peek()
            .is_some_and(|ch| ch.is_ascii_digit() || ch == '.')
        {
            self.bump();
        }
        self.chars[start..self.index].iter().collect()
    }

    fn parse_ident(&mut self) -> String {
        let start = self.index;
        while self
            .peek()
            .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            self.bump();
        }
        self.chars[start..self.index].iter().collect()
    }

    fn skip_ident(&mut self) {
        let _ = self.parse_ident();
    }

    fn skip(&mut self) {
        loop {
            while self.peek().is_some_and(|ch| ch.is_whitespace()) {
                self.bump();
            }
            if self.chars.get(self.index..self.index + 2) == Some(&['/', '/']) {
                self.index += 2;
                while self.peek().is_some_and(|ch| ch != '\n') {
                    self.bump();
                }
                continue;
            }
            break;
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.index += 1;
        Some(ch)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        <Stack>
          <H1>Hosted Postgres</H1>
          <Text>Keep the page in the database.</Text>
          <Callout tone="info" title="Decision">RDS in the VPC.</Callout>
          <Stat value="2.1 MB" label="Stored view" />
          <H2>Team size</H2>
          <Table
            headers={["Engineers", "Stored"]}
            rows={[
              ["25", "322 MB"],
              ["1000", "13 GB"],
            ]}
          />
          <BarChart
            categories={["5", "25"]}
            series={[{ name: "Stored (MB)", data: [64, 322] }]}
          />
        </Stack>
    "#;

    #[test]
    fn a_canvas_becomes_markdown() {
        let (title, body) = project(SAMPLE).unwrap();
        assert_eq!(title, "Hosted Postgres");
        assert!(body.starts_with("# Hosted Postgres"));
        assert!(body.contains("Keep the page in the database."));
        assert!(body.contains("> **Decision**"));
        assert!(body.contains("> RDS in the VPC."));
        assert!(body.contains("**2.1 MB** — Stored view"));
        assert!(body.contains("## Team size"));
        assert!(body.contains("| Engineers | Stored |"));
        assert!(body.contains("| 25 | 322 MB |"));
        assert!(body.contains("| 1000 | 13 GB |"));
        assert!(body.contains("| Stored (MB) | 64 | 322 |"));
    }

    #[test]
    fn an_ordinary_file_is_not_a_canvas() {
        assert!(from_source("src/App.tsx", SAMPLE, "Write").is_none());
        let canvas = from_source(
            "/Users/me/.cursor/projects/app/canvases/hosted-postgres.canvas.tsx",
            SAMPLE,
            "Write",
        )
        .unwrap();
        assert_eq!(canvas.slug, "hosted-postgres");
        assert_eq!(canvas.title, "Hosted Postgres");
        assert_eq!(canvas.source, "Write");
    }
}
