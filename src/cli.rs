//! Command-line arguments. They override the config file.

use anyhow::{Result, anyhow, bail};
use std::path::PathBuf;

pub const HELP: &str = "\
revu — review GitHub pull requests from the terminal

Usage: revu [OPTIONS]

Lists the open pull requests and issues that involve you, grouped by repo
(`v` switches to your pinned repos, or to everything you can access).
Repos in the config are pinned at the top. Run from a clone, its repo is
pinned too and the clone is remembered for local file reads and language
servers.

Options:
  -c, --config <FILE>    config file to use instead of the default
      --theme <NAME>     syntax theme (any bat theme, e.g. \"Nord\")
  -h, --help             show this help
  -V, --version          show the version
";

#[derive(Debug, Default, PartialEq)]
pub struct Args {
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
            "-c" | "--config" => args.config = Some(PathBuf::from(value()?)),
            "--theme" => args.theme = Some(value()?),
            f => bail!("unknown argument {f} (see --help)"),
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
        let argv = ["--theme", "Nord", "-c", "f.toml"];
        let Command::Run(a) = run(&argv).unwrap() else { panic!() };
        assert_eq!(a.theme.as_deref(), Some("Nord"));
        assert_eq!(a.config, Some(PathBuf::from("f.toml")));
        assert!(run(&["--theme=Nord"]).is_ok());
        // Short flags take the next token, `=` is for long ones.
        assert!(run(&["-c=x"]).is_err());
        assert_eq!(run(&["--help"]).unwrap(), Command::Help);
        assert!(run(&["--theme"]).is_err());
        assert!(run(&["--nope"]).is_err());
        assert!(run(&["."]).is_err());
    }
}
