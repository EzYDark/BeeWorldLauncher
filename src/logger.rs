use crate::ui::WorkerEvent;
use chrono::Local;
use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, mpsc::Sender},
};
pub struct Logger {
    file: Mutex<File>,
    path: PathBuf,
    events: Option<Sender<WorkerEvent>>,
}
impl Logger {
    pub fn new(directory: &Path, events: Option<Sender<WorkerEvent>>) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let path = directory.join(format!(
            "launcher-{}-{}.log",
            Local::now().format("%Y%m%d-%H%M%S-%f"),
            std::process::id()
        ));
        let file = File::create(&path)?;
        Ok(Self {
            file: Mutex::new(file),
            path,
            events,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn info(&self, message: impl AsRef<str>) {
        self.write("INFO", message.as_ref());
    }
    pub fn command_output(&self, message: impl AsRef<str>) {
        self.write("CMD", message.as_ref());
    }
    fn write(&self, level: &str, message: &str) {
        let clean: String = message
            .chars()
            .filter(|c| !c.is_control() || *c == '\t')
            .take(16000)
            .collect();
        if clean.is_empty() {
            return;
        }
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(
                file,
                "{} [{level}] {clean}",
                Local::now().format("%Y-%m-%d %H:%M:%S")
            );
            let _ = file.flush();
        }
        if let Some(events) = &self.events {
            let _ = events.send(WorkerEvent::Log(clean));
        }
    }
}
