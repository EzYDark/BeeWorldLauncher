use crate::{
    app::{AppPaths, Failure},
    system::{self, Context},
    transaction::Transaction,
};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};

pub fn copy_to_permanent(paths: &AppPaths, ctx: &Context) -> Result<AppPaths, Failure> {
    // Hold this across the copy so a server cannot start while its world is read.
    let _server_lock = system::lock(&paths.data_dir.join("server/active.runtime-lock"))?;
    let destination = paths.permanent();
    if same(&paths.data_dir, &destination.data_dir) {
        if paths.instance_dir != destination.instance_dir {
            return Err(Failure::plain(
                "Permanent storage is already in use with a custom instance override. Remove BEEWORLD_INSTANCE_PATH before installing; the external instance was not changed.",
            ));
        }
        return Ok(destination);
    }
    if destination.data_dir.exists() {
        return Err(Failure::plain(
            "Permanent data already exists. Restart the launcher to use it. No folders were merged or overwritten.",
        ));
    }
    let tx = Transaction::new(&paths.instance_dir)?;
    if tx.pending() {
        return Err(Failure::plain(
            "Recover the interrupted update before installing.",
        ));
    }
    let _instance_lock = system::lock(&tx.root)?;
    system::no_links(&destination.data_dir)?;
    let stage = paths.install_dir.join("data-migration");
    let source = paths.data_dir.canonicalize()?;
    let install = paths.install_dir.canonicalize()?;
    if install.starts_with(&source) || source.starts_with(&install) {
        return Err(Failure::plain(
            "Source and installation folders overlap. Choose a separate data folder before installing.",
        ));
    }
    if stage.exists() {
        return Err(Failure::plain(format!(
            "An unfinished data copy exists at {}. Your original data is intact. Rename this unfinished folder, then retry.",
            stage.display()
        )));
    }
    fs::create_dir_all(&stage)?;
    ctx.logger
        .info("Copying launcher data to permanent storage. Original files will be kept.");
    copy_verified(&paths.data_dir, &stage, ctx)?;
    let staged_instance = stage.join("prism-profile/instances/BeeWorld");
    let old_relative = paths.instance_dir.strip_prefix(&paths.data_dir).ok();
    if old_relative != Some(Path::new("prism-profile/instances/BeeWorld"))
        && paths.instance_dir.exists()
    {
        if staged_instance.exists() {
            return Err(Failure::plain(
                "A second instance occupies the migration destination. Original data was kept.",
            ));
        }
        copy_verified(&paths.instance_dir, &staged_instance, ctx)?;
    }
    // Transaction directory keys depend on the absolute instance path.
    let new_tx = Transaction::new(&destination.instance_dir)?;
    let stage_tx = staged_instance
        .parent()
        .unwrap()
        .join(new_tx.root.file_name().unwrap());
    if tx.root.exists() {
        let copied_old = tx
            .root
            .strip_prefix(&paths.data_dir)
            .ok()
            .map(|p| stage.join(p));
        if let Some(old) = copied_old.filter(|p| p.exists()) {
            if old != stage_tx {
                fs::rename(old, &stage_tx)?;
            }
        } else {
            copy_verified(&tx.root, &stage_tx, ctx)?;
        }
    }
    // Only configuration paths are rewritten; account files/worlds stay byte-identical.
    for relative in [
        "prism-profile/prismlauncher.cfg",
        "prism-profile/instances/BeeWorld/instance.cfg",
    ] {
        let config = stage.join(relative);
        if config.is_file() {
            let original = fs::read_to_string(&config)?;
            let text = original
                .replace(
                    &paths.data_dir.to_string_lossy().replace('\\', "/"),
                    &destination.data_dir.to_string_lossy().replace('\\', "/"),
                )
                .replace(
                    paths.data_dir.to_string_lossy().as_ref(),
                    destination.data_dir.to_string_lossy().as_ref(),
                );
            let text = if relative.ends_with("prismlauncher.cfg") {
                crate::workflow::ini_set(
                    &text,
                    "InstanceDir",
                    &destination
                        .instance_dir
                        .parent()
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                )
            } else {
                text
            };
            fs::write(config, text)?;
        }
    }
    let server_java = stage.join("server/active/java-path.json");
    if server_java.is_file() {
        let original: std::path::PathBuf = serde_json::from_slice(&fs::read(&server_java)?)?;
        if let Ok(relative) = original.strip_prefix(&paths.data_dir) {
            fs::write(
                &server_java,
                serde_json::to_vec(&destination.data_dir.join(relative))?,
            )?;
        }
    }
    // Tool selections are rediscovered after migration instead of pointing into Downloads.
    let selections = stage.join("system-tools.json");
    if selections.exists() {
        fs::remove_file(selections)?;
    }
    ctx.check()?;
    fs::rename(&stage, &destination.data_dir)?;
    ctx.logger.info(format!(
        "Verified data copy ready: {}",
        destination.data_dir.display()
    ));
    Ok(destination)
}
fn same(a: &Path, b: &Path) -> bool {
    a.canonicalize()
        .ok()
        .zip(b.canonicalize().ok())
        .is_some_and(|(a, b)| a == b)
}
fn hash(path: &Path) -> Result<Vec<u8>, Failure> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().to_vec())
}
fn copy_verified(source: &Path, target: &Path, ctx: &Context) -> Result<(), Failure> {
    system::no_links(source)?;
    system::no_links(target)?;
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        ctx.check()?;
        let entry = entry?;
        if entry.file_name() == "operation.lock" {
            continue;
        }
        system::no_links(&entry.path())?;
        let dest = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_verified(&entry.path(), &dest, ctx)?;
        } else if entry.file_type()?.is_file() {
            fs::copy(entry.path(), &dest)?;
            if hash(&entry.path())? != hash(&dest)? {
                return Err(Failure::plain(
                    "Data changed during copying. Original data was kept.",
                ));
            }
        } else {
            return Err(Failure::plain("Cannot migrate a special file."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger::Logger;
    use std::sync::{Arc, atomic::AtomicBool};
    #[test]
    fn external_instance_and_backups_are_copied_without_changing_source() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let paths = AppPaths {
            executable: root.join("launcher.exe"),
            data_dir: root.join("portable"),
            prism_root: root.join("portable/prism-profile"),
            instance_dir: root.join("external/Custom"),
            install_dir: root.join("installed"),
        };
        fs::create_dir_all(&paths.prism_root).unwrap();
        fs::create_dir_all(&paths.instance_dir).unwrap();
        fs::create_dir_all(&paths.install_dir).unwrap();
        fs::write(paths.instance_dir.join("world"), "keep").unwrap();
        let tx = Transaction::new(&paths.instance_dir).unwrap();
        fs::create_dir_all(tx.root.join("backup-1")).unwrap();
        fs::write(tx.root.join("backup-1/world"), "backup").unwrap();
        let ctx = Context {
            logger: Arc::new(Logger::new(&root.join("logs"), None).unwrap()),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let migrated = copy_to_permanent(&paths, &ctx).unwrap();
        assert_eq!(
            fs::read_to_string(migrated.instance_dir.join("world")).unwrap(),
            "keep"
        );
        assert_eq!(
            fs::read_to_string(
                Transaction::new(&migrated.instance_dir)
                    .unwrap()
                    .root
                    .join("backup-1/world")
            )
            .unwrap(),
            "backup"
        );
        assert!(paths.instance_dir.join("world").exists());
    }
    #[test]
    fn copied_worlds_accounts_and_backups_survive_source_removal() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let paths = AppPaths {
            executable: root.join("Downloads/launcher.exe"),
            data_dir: root.join("Downloads/data"),
            install_dir: root.join("Local/BeeWorldLauncher"),
            prism_root: root.join("Downloads/data/prism-profile"),
            instance_dir: root.join("Downloads/data/prism-profile/instances/BeeWorld"),
        };
        fs::create_dir_all(&paths.instance_dir).unwrap();
        fs::create_dir_all(&paths.install_dir).unwrap();
        fs::write(paths.instance_dir.join("world"), b"precious world").unwrap();
        fs::write(paths.prism_root.join("accounts.json"), b"private fixture").unwrap();
        let server = paths.data_dir.join("server/active");
        fs::create_dir_all(server.join("world")).unwrap();
        fs::write(server.join("world/level.dat"), b"server world").unwrap();
        fs::write(
            server.join("java-path.json"),
            serde_json::to_vec(&paths.data_dir.join("tools/java/bin/java.exe")).unwrap(),
        )
        .unwrap();
        let old_tx = Transaction::new(&paths.instance_dir).unwrap();
        fs::create_dir_all(old_tx.root.join("backup-123")).unwrap();
        fs::write(old_tx.root.join("backup-123/world"), b"backup world").unwrap();
        let ctx = Context {
            logger: Arc::new(Logger::new(&root.join("test-logs"), None).unwrap()),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let migrated = copy_to_permanent(&paths, &ctx).unwrap();
        assert!(paths.instance_dir.join("world").exists());
        let migrated_java: std::path::PathBuf = serde_json::from_slice(
            &fs::read(migrated.data_dir.join("server/active/java-path.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            migrated_java,
            migrated.data_dir.join("tools/java/bin/java.exe")
        );
        assert_eq!(
            fs::read(migrated.data_dir.join("server/active/world/level.dat")).unwrap(),
            b"server world"
        );
        system::remove_owned_tree(&paths.data_dir, root).unwrap();
        assert_eq!(
            fs::read(migrated.instance_dir.join("world")).unwrap(),
            b"precious world"
        );
        assert_eq!(
            fs::read(migrated.prism_root.join("accounts.json")).unwrap(),
            b"private fixture"
        );
        let new_tx = Transaction::new(&migrated.instance_dir).unwrap();
        assert_eq!(
            fs::read(new_tx.root.join("backup-123/world")).unwrap(),
            b"backup world"
        );
    }
}
