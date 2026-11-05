mod app;
mod cli;
mod config;
mod diff;
mod gh;
mod highlight;
mod lsp;
mod markdown;
mod source;

fn main() -> anyhow::Result<()> {
    let args = match cli::parse(std::env::args().skip(1))? {
        cli::Command::Help => {
            print!("{}\nconfig: {}\n", cli::HELP, config::config_path().display());
            return Ok(());
        }
        cli::Command::Version => {
            println!("revu {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        cli::Command::Run(args) => args,
    };
    let cfg = config::load(&args)?;
    let pr = match args.pr {
        Some(n) => Some((config::local_repo(std::path::Path::new("."))?.name, n)),
        None => None,
    };
    let mut terminal = app::init_terminal();
    let res = app::App::new(cfg, pr).run(&mut terminal);
    app::restore_terminal();
    res
}
