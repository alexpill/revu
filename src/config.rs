//! User configuration, read from `$XDG_CONFIG_HOME/revu/config.toml` (or
//! `--config`), then overridden by command-line arguments.
//!
//! ```toml
//! [[repos]]               # pinned: listed first, with all their open PRs and issues
//! name = "owner/repo"
//! path = "~/code/repo"   # optional local clone (otherwise the remembered one, if any)
//!
//! theme = "Catppuccin Mocha"   # optional, any bat theme
//! clone_dir = "~/code"        # where `c` clones repos (default ~/code)
//! delete_branch = true        # delete the PR branch after merging from revu (default false)
//!
//! [lsp]                   # file extension -> server command (overrides the defaults)
//! rs = ["rust-analyzer"]
//!
//! [[commands]]            # PR screen: a key runs a shell command (`sh -c`) in the terminal
//! key = "X"               # one key, not already taken by the PR screen
//! run = "claude 'review {path}:{line} in {repo}#{pr}'"
//! # placeholders, quoted for the shell: {repo} (owner/name), {pr} (number),
//! # {path} {line} (under the cursor, else empty), {worktree} (PR checkout, else empty)
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
    pub clone_dir: Option<PathBuf>,
    #[serde(default)]
    pub commands: Vec<CustomCommand>,
    #[serde(default)]
    pub delete_branch: bool,
}

impl Config {
    pub fn clone_dir(&self) -> PathBuf {
        self.clone_dir.clone().unwrap_or_else(|| home().join("code"))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RepoConfig {
    pub name: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
pub struct CustomCommand {
    pub key: char,
    pub run: String,
}

impl CustomCommand {
    /// `run` with each `{name}` replaced by its value, quoted for where it sits
    /// (bare, '…' or "…"): values come from PRs, so they must never run as code.
    pub fn expand(&self, vars: &[(&str, &str)]) -> String {
        let (mut out, mut quote, mut escaped) = (String::new(), None, false);
        let mut rest = self.run.as_str();
        while let Some(c) = rest.chars().next() {
            let var = vars.iter().find(|(n, _)| {
                rest.strip_prefix('{')
                    .and_then(|r| r.strip_prefix(n))
                    .is_some_and(|r| r.starts_with('}'))
            });
            if let Some((n, v)) = var.filter(|_| !escaped) {
                match quote {
                    None => out += &format!("'{}'", v.replace('\'', r"'\''")),
                    Some('\'') => out += &v.replace('\'', r"'\''"),
                    _ => v.chars().for_each(|c| {
                        if "$`\"\\".contains(c) {
                            out.push('\\');
                        }
                        out.push(c);
                    }),
                }
                rest = &rest[n.len() + 2..];
                continue;
            }
            match (quote, c) {
                _ if escaped => escaped = false,
                (None | Some('"'), '\\') => escaped = true,
                (None, '\'' | '"') => quote = Some(c),
                (Some(q), _) if q == c => quote = None,
                _ => {}
            }
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
        out
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

/// Local clones revu learned about (run from a clone, or cloned by revu), one
/// `owner/name<TAB>path` per line. Owned by revu, unlike the config file.
fn links_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local").join("state"));
    base.join("revu").join("links")
}

/// Remembered clones by lowercased repo name; vanished directories are left out.
pub fn links() -> HashMap<String, PathBuf> {
    let text = std::fs::read_to_string(links_path()).unwrap_or_default();
    parse_links(&text).filter(|(_, p)| p.is_dir()).collect()
}

fn parse_links(text: &str) -> impl Iterator<Item = (String, PathBuf)> + '_ {
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(n, p)| (n.to_lowercase(), PathBuf::from(p)))
}

pub fn save_link(name: &str, path: &Path) -> Result<()> {
    let file = links_path();
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    let mut links: Vec<_> = parse_links(&text).filter(|(n, _)| *n != name.to_lowercase()).collect();
    links.push((name.to_lowercase(), path.to_path_buf()));
    let out: String = links.iter().map(|(n, p)| format!("{n}\t{}\n", p.display())).collect();
    std::fs::create_dir_all(file.parent().unwrap())?;
    std::fs::write(&file, out).with_context(|| format!("writing {}", file.display()))
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
    cfg.clone_dir = cfg.clone_dir.as_deref().map(expand_tilde);
    if let Some(c) = cfg.commands.iter().find(|c| crate::app::PR_KEYS.contains(c.key)) {
        bail!(
            "invalid config {}: command key {:?} is already used by the PR screen",
            path.display(),
            c.key
        );
    }
    let cmds = &cfg.commands;
    if let Some(c) = (0..cmds.len()).find_map(|i| cmds[..i].iter().find(|d| d.key == cmds[i].key)) {
        bail!(
            "invalid config {}: command key {:?} is used twice",
            path.display(),
            c.key
        );
    }
    // Run from a clone: remember where it is, and pin it for this run.
    if let Ok(r) = local_repo(Path::new(".")) {
        if let Some(p) = &r.path {
            let _ = save_link(&r.name, &main_checkout(p));
        }
        if !cfg.repos.iter().any(|c| c.name.eq_ignore_ascii_case(&r.name)) {
            cfg.repos.insert(0, r);
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

/// The GitHub repo of a local clone, with the clone's root as path.
pub fn local_repo(dir: &Path) -> Result<RepoConfig> {
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

/// The main checkout when `dir` is a linked worktree: worktrees come and go.
fn main_checkout(dir: &Path) -> PathBuf {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output();
    let common = out
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    match common.as_deref().filter(|c| c.ends_with(".git")).and_then(Path::parent) {
        Some(main) => main.to_path_buf(),
        None => dir.to_path_buf(), // bare repos and the like
    }
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
    fn command_placeholders() {
        let evil = "a b'$(echo x)\"`echo y`\\";
        let vars = [("path", evil), ("line", ""), ("pr", "7")];
        let run = |run: &str| {
            CustomCommand {
                key: 'X',
                run: run.into(),
            }
            .expand(&vars)
        };
        assert_eq!(run("echo {pr} {nope} ${HOME}"), "echo '7' {nope} ${HOME}");
        assert_eq!(run("x {line}"), "x ''");
        // Bare, in single and in double quotes, the value reaches the command as is.
        let wrapped = format!("<{evil}>");
        for (template, want) in [
            ("printf %s {path}", evil),
            ("printf %s '<{path}>'", &wrapped),
            ("printf %s \"<{path}>\"", &wrapped),
            ("printf %s \\{path}", "{path}"),
        ] {
            let out = Command::new("sh").arg("-c").arg(run(template)).output().unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), want, "{template}");
        }
    }

    #[test]
    fn links_file() {
        let v: Vec<_> = parse_links("Org/Repo\t/a b/c\nbroken line\n").collect();
        assert_eq!(v, vec![("org/repo".to_string(), PathBuf::from("/a b/c"))]);
    }
}
