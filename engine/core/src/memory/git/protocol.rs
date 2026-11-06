use axum::http::StatusCode;

use crate::memory::types::memory_path_ok;

use super::author::{required_author, Parsed};
use super::commit::check_layout;
use super::error::Fail;
use super::git_cli_exec::{git_bytes, git_out, git_text, Repo};
use super::ids::{is_hex, MAX_FILE_BYTES, ZERO};
use super::snapshot::Snapshot;
use super::tree::{cat_blobs, list_tree};

pub(super) const UPLOAD_AD: &str = "application/x-git-upload-pack-advertisement";
pub(super) const RECEIVE_AD: &str = "application/x-git-receive-pack-advertisement";
pub(super) const UPLOAD_RESULT: &str = "application/x-git-upload-pack-result";
pub(super) const RECEIVE_RESULT: &str = "application/x-git-receive-pack-result";

pub(crate) enum GitHttp {
    Ready {
        content_type: &'static str,
        body: Vec<u8>,
    },
}

pub(super) fn ready(body: Vec<u8>) -> GitHttp {
    GitHttp::Ready {
        content_type: RECEIVE_RESULT,
        body,
    }
}

pub(super) fn inspect(
    repo: &Repo,
    snapshot: &Snapshot,
    old: Option<&str>,
    new_sha: &str,
    user_id: &str,
) -> Result<(), Fail> {
    let range = match old {
        Some(old) => format!("{old}..{new_sha}"),
        None => new_sha.to_string(),
    };
    let log = git_text(repo, &["log", "--format=%x02%H%x1e%B", &range], None, &[])?;
    let mut trees = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for record in log.split('\u{2}') {
        let Some((sha, message)) = record.split_once('\u{1e}') else {
            continue;
        };
        let sha = sha.trim();
        if !is_hex(sha, 40) {
            continue;
        }
        match required_author(message.trim())? {
            Parsed::Human { id, .. } if id != user_id => {
                return Err(Fail::api(
                    StatusCode::BAD_REQUEST,
                    "human author does not match the token",
                ));
            }
            // Robot commits use the caller's credentials. Kind=robot means an
            // agent wrote the note (e.g. followed a memory skill); the id is the
            // agent's own label and is not looked up in transcripts.
            Parsed::Human { .. } | Parsed::Robot { .. } => {}
            Parsed::Dream { .. } => {
                return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid author"));
            }
        }
        let mut entries = Vec::new();
        for (mode, blob, path) in list_tree(repo, sha)? {
            if (mode != "100644" && mode != "100755") || !memory_path_ok(&path) {
                return Err(Fail::api(StatusCode::BAD_REQUEST, "invalid memory path"));
            }
            if !snapshot.blobs.contains_key(&blob) && !unknown.contains(&blob) {
                unknown.push(blob.clone());
            }
            entries.push((blob, path));
        }
        trees.push(entries);
    }
    let bodies = cat_blobs(repo, &unknown)?;
    for entries in &trees {
        for (blob, _) in entries {
            if snapshot.blobs.contains_key(blob) {
                continue;
            }
            let bytes = bodies
                .get(blob)
                .ok_or_else(|| Fail::git(format!("blob {blob} missing from cat-file --batch")))?;
            if bytes.len() > MAX_FILE_BYTES {
                return Err(Fail::api(
                    StatusCode::BAD_REQUEST,
                    "memory file is too large",
                ));
            }
            if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
                return Err(Fail::api(
                    StatusCode::BAD_REQUEST,
                    "memory file must be utf-8 text",
                ));
            }
        }
        check_layout(entries.iter().map(|(_, path)| path.as_str()))?;
    }
    Ok(())
}

pub(super) fn index_pack(repo: &Repo, pack: &[u8]) -> Result<(), String> {
    let out = git_out(
        repo,
        &["index-pack", "--fix-thin", "--stdin"],
        Some(pack),
        &[],
    )
    .map_err(|err| match err {
        Fail::Api(err) => err.message,
        Fail::Conflict => "conflict".to_string(),
    })?;
    if out.ok {
        Ok(())
    } else {
        Err(out.stderr)
    }
}

