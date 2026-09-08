use crate::{
    app::Failure,
    system::{self, Context},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    id: String,
    had_original: bool,
}
pub struct Transaction {
    pub target: PathBuf,
    pub root: PathBuf,
}
impl Transaction {
    pub fn new(target: &Path) -> Result<Self, Failure> {
        system::no_links(target)?;
        let target = std::path::absolute(target)?;
        let parent = target
            .parent()
            .ok_or_else(|| Failure::plain("Instance has no parent folder"))?;
        let identity = format!(
            "{:x}",
            Sha256::digest(target.to_string_lossy().to_lowercase().as_bytes())
        );
        Ok(Self {
            root: parent.join(format!(".beeworld-{}", &identity[..16])),
            target,
        })
    }
    pub fn pending(&self) -> bool {
        self.root.join("journal.json").exists()
    }
    fn journal(&self) -> Result<Journal, Failure> {
        let record: Journal = serde_json::from_slice(&fs::read(self.root.join("journal.json"))?)?;
        if record.id.is_empty() || !record.id.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
            return Err(Failure::plain(
                "Invalid recovery record. No folders were changed.",
            ));
        }
        Ok(record)
    }
    fn stage(&self, record: &Journal) -> PathBuf {
        self.root.join(format!("stage-{}", record.id))
    }
    fn backup(&self, record: &Journal) -> PathBuf {
        self.root.join(format!("backup-{}", record.id))
    }
    pub fn begin(&self) -> Result<PathBuf, Failure> {
        system::no_links(&self.root)?;
        fs::create_dir_all(&self.root)?;
        if self.pending() {
            return Err(Failure::plain(
                "An unfinished update exists. Choose Recover interrupted update first.",
            ));
        }
        let record = Journal {
            id: format!(
                "{:020}-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| Failure::plain(e.to_string()))?
                    .as_nanos(),
                std::process::id()
            ),
            had_original: self.target.exists(),
        };
        system::write_new(
            &self.root.join("journal.json"),
            &serde_json::to_vec(&record)?,
        )?;
        let stage = self.stage(&record);
        fs::create_dir(&stage)?;
        Ok(stage)
    }
    pub fn publish(&self, ctx: &Context) -> Result<(), Failure> {
        ctx.check()?;
        let record = self.journal()?;
        let stage = self.stage(&record);
        let backup = self.backup(&record);
        system::no_links(&stage)?;
        system::no_links(&self.target)?;
        validate_pack(&stage)?;
        if record.had_original {
            fs::rename(&self.target, &backup)?;
        } else if self.target.exists() {
            return Err(Failure::plain(
                "An instance appeared during setup. It was not overwritten.",
            ));
        }
        // No cancellation between these two renames. On crash, journal recovery
        // restores backup if target is missing. Both paths are on the same volume.
        if let Err(error) = fs::rename(&stage, &self.target) {
            if record.had_original {
                fs::rename(&backup, &self.target).map_err(|restore| Failure::plain(format!(
                    "Switch failed: {error}. Automatic restoration failed: {restore}. Choose Recover. Backup: {}", backup.display())))?;
            }
            return Err(error.into());
        }
        fs::remove_file(self.root.join("journal.json"))?;
        ctx.logger
            .info(format!("Updated instance: {}", self.target.display()));
        if record.had_original {
            ctx.logger
                .info(format!("Full backup: {}", backup.display()));
        }
        Ok(())
    }
    pub fn recover(&self, ctx: &Context) -> Result<(), Failure> {
        let record = self.journal()?;
        let stage = self.stage(&record);
        let backup = self.backup(&record);
        system::no_links(&backup)?;
        system::no_links(&self.target)?;
        if !self.target.exists() && record.had_original {
            if !backup.is_dir() {
                return Err(Failure::plain(
                    "Original and backup are both missing. Recovery stopped.",
                ));
            }
            fs::rename(&backup, &self.target)?;
            ctx.logger.info("Restored the original instance.");
        }
        // A target plus a backup means publication finished before the record
        // was cleared. Retain both. Never replace the active instance here.
        if stage.exists() {
            system::remove_owned_tree(&stage, &self.root)?;
        }
        fs::remove_file(self.root.join("journal.json"))?;
        ctx.logger
            .info("Recovery finished. Existing backups were kept.");
        Ok(())
    }
    pub fn latest_backup(&self) -> Result<PathBuf, Failure> {
        let mut backups = Vec::new();
        if self.root.is_dir() {
            for entry in fs::read_dir(&self.root)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if let Some(id) = name.strip_prefix("backup-")
                    && !id.is_empty()
                    && id.bytes().all(|b| b.is_ascii_digit() || b == b'-')
                    && entry.file_type()?.is_dir()
                {
                    backups.push(entry.path());
                }
            }
        }
        backups.sort();
        backups
            .pop()
            .ok_or_else(|| Failure::plain("No previous version is available."))
    }
    pub fn restore(&self, ctx: &Context) -> Result<(), Failure> {
        let backup = self.latest_backup()?;
        validate_pack(&backup)?;
        let stage = self.begin()?;
        ctx.logger
            .info(format!("Copying backup: {}", backup.display()));
        system::copy_tree(&backup, &stage, ctx)?;
        self.publish(ctx)
    }
}
pub fn validate_pack(path: &Path) -> Result<(), Failure> {
    let data: serde_json::Value = serde_json::from_slice(&fs::read(path.join("mmc-pack.json"))?)?;
    let valid = data
        .get("components")
        .and_then(|v| v.as_array())
        .is_some_and(|components| {
            components.iter().any(|c| {
                c.get("uid").and_then(|v| v.as_str()) == Some("net.minecraft")
                    && c.get("version")
                        .and_then(|v| v.as_str())
                        .is_some_and(|v| !v.is_empty())
            })
        });
    if !valid {
        return Err(Failure::plain(
            "mmc-pack.json has no valid Minecraft component.",
        ));
    }
    if !path.join("instance.cfg").is_file() {
        return Err(Failure::plain("instance.cfg is missing."));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger::Logger;
    use std::sync::{Arc, atomic::AtomicBool};
    fn context(root: &Path) -> Context {
        Context {
            logger: Arc::new(Logger::new(&root.join("logs"), None).unwrap()),
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    fn pack(path: &Path, world: &str) {
        fs::create_dir_all(path.join("minecraft/saves")).unwrap();
        fs::write(
            path.join("mmc-pack.json"),
            r#"{"components":[{"uid":"net.minecraft","version":"1.21.1"}]}"#,
        )
        .unwrap();
        fs::write(path.join("instance.cfg"), "[General]\nname=BeeWorld").unwrap();
        fs::write(path.join("minecraft/saves/world"), world).unwrap();
    }
    #[test]
    fn staging_failure_leaves_world_and_instance_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("BeeWorld");
        pack(&target, "player world");
        let tx = Transaction::new(&target).unwrap();
        let stage = tx.begin().unwrap();
        fs::write(stage.join("incomplete"), "broken download").unwrap();
        assert!(tx.publish(&context(temp.path())).is_err());
        assert_eq!(
            fs::read_to_string(target.join("minecraft/saves/world")).unwrap(),
            "player world"
        );
        tx.recover(&context(temp.path())).unwrap();
        assert!(!stage.exists());
        assert!(!tx.pending());
    }
    #[test]
    fn publish_and_restore_keep_both_generations_and_worlds() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("BeeWorld");
        pack(&target, "old world");
        let tx = Transaction::new(&target).unwrap();
        let stage = tx.begin().unwrap();
        pack(&stage, "new world");
        tx.publish(&context(temp.path())).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("minecraft/saves/world")).unwrap(),
            "new world"
        );
        tx.restore(&context(temp.path())).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("minecraft/saves/world")).unwrap(),
            "old world"
        );
        assert_eq!(
            fs::read_to_string(tx.latest_backup().unwrap().join("minecraft/saves/world")).unwrap(),
            "new world"
        );
    }
    #[test]
    fn crash_between_renames_restores_original() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("BeeWorld");
        pack(&target, "precious");
        let tx = Transaction::new(&target).unwrap();
        let stage = tx.begin().unwrap();
        pack(&stage, "new");
        fs::rename(&target, tx.backup(&tx.journal().unwrap())).unwrap();
        tx.recover(&context(temp.path())).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("minecraft/saves/world")).unwrap(),
            "precious"
        );
    }
    #[test]
    fn crash_after_publication_keeps_new_instance_and_backup() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("BeeWorld");
        pack(&target, "old");
        let tx = Transaction::new(&target).unwrap();
        let stage = tx.begin().unwrap();
        pack(&stage, "new");
        fs::rename(&target, tx.backup(&tx.journal().unwrap())).unwrap();
        fs::rename(stage, &target).unwrap();
        tx.recover(&context(temp.path())).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("minecraft/saves/world")).unwrap(),
            "new"
        );
        assert!(tx.latest_backup().unwrap().exists());
    }
}
