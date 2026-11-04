//! User configuration, read from `$XDG_CONFIG_HOME/prt/config.toml`.
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

use anyhow::{Context, Result, bail};
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
    base.join("prt").join("config.toml")
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".cache"));
    base.join("prt")
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

pub fn load() -> Result<Config> {
    let path = config_path();
    let mut cfg: Config = match std::fs::read_to_string(&path) {
        Ok(s) => toml::from_str(&s).with_context(|| format!("invalid config {}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    for r in &mut cfg.repos {
        validate_name(&r.name)?;
        r.path = r.path.as_deref().map(expand_tilde);
    }
    // No repos configured: fall back to the repo we're standing in.
    if cfg.repos.is_empty() {
        if let Some(r) = cwd_repo() {
            cfg.repos.push(r);
        }
    }
    Ok(cfg)
}

/// Names end up inlined in GraphQL queries, so only allow GitHub's charset.
fn validate_name(name: &str) -> Result<()> {
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    match name.split_once('/') {
        Some((o, n)) if ok(o) && ok(n) => Ok(()),
        _ => bail!("invalid repo name {name:?}, expected owner/name"),
    }
}

fn cwd_repo() -> Option<RepoConfig> {
    let out = Command::new("git").args(["remote", "get-url", "origin"]).output().ok()?;
    let url = String::from_utf8(out.stdout).ok()?;
    let name = parse_github_remote(url.trim())?;
    let top = Command::new("git").args(["rev-parse", "--show-toplevel"]).output().ok()?;
    let path = PathBuf::from(String::from_utf8(top.stdout).ok()?.trim());
    Some(RepoConfig { name, path: Some(path) })
}

pub fn parse_github_remote(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
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
        assert_eq!(parse_github_remote("https://github.com/a/b-c.d").as_deref(), Some("a/b-c.d"));
        assert_eq!(parse_github_remote("https://gitlab.com/a/b"), None);
        assert!(validate_name("a/b\"){").is_err());
    }
}
