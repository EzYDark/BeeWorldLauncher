use crate::{
    app::{AppPaths, Failure},
    self_update, system,
};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

pub fn install(release: &self_update::Release) -> Result<(), Failure> {
    let executable = std::env::current_exe()?;
    let parent = executable
        .parent()
        .ok_or_else(|| Failure::plain("Launcher location unavailable."))?;
    // Use the executable's directory, not server data writable by a different account.
    let staging = parent.join(".beeworld-launcher-update");
    let _lock = system::lock(&staging).map_err(|e| Failure::plain(format!("Cannot update this launcher: {e}. Run update-launcher as the account that owns its installation.")))?;
    let paths = AppPaths::new(staging)?;
    let ctx = super::context(&paths)?;
    let archive = self_update::download(&paths, release, &ctx)?;
    replace_archive(&archive, &executable)?;
    println!(
        "Launcher updated to {}. Reopen it to use the new version. Running servers are unchanged.",
        release.tag_name
    );
    Ok(())
}
fn replace_archive(archive: &Path, destination: &Path) -> Result<(), Failure> {
    let mut archive =
        tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(archive)?).take(120_000_000));
    let mut binary = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() != Path::new("beeworld-server") {
            continue;
        }
        if binary.is_some() || !entry.header().entry_type().is_file() || entry.size() > 100_000_000
        {
            return Err(Failure::plain("Invalid Linux launcher archive."));
        }
        let mut bytes = Vec::new();
        entry.by_ref().take(100_000_001).read_to_end(&mut bytes)?;
        if bytes.len() > 100_000_000 {
            return Err(Failure::plain("Launcher binary exceeds size limit."));
        }
        binary = Some(bytes);
    }
    let binary = binary.ok_or_else(|| Failure::plain("The archive is missing beeworld-server."))?;
    if binary.len() < 64 || &binary[..6] != b"\x7fELF\x02\x01" || binary[18..20] != [62, 0] {
        return Err(Failure::plain(
            "The downloaded launcher is not a Linux x64 executable.",
        ));
    }
    replace(&binary, destination)
}
fn replace(bytes: &[u8], destination: &Path) -> Result<(), Failure> {
    system::no_links(destination)?;
    let metadata = fs::metadata(destination)?;
    let parent = destination
        .parent()
        .ok_or_else(|| Failure::plain("Missing launcher directory."))?;
    let suffix = |value: &str| {
        let mut name = destination.as_os_str().to_os_string();
        name.push(value);
        std::path::PathBuf::from(name)
    };
    let staged = suffix(".update-new");
    let backup = suffix(".update-old");
    let backup_stage = suffix(".backup-new");
    for path in [&staged, &backup, &backup_stage] {
        system::no_links(path)?;
    }
    if backup_stage.exists() {
        return Err(Failure::plain(
            "An unfinished launcher backup exists. Rename the .backup-new file before retrying.",
        ));
    }
    // Do not overwrite leftover staging files after an interrupted update.
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    let mut created_backup = false;
    let result = (|| -> Result<(), Failure> {
        output.write_all(bytes)?;
        output.sync_all()?;
        std::os::unix::fs::chown(&staged, Some(metadata.uid()), Some(metadata.gid()))?;
        fs::set_permissions(&staged, fs::Permissions::from_mode(metadata.mode() & 0o777))?;
        fs::hard_link(destination, &backup_stage)?;
        created_backup = true;
        fs::rename(&backup_stage, &backup)?;
        fs::rename(&staged, destination)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staged);
        if created_backup {
            let _ = fs::remove_file(&backup_stage);
        }
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Downloads the published Linux release and replaces an isolated launcher copy"]
    fn published_linux_archive_verification_and_replacement() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let ctx = super::super::context(&paths).unwrap();
        let client = reqwest::blocking::Client::builder()
            .https_only(true)
            .timeout(std::time::Duration::from_secs(60))
            .user_agent("BeeWorldLauncher-verification")
            .build()
            .unwrap();
        let bytes = client
            .get("https://api.github.com/repos/EzYDark/BeeWorldLauncher/releases/tags/v0.3.0")
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .unwrap();
        let release: self_update::Release = serde_json::from_slice(&bytes).unwrap();
        let destination = temp.path().join("beeworld-server");
        fs::copy("/bin/true", &destination).unwrap();
        let original = fs::read(&destination).unwrap();
        let mut bad = release.clone();
        bad.assets
            .iter_mut()
            .find(|a| a.name == "BeeWorldLauncher-linux-x64.tar.gz")
            .unwrap()
            .digest = Some(format!("sha256:{}", "0".repeat(64)));
        assert!(self_update::download(&paths, &bad, &ctx).is_err());
        assert_eq!(fs::read(&destination).unwrap(), original);
        let downloaded = self_update::download(&paths, &release, &ctx).unwrap();
        replace_archive(&downloaded, &destination).unwrap();
        let output = std::process::Command::new(&destination)
            .arg("--help")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("BeeWorld server for Linux x64"));
        assert_eq!(
            fs::read(destination.with_extension("update-old")).unwrap(),
            original
        );
    }
    #[test]
    fn atomic_replacement_keeps_running_old_process_and_backup() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("beeworld-server");
        fs::copy("/bin/sleep", &destination).unwrap();
        let original = fs::read(&destination).unwrap();
        let mut child = std::process::Command::new(&destination)
            .arg("30")
            .spawn()
            .unwrap();
        let replacement = fs::read("/bin/true").unwrap();
        replace(&replacement, &destination).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        assert!(
            std::process::Command::new(&destination)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            fs::read(destination.with_extension("update-old")).unwrap(),
            original
        );
        assert_eq!(fs::read(&destination).unwrap(), replacement);
        child.kill().unwrap();
        child.wait().unwrap();
    }
    #[test]
    fn existing_stage_never_replaces_current_binary() {
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("beeworld-server");
        fs::write(&destination, b"original").unwrap();
        fs::write(destination.with_extension("update-new"), b"unfinished").unwrap();
        assert!(replace(b"new", &destination).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"original");
    }
    #[test]
    fn archive_rejects_missing_binary_and_symlink() {
        for symlink in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("release.tar.gz");
            let gzip = flate2::write::GzEncoder::new(
                fs::File::create(&path).unwrap(),
                flate2::Compression::default(),
            );
            let mut tar = tar::Builder::new(gzip);
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_mode(0o755);
            if symlink {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("/bin/true").unwrap();
            }
            header.set_cksum();
            tar.append_data(
                &mut header,
                if symlink {
                    "beeworld-server"
                } else {
                    "README.md"
                },
                &[][..],
            )
            .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
            let destination = temp.path().join("installed");
            fs::write(&destination, b"original").unwrap();
            assert!(replace_archive(&path, &destination).is_err());
            assert_eq!(fs::read(destination).unwrap(), b"original");
        }
    }
}
