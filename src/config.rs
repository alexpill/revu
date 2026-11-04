//! User configuration, read from `$XDG_CONFIG_HOME/revu/config.toml` (or
//! `--config`), then overridden by command-line arguments.
//!
//! ```toml
//! [[repos]]
//! name = "owner/repo"
//! path = "~/code/repo"   # optional local clone: enables local diff/file reads and LSP
//!
//! theme = "Catppuccin Mocha"   # optional, any bat theme
//!
//! [lsp]                   # file extension -> server command (overrides the defaults)
//! rs = ["rust-analyzer"]
//! ```

use crate::cli::Args;
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub repos: Vec<RepoConfig>,
    #[serde(default)]
    pub lsp: HashMap<String, Vec<String>>,
    /// bat theme name for code, e.g. "Nord" or "Catppuccin Mocha".
    pub theme: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoConfig {
    pub name: String,
    pub path: Option<PathBuf>,
}

impl RepoConfig {
    pub fn owner_name(&self) -> (&str, &str) {
        self.name.split_once('/').unwrap_or(("", &self.name))
    }
}

pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    base.join("revu").join("config.toml")
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cache"));
    base.join("revu")
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn expand_tilde(p: &Path) -> PathBuf {
    match p.strip_prefix("~") {
        Ok(rest) => home().join(rest),
        Err(_) => p.to_path_buf(),
    }
}

pub fn load(args: &Args) -> Result<Config> {
    let path = args.config.as_deref().map(expand_tilde).unwrap_or_else(config_path);
    let mut cfg: Config = match std::fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).with_context(|| format!("invalid config {}", path.display()))?,
        // The default file is optional; one passed with --config is not.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && args.config.is_none() => Config::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    for r in &mut cfg.repos {
        validate_name(&r.name)?;
        r.path = r.path.as_deref().map(expand_tilde);
    }
    if let Some(theme) = &args.theme {
        cfg.theme = Some(theme.clone());
    }
    if !args.repos.is_empty() {
        let mut repos: Vec<RepoConfig> = Vec::new();
        for spec in &args.repos {
            let mut r = resolve(spec)?;
            // `--repo owner/name` keeps the local path the config knows for it.
            if r.path.is_none() {
                r.path = cfg
                    .repos
                    .iter()
                    .find(|c| c.name.eq_ignore_ascii_case(&r.name))
                    .and_then(|c| c.path.clone());
            }
            if !repos.iter().any(|x| x.name.eq_ignore_ascii_case(&r.name)) {
                repos.push(r);
            }
        }
        cfg.repos = repos;
    } else if cfg.repos.is_empty()
        && let Ok(r) = local_repo(Path::new("."))
    {
        // Nothing configured: use the repo we're standing in.
        cfg.repos.push(r);
    }
    Ok(cfg)
}

/// A repo given on the command line: local directory, GitHub URL or owner/name.
fn resolve(spec: &str) -> Result<RepoConfig> {
    let dir = expand_tilde(Path::new(spec));
    if dir.is_dir() {
        return local_repo(&dir);
    }
    if let Some(name) = parse_github_remote(spec) {
        return Ok(RepoConfig { name, path: None });
    }
    validate_name(spec).map_err(|_| anyhow!("{spec:?} is not a directory, a GitHub URL or owner/name"))?;
    Ok(RepoConfig {
        name: spec.to_string(),
        path: None,
    })
}

/// Names end up inlined in GraphQL queries, so only allow GitHub's charset.
fn validate_name(name: &str) -> Result<()> {
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    match name.split_once('/') {
        Some((o, n)) if ok(o) && ok(n) => Ok(()),
        _ => bail!("invalid repo name {name:?}, expected owner/name"),
    }
}

/// The GitHub repo of a local clone, with the clone's root as path.
fn local_repo(dir: &Path) -> Result<RepoConfig> {
    let git = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let top =
        git(&["rev-parse", "--show-toplevel"]).ok_or_else(|| anyhow!("{} is not a git repository", dir.display()))?;
    let remotes = git(&["remote", "-v"]).unwrap_or_default();
    let name = pick_remote(&remotes).ok_or_else(|| anyhow!("no GitHub remote in {top}"))?;
    Ok(RepoConfig {
        name,
        path: Some(PathBuf::from(top)),
    })
}

/// From `git remote -v` output: `upstream` first (PRs of a fork's parent
/// live there), then `origin`, then any GitHub remote.
fn pick_remote(remote_v: &str) -> Option<String> {
    let remotes: Vec<(&str, String)> = remote_v
        .lines()
        .filter_map(|l| {
            let (name, rest) = l.split_once('\t')?;
            Some((name, parse_github_remote(rest.split(' ').next()?)?))
        })
        .collect();
    ["upstream", "origin"]
        .iter()
        .find_map(|want| remotes.iter().find(|(n, _)| n == want))
        .or(remotes.first())
        .map(|(_, repo)| repo.clone())
}

pub fn parse_github_remote(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))
        .or_else(|| url.strip_prefix("github.com/"))?;
    let name = rest.trim_end_matches('/').trim_end_matches(".git").to_string();
    validate_name(&name).ok()?;
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes() {
        assert_eq!(parse_github_remote("git@github.com:a/b.git").as_deref(), Some("a/b"));
        assert_eq!(
            parse_github_remote("https://github.com/a/b-c.d").as_deref(),
            Some("a/b-c.d")
        );
        assert_eq!(parse_github_remote("https://gitlab.com/a/b"), None);
        assert_eq!(parse_github_remote("github.com/a/b/").as_deref(), Some("a/b"));
        assert!(validate_name("a/b\"){").is_err());
    }

    #[test]
    fn remote_preference() {
        let v = "origin\tgit@github.com:me/fork.git (fetch)\norigin\tgit@github.com:me/fork.git (push)\n\
                 upstream\thttps://github.com/org/proj (fetch)\nlab\thttps://gitlab.com/x/y (fetch)";
        assert_eq!(pick_remote(v).as_deref(), Some("org/proj"));
        assert_eq!(
            pick_remote("mine\tgit@github.com:a/b.git (fetch)").as_deref(),
            Some("a/b")
        );
        assert_eq!(pick_remote("lab\thttps://gitlab.com/x/y (fetch)"), None);
    }

    #[test]
    fn specs() {
        assert_eq!(resolve("git@github.com:a/b.git").unwrap().name, "a/b");
        assert_eq!(resolve("https://github.com/a/b").unwrap().name, "a/b");
        assert_eq!(resolve("a/b-not-a-dir").unwrap().name, "a/b-not-a-dir");
        assert!(resolve("not a repo").is_err());
    }
}
