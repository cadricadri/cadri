//! Header of a git bundle. Nothing in production calls it until the log format lands.
#![allow(dead_code)]

use super::error::Fail;
use super::ids::is_hex;

pub(super) struct BundleHeader {
    pub prerequisites: Vec<String>,
    /// `(sha, refname)`
    pub refs: Vec<(String, String)>,
}

pub(super) fn parse_bundle_header(bytes: &[u8]) -> Result<BundleHeader, Fail> {
    let bad = |why: &str| Fail::git(format!("bad git bundle header: {why}"));
    let mut lines = bytes.split_inclusive(|byte| *byte == b'\n');
    let first = lines.next().ok_or_else(|| bad("empty"))?;
    let version3 = match first.strip_suffix(b"\n").unwrap_or(first) {
        b"# v2 git bundle" => false,
        b"# v3 git bundle" => true,
        _ => return Err(bad("signature")),
    };
    let mut header = BundleHeader {
        prerequisites: Vec::new(),
        refs: Vec::new(),
    };
    for line in lines {
        if line == b"\n" {
            return Ok(header);
        }
        let line = line
            .strip_suffix(b"\n")
            .ok_or_else(|| bad("unterminated line"))?;
        let line = std::str::from_utf8(line).map_err(|_| bad("not utf-8"))?;
        if let Some(rest) = line.strip_prefix('@') {
            if !version3 || rest.is_empty() {
                return Err(bad("capability"));
            }
        } else if let Some(rest) = line.strip_prefix('-') {
            let sha = rest.split(' ').next().unwrap_or("");
            if !is_sha(sha) {
                return Err(bad("prerequisite"));
            }
            header.prerequisites.push(sha.to_string());
        } else {
            let (sha, name) = line.split_once(' ').ok_or_else(|| bad("ref line"))?;
            if !is_sha(sha) || name.is_empty() {
                return Err(bad("ref line"));
            }
            header.refs.push((sha.to_string(), name.to_string()));
        }
    }
    Err(bad("no blank line ends the header"))
}

fn is_sha(text: &str) -> bool {
    is_hex(text, 40)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn full_and_incremental_bundles_report_prerequisites_and_ref() {
        let dir = std::env::temp_dir().join(format!("cadri-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("a.md"), "one").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "one"]);
        let old = git(&dir, &["rev-parse", "HEAD"]);
        let full = dir.join("full.bundle");
        git(
            &dir,
            &[
                "bundle",
                "create",
                full.to_str().unwrap(),
                "refs/heads/main",
            ],
        );
        std::fs::write(dir.join("a.md"), "two").unwrap();
        git(&dir, &["commit", "-q", "-am", "two"]);
        let new = git(&dir, &["rev-parse", "HEAD"]);
        let delta = dir.join("delta.bundle");
        git(
            &dir,
            &[
                "bundle",
                "create",
                delta.to_str().unwrap(),
                "refs/heads/main",
                &format!("^{old}"),
            ],
        );

        let header = parse_bundle_header(&std::fs::read(&full).unwrap()).unwrap();
        assert!(header.prerequisites.is_empty());
        assert_eq!(header.refs, [(old.clone(), "refs/heads/main".to_string())]);

        let header = parse_bundle_header(&std::fs::read(&delta).unwrap()).unwrap();
        assert_eq!(header.prerequisites, [old]);
        assert_eq!(header.refs, [(new, "refs/heads/main".to_string())]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_v3_header_skips_capability_lines() {
        let a = "a".repeat(40);
        let b = "b".repeat(40);
        let text = format!(
            "# v3 git bundle\n@object-format=sha1\n-{a} first commit\n{b} refs/heads/main\n\nPACK"
        );
        let header = parse_bundle_header(text.as_bytes()).unwrap();
        assert_eq!(header.prerequisites, [a]);
        assert_eq!(header.refs, [(b, "refs/heads/main".to_string())]);
    }

    #[test]
    fn malformed_headers_are_rejected() {
        let a = "a".repeat(40);
        assert!(parse_bundle_header(b"").is_err());
        assert!(parse_bundle_header(b"# v1 git bundle\n\n").is_err());
        assert!(
            parse_bundle_header(format!("# v2 git bundle\n@x=y\n{a} r\n\n").as_bytes()).is_err()
        );
        assert!(parse_bundle_header(b"# v2 git bundle\nnothex refs/heads/main\n\n").is_err());
        assert!(
            parse_bundle_header(format!("# v2 git bundle\n{a} refs/heads/main\n").as_bytes())
                .is_err()
        );
    }
}
