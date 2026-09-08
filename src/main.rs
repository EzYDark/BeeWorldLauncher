#[cfg(not(any(windows, all(target_os = "linux", target_arch = "x86_64"))))]
compile_error!("BeeWorld Launcher supports Windows and Linux x64.");
#[cfg(windows)]
mod app;
#[cfg(target_os = "linux")]
#[path = "linux/app.rs"]
mod app;
#[cfg(windows)]
mod dependencies;
#[cfg(target_os = "linux")]
#[path = "linux/dependencies.rs"]
mod dependencies;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod java;
#[cfg(target_os = "linux")]
#[path = "linux/java.rs"]
mod java;
#[cfg(target_os = "linux")]
mod linux;
mod logger;
#[cfg(windows)]
mod migration;
#[cfg(windows)]
mod self_update;
mod server;
mod server_process;
mod system;
#[cfg(windows)]
mod transaction;
#[cfg(windows)]
mod ui;
mod versions;
#[cfg(windows)]
mod workflow;
#[cfg(target_os = "linux")]
#[path = "linux/workflow.rs"]
mod workflow;

fn main() {
    if let Err(error) = run() {
        eprintln!("BeeWorld Launcher: {error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    linux::run()
}

#[cfg(windows)]
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
