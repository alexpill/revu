mod app;
mod config;
mod diff;
mod gh;
mod highlight;
mod lsp;
mod markdown;
mod source;

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|a| a == "-h" || a == "--help") {
        println!(
            "prt — review GitHub pull requests from the terminal\n\nconfig: {}",
            config::config_path().display()
        );
        return Ok(());
    }
    let cfg = config::load()?;
    let mut terminal = ratatui::init();
    let res = app::App::new(cfg).run(&mut terminal);
    ratatui::restore();
    res
}
