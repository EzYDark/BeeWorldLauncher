use crate::{app::Failure, logger::Logger};
use std::os::windows::{fs::MetadataExt, process::CommandExt};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub struct Context {
    pub logger: Arc<Logger>,
    pub cancelled: Arc<AtomicBool>,
}
impl Context {
    pub fn check(&self) -> Result<(), Failure> {
        if self.cancelled.load(Ordering::Relaxed) {
            Err(Failure::plain(
                "Cancelled. Use Recover if an unfinished update is listed.",
            ))
        } else {
            Ok(())
        }
    }
}

pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}
pub fn run(command: &mut Command, label: &str, ctx: &Context) -> Result<CommandResult, Failure> {
    run_timeout(command, label, ctx, Duration::from_secs(1200))
}
pub fn run_timeout(
    command: &mut Command,
    label: &str,
    ctx: &Context,
    timeout: Duration,
) -> Result<CommandResult, Failure> {
    ctx.check()?;
    ctx.logger.info(label);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(0x08000000);
    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Failure::plain("No command output pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Failure::plain("No command error pipe"))?;
    thread::scope(|scope| {
        let out = scope.spawn(|| read_output(stdout, ctx));
        let err = scope.spawn(|| read_output(stderr, ctx));
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Ok(status);
            }
            if ctx.cancelled.load(Ordering::Relaxed) || started.elapsed() > timeout {
                // Kill only this command's tree, including Git's HTTP/LFS children.
                let _ = Command::new(windows_tool("taskkill.exe"))
                    .args(["/PID", &child.id().to_string(), "/T", "/F"])
                    .creation_flags(0x08000000)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                let _ = child.wait();
                break Err(Failure::plain(
                    "Command cancelled or exceeded its time limit.",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        };
        let stdout = out
            .join()
            .map_err(|_| Failure::plain("Command output reader failed"))??;
        let stderr = err
            .join()
            .map_err(|_| Failure::plain("Command error reader failed"))??;
        let status = status?;
        Ok::<_, Failure>(CommandResult {
            stdout,
            stderr,
            success: status.success(),
        })
    })
}
fn read_output(reader: impl Read, ctx: &Context) -> Result<String, Failure> {
    let mut collected = String::new();
    for line in BufReader::new(reader).split(b'\n') {
        let bytes = line?;
        let line = String::from_utf8_lossy(&bytes);
        ctx.logger.command_output(&line);
        if collected.len() < 4 * 1024 * 1024 {
            collected.push_str(&line);
            collected.push('\n');
        }
    }
    Ok(collected)
}
pub fn checked(command: &mut Command, label: &str, ctx: &Context) -> Result<String, Failure> {
    let result = run(command, label, ctx)?;
    if !result.success {
        let detail = result
            .stderr
            .lines()
            .rev()
            .find(|s| !s.trim().is_empty())
            .unwrap_or("The command returned a failure exit code.");
        return Err(Failure::new(
            label,
            detail,
            "Read the log. Return to the menu; do not remove your instance.",
        ));
    }
    Ok(result.stdout)
}
pub fn windows_tool(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()))
        .join("System32")
        .join(name)
}
pub fn powershell() -> Command {
    let mut command = Command::new(windows_tool(r"WindowsPowerShell\v1.0\powershell.exe"));
    command.args(["-NoProfile", "-NonInteractive", "-Command"]);
    command
}
pub fn ps_literal(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "''"))
}

// Windows directory junctions are reparse points too. Do not follow them during
// backup, extraction, cleanup or publication.
pub fn no_links(path: &Path) -> Result<(), Failure> {
    for part in path.ancestors() {
        match fs::symlink_metadata(part) {
            Ok(metadata) if metadata.file_attributes() & 0x400 != 0 => {
                return Err(Failure::plain(format!(
                    "Linked folders are not supported here: {}",
                    part.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
pub fn copy_tree(source: &Path, destination: &Path, ctx: &Context) -> Result<(), Failure> {
    ctx.check()?;
    no_links(source)?;
    no_links(destination)?;
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        ctx.check()?;
        let entry = entry?;
        no_links(&entry.path())?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target, ctx)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(Failure::plain("Special files cannot be backed up."));
        }
    }
    Ok(())
}
pub fn remove_owned_tree(path: &Path, owner: &Path) -> Result<(), Failure> {
    let owner = owner.canonicalize()?;
    let target = path.canonicalize()?;
    if target == owner || !target.starts_with(&owner) {
        return Err(Failure::plain(
            "Cleanup target is outside its owning folder.",
        ));
    }
    no_links(path)?;
    // Validate every descendant before a recursive deletion.
    fn check_tree(path: &Path) -> Result<(), Failure> {
        no_links(path)?;
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                check_tree(&entry?.path())?;
            }
        }
        Ok(())
    }
    check_tree(path)?;
    fs::remove_dir_all(path)?;
    Ok(())
}
pub fn lock(folder: &Path) -> Result<File, Failure> {
    no_links(folder)?;
    fs::create_dir_all(folder)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(folder.join("operation.lock"))?;
    file.try_lock()
        .map_err(|_| Failure::plain("Another BeeWorld Launcher operation is using this folder."))?;
    Ok(file)
}
pub fn write_new(path: &Path, contents: &[u8]) -> Result<(), Failure> {
    use std::io::Write;
    no_links(path)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    Ok(())
}
