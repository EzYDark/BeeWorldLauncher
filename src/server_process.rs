use crate::{app::Failure, system};
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufRead, BufReader, Write},
    os::windows::process::CommandExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver},
    },
    thread,
};

/// Owns one headless process. Commands go directly to stdin, never a shell.
/// stdout and stderr are drained continuously even when the console is hidden.
pub struct ServerProcess {
    child: Child,
    input: Option<ChildStdin>,
    lines: Receiver<String>,
    recent: VecDeque<String>,
    readers: Vec<thread::JoinHandle<()>>,
    stopping: bool,
    _lock: File,
}
impl ServerProcess {
    pub fn start(java: &Path, folder: &Path, log: &Path) -> Result<Self, Failure> {
        system::no_links(folder)?;
        if !folder.join("fabric-server-launch.jar").is_file() {
            return Err(Failure::plain("Download the server before starting it."));
        }
        if !fs::read_to_string(folder.join("eula.txt"))
            .unwrap_or_default()
            .lines()
            .any(|line| line.trim() == "eula=true")
        {
            return Err(Failure::plain(
                "Accept the Minecraft EULA in server setup first.",
            ));
        }
        let mut command = Command::new(java);
        command.current_dir(folder).args([
            "-Xms512M",
            "-Xmx4G",
            "-jar",
            "fabric-server-launch.jar",
            "nogui",
        ]);
        Self::spawn(&mut command, folder, log)
    }
    fn spawn(command: &mut Command, folder: &Path, log: &Path) -> Result<Self, Failure> {
        let lock = system::lock(&folder.with_extension("runtime-lock"))?;
        system::no_links(log)?;
        if let Some(parent) = log.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = Arc::new(Mutex::new(
            fs::OpenOptions::new().create(true).append(true).open(log)?,
        ));
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(0x08000000)
            .spawn()?;
        let input = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Failure::plain("Server output unavailable."))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| Failure::plain("Server error output unavailable."))?;
        // The channel itself is bounded; slow rendering cannot consume unlimited RAM.
        let (sender, lines) = mpsc::sync_channel(1000);
        let reader = |stream: Box<dyn std::io::Read + Send>,
                      sender: mpsc::SyncSender<String>,
                      file: Arc<Mutex<File>>| {
            thread::spawn(move || {
                for line in BufReader::new(stream).split(b'\n') {
                    let Ok(bytes) = line else {
                        break;
                    };
                    let text: String = String::from_utf8_lossy(&bytes)
                        .chars()
                        .filter(|c| !c.is_control() || *c == '\t')
                        .take(8192)
                        .collect();
                    if let Ok(mut file) = file.lock() {
                        let _ = writeln!(file, "{text}");
                    }
                    let _ = sender.try_send(text);
                }
            })
        };
        let readers = vec![
            reader(Box::new(stdout), sender.clone(), file.clone()),
            reader(Box::new(stderr), sender, file),
        ];
        Ok(Self {
            child,
            input,
            lines,
            recent: VecDeque::new(),
            readers,
            stopping: false,
            _lock: lock,
        })
    }
    pub fn command(&mut self, text: &str) -> Result<(), Failure> {
        if text.is_empty() || text.len() > 4096 || text.chars().any(char::is_control) {
            return Err(Failure::plain(
                "Enter one server command without line breaks.",
            ));
        }
        if self.child.try_wait()?.is_some() {
            return Err(Failure::plain("The server has stopped."));
        }
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| Failure::plain("Server input is closed."))?;
        writeln!(input, "{text}")?;
        input.flush()?;
        Ok(())
    }
    pub fn stop(&mut self) -> Result<(), Failure> {
        if !self.stopping {
            self.command("stop")?;
            self.stopping = true;
        }
        Ok(())
    }
    pub fn poll(&mut self) -> Result<Option<std::process::ExitStatus>, Failure> {
        for line in self.lines.try_iter() {
            self.recent.push_back(line);
            if self.recent.len() > 1000 {
                self.recent.pop_front();
            }
        }
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.input.take();
            for reader in self.readers.drain(..) {
                let _ = reader.join();
            }
            for line in self.lines.try_iter() {
                self.recent.push_back(line);
                if self.recent.len() > 1000 {
                    self.recent.pop_front();
                }
            }
        }
        Ok(status)
    }
    pub fn output(&self) -> String {
        self.recent.iter().cloned().collect::<Vec<_>>().join("\n")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn console_streams_hidden_logs_sends_literal_commands_and_stops_cleanly() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut command = system::powershell();
        command.arg("[Console]::WriteLine('ready'); while($null -ne ($line=[Console]::ReadLine())) { if($line -eq 'stop') { [Console]::WriteLine('saved'); exit 0 }; [Console]::WriteLine('received:'+ $line) }");
        let mut process =
            ServerProcess::spawn(&mut command, root, &root.join("logs/console.log")).unwrap();
        assert!(
            ServerProcess::spawn(&mut system::powershell(), root, &root.join("other.log")).is_err()
        );
        assert!(process.command("say first\nstop").is_err());
        process
            .command("say literal; $(not-a-shell-command)")
            .unwrap();
        process.stop().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = process.poll().unwrap() {
                assert!(status.success());
                break;
            }
            if std::time::Instant::now() > deadline {
                process.child.kill().unwrap();
                process.child.wait().unwrap();
                panic!("Fixture did not stop");
            }
            thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            process
                .output()
                .contains("received:say literal; $(not-a-shell-command)")
        );
        assert!(process.output().contains("saved"));
        assert!(
            fs::read_to_string(root.join("logs/console.log"))
                .unwrap()
                .contains("ready")
        );
    }
}
