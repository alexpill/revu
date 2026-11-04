//! Where file contents come from: the local clone when configured (fast, no
//! size limits), GitHub's API otherwise. The user's checkout is never touched:
//! we only fetch objects and create detached worktrees in the cache dir.

use crate::{config, gh};
use anyhow::{Context, Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Source {
    /// owner/name
    pub repo: String,
    pub local: Option<PathBuf>,
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().context("failed to run git")?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

impl Source {
    fn owner_name(&self) -> (&str, &str) {
        self.repo.split_once('/').unwrap_or(("", &self.repo))
    }

    /// The remote pointing at this repo (forks often call it `upstream`).
    fn remote(&self, dir: &Path) -> Result<String> {
        let remotes = git(dir, &["remote", "-v"])?;
        remotes
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .find(|(_, url)| {
                let url = url.split(' ').next().unwrap_or_default();
                config::parse_github_remote(url).is_some_and(|n| n.eq_ignore_ascii_case(&self.repo))
            })
            .map(|(name, _)| name.to_string())
            .ok_or_else(|| anyhow!("no git remote for {} in {}", self.repo, dir.display()))
    }

    /// Fetches the PR head into the local clone so `read`/`list` stay local.
    pub fn fetch_pr(&self, number: u64) -> Result<()> {
        let Some(dir) = &self.local else { return Ok(()) };
        let remote = self.remote(dir)?;
        git(dir, &["fetch", "--quiet", "--no-tags", &remote, &format!("refs/pull/{number}/head")]).map(drop)
    }

    pub fn read(&self, oid: &str, path: &str) -> Result<String> {
        if let Some(dir) = &self.local {
            if let Ok(s) = git(dir, &["show", &format!("{oid}:{path}")]) {
                return Ok(s);
            }
        }
        let (owner, name) = self.owner_name();
        gh::file(owner, name, oid, path)
    }

    pub fn list(&self, oid: &str) -> Result<Vec<String>> {
        if let Some(dir) = &self.local {
            if let Ok(s) = git(dir, &["ls-tree", "-r", "--name-only", oid]) {
                return Ok(s.lines().map(String::from).collect());
            }
        }
        let (owner, name) = self.owner_name();
        gh::tree(owner, name, oid)
    }

    /// A detached checkout of `oid` for language servers, reused across runs.
    pub fn worktree(&self, number: u64, oid: &str) -> Result<PathBuf> {
        let dir = self.local.as_ref().ok_or_else(|| anyhow!("{} has no local path in the config", self.repo))?;
        let wt = config::cache_dir().join("worktrees").join(self.repo.replace('/', "__")).join(format!("pr-{number}"));
        self.fetch_pr(number)?;
        if wt.exists() {
            git(&wt, &["checkout", "--quiet", "--detach", oid])?;
        } else {
            git(dir, &["worktree", "prune"])?;
            git(dir, &["worktree", "add", "--quiet", "--detach", &wt.to_string_lossy(), oid])?;
        }
        Ok(wt)
    }
}
