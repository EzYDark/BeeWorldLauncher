use crate::{
    app::{AppPaths, Failure},
    logger::Logger,
    server_process::ServerProcess,
    system::{self, Context},
    versions::{self, Target, Versions},
};
use std::{
    fs,
    io::{self, BufRead, IsTerminal, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

#[derive(Debug, Default)]
struct Options {
    command: String,
    data: Option<PathBuf>,
    version: Option<String>,
    yes: bool,
    eula: bool,
    no_console: bool,
    input: Option<String>,
}
impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, Failure> {
        let mut options = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--data-dir" => {
                    options.data = Some(
                        args.next()
                            .ok_or_else(|| Failure::plain("--data-dir needs a folder"))?
                            .into(),
                    )
                }
                "--version" => {
                    options.version =
                        Some(args.next().ok_or_else(|| {
                            Failure::plain("--version needs a release tag or latest")
                        })?)
                }
                "--yes" => options.yes = true,
                "--accept-eula" => options.eula = true,
                "--no-console" => options.no_console = true,
                "--command" => {
                    options.input = Some(args.next().ok_or_else(|| {
                        Failure::plain("--command needs one quoted server command")
                    })?)
                }
                "--help" | "-h" => options.command = "help".into(),
                "install" | "update" | "start" | "status" | "versions" | "logs" | "help"
                | "command" | "stop"
                    if options.command.is_empty() =>
                {
                    options.command = arg
                }
                _ => return Err(Failure::plain(format!("Unknown argument: {arg}"))),
            }
        }
        if options.command == "command" && options.input.is_none() {
            return Err(Failure::plain(
                "Use command --command \"list\" to send a server command.",
            ));
        }
        if options.version.is_some() && !matches!(options.command.as_str(), "install" | "update") {
            return Err(Failure::plain("--version is used with install or update."));
        }
        Ok(options)
    }
}
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options::parse(std::env::args().skip(1))?;
    if options.command == "help" {
        help();
        return Ok(());
    }
    let data = options.data.clone().unwrap_or_else(|| {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("beeworld-launcher")
    });
    let paths = AppPaths::new(data)?;
    if !options.command.is_empty() {
        dispatch(&paths, &options)?;
        return Ok(());
    }
    if !io::stdin().is_terminal() {
        help();
        return Ok(());
    }
    loop {
        println!(
            "\nBeeWorld server\n1. Start\n2. Install or update\n3. Versions\n4. Status\n5. Quit"
        );
        match read_line("Choose: ")?.as_str() {
            "1" => options.command = "start".into(),
            "2" => options.command = "install".into(),
            "3" => options.command = "versions".into(),
            "4" => options.command = "status".into(),
            "5" | "" => break,
            _ => continue,
        }
        if let Err(error) = dispatch(&paths, &options) {
            eprintln!("{error}");
        }
        // The console owns stdin until process exit.
        if options.command == "start" {
            break;
        }
    }
    Ok(())
}
fn help() {
    println!(
        "BeeWorld server for Linux x64\n\nOpen without arguments for the menu.\n\ninstall / update   Install a stable release after confirmation\nstart              Check updates, then run the server and console\nversions           List stable releases\nstatus             Show installed and selected versions\nlogs               Print recent server logs\ncommand            Send --command \"list\" to a running server\nstop               Save and stop a running server\n\n--data-dir PATH    Choose the data folder\n--version TAG      Select a release for install/update, or latest\n--yes              Confirm this operation without a prompt\n--accept-eula      Accept https://www.minecraft.net/eula for installation\n--no-console       Run without stdin commands, suitable for a service\n\nRequires Git, Git LFS and Java 21 x64. Ctrl+C or SIGTERM stops the server cleanly."
    );
}
fn read_line(prompt: &str) -> Result<String, Failure> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    Ok(line.trim().into())
}
fn confirm(options: &Options, text: &str) -> Result<bool, Failure> {
    println!("{text}");
    if options.yes {
        return Ok(true);
    }
    if !io::stdin().is_terminal() {
        return Err(Failure::plain(
            "Confirmation required. Use --yes for an unattended operation.",
        ));
    }
    Ok(read_line("Continue? [y/N] ")?.eq_ignore_ascii_case("y"))
}
fn context(paths: &AppPaths) -> Result<Context, Failure> {
    Ok(Context {
        logger: Arc::new(Logger::new(&paths.data_dir.join("logs"), None)?),
        cancelled: Arc::new(AtomicBool::new(false)),
    })
}
fn dispatch(paths: &AppPaths, options: &Options) -> Result<(), Failure> {
    match options.command.as_str() {
        "command" | "stop" => {
            let command = if options.command == "stop" {
                "stop"
            } else {
                options.input.as_deref().unwrap()
            };
            if command.is_empty() || command.len() > 4096 || command.chars().any(char::is_control) {
                return Err(Failure::plain(
                    "Use one server command without line breaks.",
                ));
            }
            let mut socket =
                std::os::unix::net::UnixStream::connect(paths.data_dir.join("server/control.sock"))
                    .map_err(|_| {
                        Failure::plain("No running BeeWorld server found in this data folder.")
                    })?;
            socket.set_read_timeout(Some(Duration::from_secs(5)))?;
            socket.set_write_timeout(Some(Duration::from_secs(5)))?;
            writeln!(socket, "{command}")?;
            let mut reply = String::new();
            io::BufReader::new(socket)
                .take(4096)
                .read_to_string(&mut reply)?;
            if !reply.starts_with("OK") {
                return Err(Failure::plain(reply));
            }
            println!("{}", reply.trim());
        }
        "versions" => {
            let rows = versions::catalogue()?;
            if rows.is_empty() {
                println!("No stable releases available yet.");
            }
            for release in rows {
                println!("{}  {}", release.label, release.date);
            }
            println!(
                "Use install --version TAG to choose one. Use --version latest to follow new releases."
            );
        }
        "status" => {
            let settings = Versions::read(paths)?;
            println!(
                "Installed: {}\nSelected: {}\nData: {}",
                settings
                    .installed(Target::Server)
                    .map_or("Not installed", |v| &v.label),
                settings
                    .selected(Target::Server)
                    .map_or("Latest stable release", |v| &v.label),
                paths.data_dir.display()
            );
        }
        "logs" => {
            let file = fs::File::open(paths.data_dir.join("logs/server-console.log"))?;
            let mut lines = std::collections::VecDeque::new();
            for line in io::BufReader::new(file).lines() {
                lines.push_back(line?);
                if lines.len() > 100 {
                    lines.pop_front();
                }
            }
            for line in lines {
                println!("{line}");
            }
        }
        "install" | "update" => {
            if !confirm(
                options,
                "Install the selected stable release? Worlds are kept; pack configuration is replaced.",
            )? {
                return Ok(());
            }
            if !options.eula {
                if !io::stdin().is_terminal() {
                    return Err(Failure::plain(
                        "Server installation requires --accept-eula: https://www.minecraft.net/eula",
                    ));
                }
                if !read_line("Accept https://www.minecraft.net/eula? [y/N] ")?
                    .eq_ignore_ascii_case("y")
                {
                    return Ok(());
                }
            }
            let _lock = system::lock(&paths.data_dir)?;
            if let Some(tag) = &options.version {
                let selected = if tag == "latest" {
                    None
                } else {
                    Some(
                        versions::catalogue()?
                            .into_iter()
                            .find(|v| v.label == *tag)
                            .ok_or_else(|| {
                                Failure::plain("That stable release is not available.")
                            })?,
                    )
                };
                let mut settings = Versions::read(paths)?;
                settings.select(Target::Server, selected)?;
                settings.save(paths)?;
            }
            let ctx = context(paths)?;
            println!(
                "Installing. Detailed progress: {}",
                ctx.logger.path().display()
            );
            println!("{}", crate::server::install(paths, &ctx)?);
        }
        "start" => {
            let settings = Versions::read(paths)?;
            match versions::desired(paths, Target::Server) {
                Ok(desired)
                    if settings
                        .installed(Target::Server)
                        .is_none_or(|v| v.sha != desired.sha) =>
                {
                    println!(
                        "A different server release is available: {}. Run update to install it.",
                        desired.label
                    )
                }
                Ok(_) => println!("Installed server matches the selected release."),
                Err(_) => {
                    println!("Could not check releases. You can still start the installed server.")
                }
            }
            if !confirm(
                options,
                "Start the installed server? No update will be installed.",
            )? {
                return Ok(());
            }
            run_server(paths, options.no_console)?;
        }
        _ => help(),
    }
    Ok(())
}
fn run_server(paths: &AppPaths, no_console: bool) -> Result<(), Failure> {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    let ctx = context(paths)?;
    let java = crate::java::server_java(paths, &ctx)?;
    let _control_lock = system::lock(&paths.data_dir.join("server/control.runtime-lock"))?;
    let socket_path = paths.data_dir.join("server/control.sock");
    system::no_links(&socket_path)?;
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }
    let control = UnixListener::bind(&socket_path)?;
    let _socket_cleanup = SocketCleanup(socket_path.clone());
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
    control.set_nonblocking(true)?;
    let log_path = paths.data_dir.join("logs/server-console.log");
    let previous_length = fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
    let stopping = Arc::new(AtomicBool::new(false));
    let sigint = signal_hook::flag::register(signal_hook::consts::SIGINT, stopping.clone())?;
    let sigterm = signal_hook::flag::register(signal_hook::consts::SIGTERM, stopping.clone())?;
    let result = (|| {
        let mut server =
            ServerProcess::start(&java, &paths.data_dir.join("server/active"), &log_path)?;
        let mut log = fs::File::open(&log_path)?;
        log.seek(SeekFrom::Start(previous_length))?;
        let (sender, input) = mpsc::channel();
        if !no_console {
            std::thread::spawn(move || {
                for line in io::stdin().lock().lines() {
                    match line {
                        Ok(line) => {
                            if sender.send(line).is_err() {
                                return;
                            }
                        }
                        _ => return,
                    }
                }
                let _ = sender.send("stop".into());
            });
        }
        println!("Server console. Type stop to save and close.");
        loop {
            match control.accept() {
                Ok((mut socket, _)) => {
                    socket.set_read_timeout(Some(Duration::from_millis(500)))?;
                    socket.set_write_timeout(Some(Duration::from_millis(500)))?;
                    let mut line = String::new();
                    let result = io::BufReader::new(&socket).take(4098).read_line(&mut line);
                    let result = match result {
                        Ok(_) if line.ends_with('\n') => {
                            let command = line.trim_end_matches('\n');
                            if command == "stop" {
                                server.stop()
                            } else {
                                server.command(command)
                            }
                        }
                        _ => Err(Failure::plain("Invalid server command.")),
                    };
                    let reply = match result {
                        Ok(()) => "OK: command sent".into(),
                        Err(e) => format!("Error: {e}"),
                    };
                    let _ = writeln!(socket, "{reply}");
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(e.into()),
            }
            if stopping.load(Ordering::Relaxed) {
                server.stop()?;
            }
            for command in input.try_iter() {
                if command.trim() == "stop" {
                    server.stop()?;
                } else if let Err(error) = server.command(command.trim()) {
                    eprintln!("{error}");
                }
            }
            let status = server.poll()?;
            let mut bytes = Vec::new();
            log.read_to_end(&mut bytes)?;
            if !bytes.is_empty() {
                print!("{}", String::from_utf8_lossy(&bytes));
                io::stdout().flush()?;
            }
            if let Some(status) = status {
                if !status.success() {
                    return Err(Failure::plain(format!(
                        "Server exited with {status}. Check the server log."
                    )));
                }
                println!("Server stopped.");
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    })();
    signal_hook::low_level::unregister(sigint);
    signal_hook::low_level::unregister(sigterm);
    result
}
struct SocketCleanup(PathBuf);
impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automation_requires_explicit_flags_and_keeps_version_scoped() {
        let options = Options::parse(
            ["install", "--yes", "--accept-eula", "--version", "v1"].map(String::from),
        )
        .unwrap();
        assert!(options.yes && options.eula);
        assert_eq!(options.version.as_deref(), Some("v1"));
        assert!(!Options::parse(["install".into()]).unwrap().eula);
        assert!(Options::parse(["start", "--version", "v1"].map(String::from)).is_err());
        assert!(Options::parse(["--data-dir".into()]).is_err());
    }
}
