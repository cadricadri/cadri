//! Jail to confine git processes.
//!
//! The engine starts git processes with `--memory-confine REPO --memory-scratch SCRATCH -- command...`
//! to confine them to the repo and scratch directories.
//! 
//! The reason to run the engine's git processes in a sandboxed
//! environment is to prevent the git processes from accessing the host's
//! filesystem, in case of an exploit.
use std::path::PathBuf;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) struct Job {
    pub repo: PathBuf,
    pub scratch: PathBuf,
    pub command: Vec<String>,
}

pub(crate) fn available() -> bool {
    #[cfg(target_os = "linux")]
    {
        linux::probe()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// `--memory-confine REPO --memory-scratch SCRATCH -- command...`
pub(crate) fn job_from_args() -> Option<Job> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("--memory-confine") {
        return None;
    }
    let repo = PathBuf::from(args.get(1)?);
    if args.get(2).map(String::as_str) != Some("--memory-scratch") {
        return None;
    }
    let scratch = PathBuf::from(args.get(3)?);
    let dash = args.iter().position(|arg| arg == "--")?;
    let command = args[dash + 1..].to_vec();
    if command.is_empty() {
        return None;
    }
    Some(Job {
        repo,
        scratch,
        command,
    })
}

pub(crate) fn confine_and_exec(job: Job) -> ! {
    #[cfg(target_os = "linux")]
    {
        if let Err(err) = linux::confine(&job.repo, &job.scratch) {
            eprintln!("landlock confine failed: {err}");
            std::process::exit(1);
        }
        exec(&job.command);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = job;
        eprintln!("landlock is unavailable; refusing to run an unconfined git child");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn exec(command: &[String]) -> ! {
    use std::os::unix::process::CommandExt;
    use std::process::Command;
    let error = Command::new(&command[0]).args(&command[1..]).exec();
    eprintln!("failed to exec {}: {error}", command[0]);
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
mod linux {
    use std::path::Path;

    use landlock::{
        Access, AccessFs, CompatLevel, Compatible, PathBeneath, PathFd, Ruleset, RulesetAttr,
        RulesetCreatedAttr, ABI,
    };

    pub fn probe() -> bool {
        let abi = ABI::V1;
        Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(AccessFs::from_read(abi))
            .and_then(|rules| rules.create())
            .is_ok()
    }

    pub fn confine(repo: &Path, scratch: &Path) -> Result<(), String> {
        let abi = ABI::V1;
        let mut read_exec = AccessFs::from_read(abi);
        read_exec |= AccessFs::Execute;
        let all = AccessFs::from_all(abi);
        let mut rules = Vec::new();
        for path in ["/usr", "/bin", "/lib", "/lib64", "/etc", "/proc"] {
            if let Ok(fd) = PathFd::new(path) {
                rules.push(PathBeneath::new(fd, read_exec));
            }
        }
        if let Ok(fd) = PathFd::new("/dev") {
            rules.push(PathBeneath::new(fd, all));
        }
        rules.push(PathBeneath::new(
            PathFd::new(repo).map_err(|err| err.to_string())?,
            all,
        ));
        rules.push(PathBeneath::new(
            PathFd::new(scratch).map_err(|err| err.to_string())?,
            all,
        ));
        Ruleset::default()
            .set_compatibility(CompatLevel::HardRequirement)
            .handle_access(all)
            .map_err(|err| err.to_string())?
            .create()
            .map_err(|err| err.to_string())?
            .add_rules(rules.into_iter().map(Ok::<_, landlock::RulesetError>))
            .map_err(|err| err.to_string())?
            .restrict_self()
            .map_err(|err| err.to_string())?;
        Ok(())
    }
}
