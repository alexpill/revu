//! Command-line arguments. They override the config file, and repos given
//! here replace the configured ones (so no config file is needed at all).

use anyhow::{Result, anyhow, bail};
use std::path::PathBuf;

pub const HELP: &str = "\
revu — review GitHub pull requests from the terminal

Usage: revu [REPO...] [OPTIONS]

REPO is a local directory (e.g. `.`), owner/name, or a GitHub URL
(https or ssh). Repos given on the command line replace the configured ones;
a local directory also enables local file reads and language servers.

Options:
  -r, --repo <REPO>      add a repo (repeatable), same forms as above
  -c, --config <FILE>    config file to use instead of the default
      --theme <NAME>     syntax theme (any bat theme, e.g. \"Nord\")
  -h, --help             show this help
  -V, --version          show the version

Examples:
  revu .
  revu ~/code/api --repo owner/frontend
  revu --repo git@github.com:owner/repo.git --theme \"Catppuccin Mocha\"
";

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub repos: Vec<String>,
    pub config: Option<PathBuf>,
    pub theme: Option<String>,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Run(Args),
    Help,
    Version,
}

pub fn parse(argv: impl IntoIterator<Item = String>) -> Result<Command> {
    let mut args = Args::default();
    let mut it = argv.into_iter();
    while let Some(arg) = it.next() {
        // --flag=value or --flag value
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) if arg.starts_with("--") => (f.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| it.next())
                .ok_or_else(|| anyhow!("{flag} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "-r" | "--repo" => args.repos.push(value()?),
            "-c" | "--config" => args.config = Some(PathBuf::from(value()?)),
            "--theme" => args.theme = Some(value()?),
            f if f.starts_with('-') => bail!("unknown option {f} (see --help)"),
            _ => args.repos.push(arg),
        }
    }
    Ok(Command::Run(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(argv: &[&str]) -> Result<Command> {
        parse(argv.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses() {
        let argv = [
            ".",
            "--repo",
            "a/b",
            "-r",
            "e/f",
            "--repo=c/d",
            "--theme",
            "Nord",
            "-c",
            "f.toml",
        ];
        let Command::Run(a) = run(&argv).unwrap() else { panic!() };
        assert_eq!(a.repos, vec![".", "a/b", "e/f", "c/d"]);
        assert_eq!(a.theme.as_deref(), Some("Nord"));
        assert_eq!(a.config, Some(PathBuf::from("f.toml")));
        // Short flags take the next token, `=` is for long ones.
        assert!(run(&["-r=x"]).is_err());
        assert_eq!(run(&["x", "--help"]).unwrap(), Command::Help);
        assert!(run(&["--repo"]).is_err());
        assert!(run(&["--nope"]).is_err());
    }
}
