#[cfg(not(windows))]
compile_error!("BeeWorld Launcher supports Windows only.");
mod app;
mod dependencies;
mod install;
mod java;
mod logger;
mod migration;
mod self_update;
mod server;
mod server_process;
mod system;
mod transaction;
mod ui;
mod versions;
mod workflow;

fn main() {
    if let Err(error) = run() {
        eprintln!("BeeWorld Launcher: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = app::Options::parse(std::env::args_os().skip(1))?;
    if options.help {
        println!(
            "BeeWorld Launcher\n\nStarts with a menu. No downloads or installation until you confirm.\n\n--data-dir PATH  Store portable data in PATH\n--check          Print local setup information without writing files\n--install       Open the menu with Install selected\n--uninstall     Open the menu with Uninstall selected\n--update-only   Open the menu with Update selected\n--help          Show this help"
        );
        return Ok(());
    }
    let paths = app::AppPaths::discover(options.data_dir)?;
    if options.check {
        println!("{}", paths.summary());
        return Ok(());
    }
    ui::run(paths, options.selection)?;
    Ok(())
}