/// `git upload-pack` arguments that never expose internal refs.
pub(super) fn upload_args<'a>(rest: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec!["-c", "uploadpack.hideRefs=refs/cadri", "upload-pack"];
    args.extend_from_slice(rest);
    args
}

pub(super) fn advertise_upload(repo: &Repo) -> Result<Vec<u8>, Fail> {
    let advertised = git_bytes(
        repo,
        &upload_args(&["--stateless-rpc", "--advertise-refs", "."]),
        None,
        &[],
    )?;
    let mut body = pkt("# service=git-upload-pack\n");
    body.extend_from_slice(b"0000");
    body.extend(advertised);
    Ok(body)
}

pub(super) fn advertise_receive(head: Option<&str>) -> Vec<u8> {
    let caps = "report-status ofs-delta agent=cadri/0.1\n";
    let line = match head {
        Some(sha) => format!("{sha} refs/heads/main\0{caps}"),
        None => format!("{ZERO} capabilities^{{}}\0{caps}"),
    };
    let mut body = pkt("# service=git-receive-pack\n");
    body.extend_from_slice(b"0000");
    body.extend(pkt(&line));
    body.extend_from_slice(b"0000");
    body
}

pub(super) struct CommandLine {
    pub(super) old: String,
    pub(super) new: String,
    pub(super) name: String,
}

pub(super) fn split_pack(body: &[u8]) -> Result<(Vec<CommandLine>, &[u8]), String> {
    let mut rest = body;
    let mut commands = Vec::new();
    loop {
        let (pkt_body, next) = read_pkt(rest)?;
        rest = next;
        let Some(pkt_body) = pkt_body else {
            break;
        };
        commands.push(parse_command(&pkt_body)?);
    }
    while rest.starts_with(b"0000") {
        rest = &rest[4..];
    }
    Ok((commands, rest))
}

pub(super) fn parse_command(bytes: &[u8]) -> Result<CommandLine, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "command is not utf-8".to_string())?;
    let text = text.trim_end_matches(['\n', '\r']);
    let text = text.split('\0').next().unwrap_or(text);
    let mut parts = text.split(' ');
    let old = parts.next().ok_or("short command")?.to_string();
    let new = parts.next().ok_or("short command")?.to_string();
    let name = parts.next().ok_or("short command")?.to_string();
    if old.len() != 40 || new.len() != 40 {
        return Err("bad object name".into());
    }
    Ok(CommandLine { old, new, name })
}

pub(super) fn read_pkt(input: &[u8]) -> Result<(Option<Vec<u8>>, &[u8]), String> {
    if input.len() < 4 {
        return Err("short pkt-line".into());
    }
    let len = usize::from_str_radix(
        std::str::from_utf8(&input[..4]).map_err(|_| "pkt-line".to_string())?,
        16,
    )
    .map_err(|_| "pkt-line".to_string())?;
    if len == 0 {
        return Ok((None, &input[4..]));
    }
    if len < 4 || input.len() < len {
        return Err("short pkt-line".into());
    }
    Ok((Some(input[4..len].to_vec()), &input[len..]))
}

pub(super) fn pkt(payload: &str) -> Vec<u8> {
    let len = payload.len() + 4;
    let mut out = format!("{len:04x}").into_bytes();
    out.extend_from_slice(payload.as_bytes());
    out
}

pub(super) fn report(update: Result<(), &str>) -> Vec<u8> {
    let mut body = pkt("unpack ok\n");
    match update {
        Ok(()) => body.extend(pkt("ok refs/heads/main\n")),
        Err(message) => {
            let message = message.replace(['\n', '\r'], " ");
            body.extend(pkt(&format!("ng refs/heads/main {message}\n")));
        }
    }
    body.extend_from_slice(b"0000");
    body
}

pub(super) fn unpack_fail(message: &str) -> Vec<u8> {
    let mut body = pkt(&format!("unpack {message}\n"));
    body.extend_from_slice(b"0000");
    body
}
