use crate::{
    app::Failure,
    system::{self, Context},
};
use std::{fs, io::Read, path::Path};

/// Build a dedicated-server payload without carrying player accounts or saves.
/// Fabric ignores environment=client itself; excluding those jars also avoids
/// shipping files the server will never use.
pub fn stage_pack(instance: &Path, destination: &Path, ctx: &Context) -> Result<usize, Failure> {
    system::no_links(instance)?;
    system::no_links(destination)?;
    if destination.exists() {
        return Err(Failure::plain(
            "Server staging folder already exists. It was not overwritten.",
        ));
    }
    fs::create_dir_all(destination.join("mods"))?;
    let source = instance.join("minecraft");
    let mut count = 0;
    for entry in fs::read_dir(source.join("mods"))? {
        ctx.check()?;
        let entry = entry?;
        let path = entry.path();
        system::no_links(&path)?;
        if !entry.file_type()?.is_file() || path.extension().is_none_or(|ext| ext != "jar") {
            continue;
        }
        if server_compatible(&path)? {
            fs::copy(path, destination.join("mods").join(entry.file_name()))?;
            count += 1;
        }
    }
    for folder in [
        "config",
        "defaultconfigs",
        "kubejs",
        "scripts",
        "global_packs",
    ] {
        let from = source.join(folder);
        if from.is_dir() {
            system::copy_tree(&from, &destination.join(folder), ctx)?;
        }
    }
    fs::copy(
        instance.join("mmc-pack.json"),
        destination.join("mmc-pack.json"),
    )?;
    Ok(count)
}
fn server_compatible(path: &Path) -> Result<bool, Failure> {
    let mut archive = zip::ZipArchive::new(fs::File::open(path)?)
        .map_err(|e| Failure::plain(format!("Cannot read mod {}: {e}", path.display())))?;
    let mut metadata = match archive.by_name("fabric.mod.json") {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(true),
        Err(e) => return Err(Failure::plain(e.to_string())),
    };
    if metadata.size() > 1024 * 1024 {
        return Err(Failure::plain("Mod metadata exceeds its size limit."));
    }
    let mut text = String::new();
    metadata.read_to_string(&mut text)?;
    // Keep non-strict metadata for Fabric to validate instead of dropping mods.
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Ok(true);
    };
    Ok(value["environment"].as_str() != Some("client"))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::logger::Logger;
    use std::{
        io::Write,
        sync::{Arc, atomic::AtomicBool},
    };
    fn jar(path: &Path, environment: &str) {
        let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
        zip.start_file("fabric.mod.json", zip::write::SimpleFileOptions::default())
            .unwrap();
        write!(zip, "{{\"environment\":\"{environment}\"}}").unwrap();
        zip.finish().unwrap();
    }
    #[test]
    fn server_payload_excludes_client_mods_accounts_and_player_worlds() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let instance = root.join("instance");
        fs::create_dir_all(instance.join("minecraft/mods")).unwrap();
        fs::create_dir_all(instance.join("minecraft/config")).unwrap();
        fs::create_dir_all(instance.join("minecraft/saves/precious-world")).unwrap();
        fs::write(instance.join("accounts.json"), "private fixture").unwrap();
        fs::write(instance.join("mmc-pack.json"), "{}").unwrap();
        fs::write(instance.join("minecraft/config/game.json"), "{}").unwrap();
        jar(&instance.join("minecraft/mods/client.jar"), "client");
        jar(&instance.join("minecraft/mods/common.jar"), "*");
        let ctx = Context {
            logger: Arc::new(Logger::new(&root.join("logs"), None).unwrap()),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let server = root.join("server");
        assert_eq!(stage_pack(&instance, &server, &ctx).unwrap(), 1);
        assert!(server.join("mods/common.jar").exists());
        assert!(!server.join("mods/client.jar").exists());
        assert!(!server.join("accounts.json").exists());
        assert!(!server.join("saves").exists());
        assert!(server.join("config/game.json").exists());
        assert!(instance.join("minecraft/saves/precious-world").exists());
        assert!(stage_pack(&instance, &server, &ctx).is_err());
    }
}

pub fn install(paths: &crate::app::AppPaths, ctx: &Context) -> Result<String, Failure> {
    let revision = crate::versions::desired(paths, crate::versions::Target::Server)?;
    install_revision(paths, ctx, revision)
}

fn install_revision(
    paths: &crate::app::AppPaths,
    ctx: &Context,
    revision: crate::versions::Revision,
) -> Result<String, Failure> {
    use crate::{
        dependencies,
        versions::{Target, Versions},
        workflow::git,
    };
    let root = paths.data_dir.join("server");
    system::no_links(&root)?;
    fs::create_dir_all(&root)?;
    let _runtime = system::lock(&root.join("active.runtime-lock"))?;
    recover_publication(&root)?;
    let stage = root.join("staging");
    let source = root.join("source-staging");
    if stage.exists() || source.exists() {
        return Err(Failure::plain(
            "An unfinished server download exists. Your running version is unchanged. Rename the staging folders before retrying.",
        ));
    }
    let mut settings = Versions::read(paths)?;
    dependencies::ensure_tools(paths, ctx, &[0, 1])?;
    crate::versions::validate_sha(&revision.sha)?;
    system::checked(
        git(paths, &root)?
            .args(["clone", "--no-checkout", crate::app::REPO_URL])
            .arg(&source),
        "Downloading server pack",
        ctx,
    )?;
    system::checked(
        git(paths, &source)?.args(["checkout", "--detach", &revision.sha]),
        "Selecting server version",
        ctx,
    )?;
    system::checked(
        git(paths, &source)?.args(["lfs", "pull", "origin", &revision.sha]),
        "Downloading server mods",
        ctx,
    )?;
    system::checked(
        git(paths, &source)?.args(["lfs", "fsck"]),
        "Checking server mods",
        ctx,
    )?;
    let count = stage_pack(&source, &stage, ctx)?;
    let pack: serde_json::Value = serde_json::from_slice(&fs::read(stage.join("mmc-pack.json"))?)?;
    let component = |uid: &str| {
        pack["components"]
            .as_array()
            .and_then(|a| a.iter().find(|c| c["uid"] == uid))
            .and_then(|c| c["version"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| Failure::plain("Server pack component is missing."))
    };
    let minecraft = component("net.minecraft")?;
    let loader = component("net.fabricmc.fabric-loader")?;
    if minecraft != "1.21.1" {
        return Err(Failure::plain(
            "This server installer currently supports BeeWorld's Minecraft 1.21.1. This version was not installed.",
        ));
    }
    let java = crate::java::server_java(paths, ctx)?;
    let installer = root.join("fabric-installer-1.1.2.jar");
    download_installer(&installer)?;
    system::checked(
        std::process::Command::new(&java)
            .arg("-jar")
            .arg(&installer)
            .args([
                "server",
                "-mcversion",
                &minecraft,
                "-loader",
                &loader,
                "-downloadMinecraft",
                "-dir",
            ])
            .arg(&stage),
        "Installing Fabric server",
        ctx,
    )?;
    let active = root.join("active");
    // Only runtime/player state is carried across pack versions. Never overwrite
    // these files with repository defaults or silently drop a custom world name.
    if active.exists() {
        for entry in fs::read_dir(&active)? {
            let entry = entry?;
            let name = entry.file_name();
            if [
                "mods",
                "config",
                "defaultconfigs",
                "kubejs",
                "scripts",
                "global_packs",
                "libraries",
                ".fabric",
                ".launcher-runtime",
                "mmc-pack.json",
                "server.jar",
                "fabric-server-launch.jar",
                "fabric-server-launcher.properties",
                "java-path.json",
            ]
            .iter()
            .any(|n| name == *n)
            {
                continue;
            }
            system::no_links(&entry.path())?;
            if entry.file_type()?.is_dir() {
                system::copy_tree(&entry.path(), &stage.join(name), ctx)?;
            } else {
                fs::copy(entry.path(), stage.join(name))?;
            }
        }
    }
    fs::write(stage.join("eula.txt"), "eula=true\n")?;
    fs::write(stage.join("java-path.json"), serde_json::to_vec(&java)?)?;
    fs::write(
        stage.join(".beeworld-version.json"),
        serde_json::to_vec(&revision)?,
    )?;
    ctx.check()?;
    // Runtime lock is outside the active folder and remains held across publication.
    let backup = root.join(format!("backup-{}", chrono::Utc::now().timestamp_millis()));
    fs::write(
        root.join("publish.json"),
        serde_json::to_vec(&backup.file_name().unwrap().to_string_lossy().as_ref())?,
    )?;
    if active.exists() {
        fs::rename(&active, &backup)?;
    }
    if let Err(error) = fs::rename(&stage, &active) {
        if backup.exists() {
            fs::rename(&backup, &active)?;
        }
        return Err(error.into());
    }
    fs::remove_file(root.join("publish.json"))?;
    settings.record_installed(Target::Server, revision)?;
    settings.save(paths)?;
    system::remove_owned_tree(&source, &root)?;
    Ok(format!(
        "Server ready with {count} mods. Choose Start server."
    ))
}
#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;
    use crate::{app::AppPaths, logger::Logger, server_process::ServerProcess, versions::Revision};
    use std::{
        sync::{Arc, atomic::AtomicBool},
        time::{Duration, Instant},
    };
    #[test]
    #[ignore = "Downloads a fixed pack revision into an isolated Linux fixture and runs Minecraft"]
    fn linux_server_install_update_and_shutdown() {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(temp.path().join("data")).unwrap();
        let ctx = Context {
            logger: Arc::new(Logger::new(&temp.path().join("logs"), None).unwrap()),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        // Test-only revision injection: production installs still require a stable release.
        let revision = Revision {
            sha: "1340bf3029fe7e4e0766c25175a299d4870e2119".into(),
            label: "Linux compatibility fixture".into(),
            date: String::new(),
        };
        install_revision(&paths, &ctx, revision.clone()).unwrap();
        let active = paths.data_dir.join("server/active");
        let properties = "server-ip=127.0.0.1\nserver-port=25589\nview-distance=2\nsimulation-distance=2\nmax-players=2\nonline-mode=true\n";
        fs::write(active.join("server.properties"), properties).unwrap();
        let java = crate::java::server_java(&paths, &ctx).unwrap();
        let mut server = ServerProcess::start(
            &java,
            &active,
            &paths.data_dir.join("logs/server-console.log"),
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(180);
        while !server.output().contains("Done (") {
            assert!(server.poll().unwrap().is_none(), "{}", server.output());
            assert!(Instant::now() < deadline, "{}", server.output());
            std::thread::sleep(Duration::from_millis(100));
        }
        server.command("list").unwrap();
        server.stop().unwrap();
        while server.poll().unwrap().is_none() {
            assert!(Instant::now() < deadline, "Server failed to stop");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(server.output().contains("Saving"), "{}", server.output());
        drop(server);
        let world = fs::read(active.join("world/level.dat")).unwrap();
        // Minecraft expands server.properties during its first launch.
        let properties = fs::read_to_string(active.join("server.properties")).unwrap();
        install_revision(&paths, &ctx, revision).unwrap();
        assert_eq!(fs::read(active.join("world/level.dat")).unwrap(), world);
        assert_eq!(
            fs::read_to_string(active.join("server.properties")).unwrap(),
            properties
        );
        assert!(!paths.instance_dir.exists());
        println!(
            "Linux fixture retained for CLI checks: {}",
            temp.keep().display()
        );
    }
}

fn download_installer(destination: &Path) -> Result<(), Failure> {
    use sha2::{Digest, Sha256};
    const SHA: &str = "61e035bf7bf70153e127440ce34de47c9036f0a2d0c65d1529454bd35ceefe4f";
    if let Ok(bytes) = fs::read(destination)
        && format!("{:x}", Sha256::digest(&bytes)) == SHA
    {
        return Ok(());
    }
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| Failure::plain(e.to_string()))?;
    let bytes = client.get("https://maven.fabricmc.net/net/fabricmc/fabric-installer/1.1.2/fabric-installer-1.1.2.jar").send().and_then(|r| r.error_for_status()).and_then(|r| r.bytes()).map_err(|e| Failure::plain(e.to_string()))?;
    if format!("{:x}", Sha256::digest(&bytes)) != SHA {
        return Err(Failure::plain("Fabric installer checksum failed."));
    }
    fs::write(destination, bytes)?;
    Ok(())
}

#[cfg(all(test, windows))]
mod live_tests {
    use super::*;
    #[test]
    #[ignore = "Updates the verified real server fixture and checks world preservation"]
    fn real_server_update_preserves_world_and_properties() {
        use crate::{app::AppPaths, logger::Logger};
        use std::sync::{Arc, atomic::AtomicBool};
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/verification/server");
        let paths = AppPaths {
            executable: root.join("launcher.exe"),
            data_dir: root.clone(),
            instance_dir: root.join("unused-instance"),
            prism_root: root.join("unused-prism"),
            install_dir: root.join("unused-install"),
        };
        let active = root.join("server/active");
        let before = fs::read(active.join("world/level.dat"))
            .expect("Run real_server_install_and_start first");
        let properties = fs::read(active.join("server.properties")).unwrap();
        fs::write(active.join("world/player-preservation-fixture"), "keep me").unwrap();
        let ctx = Context {
            logger: Arc::new(Logger::new(&root.join("logs"), None).unwrap()),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        install(&paths, &ctx).unwrap();
        assert_eq!(fs::read(active.join("world/level.dat")).unwrap(), before);
        assert_eq!(
            fs::read(active.join("server.properties")).unwrap(),
            properties
        );
        assert_eq!(
            fs::read_to_string(active.join("world/player-preservation-fixture")).unwrap(),
            "keep me"
        );
        assert!(!paths.prism_root.exists());
        assert!(
            fs::read_dir(root.join("server"))
                .unwrap()
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("backup-")
                    && e.path().join("world/level.dat").exists())
        );
    }
    #[test]
    #[ignore = "Downloads the real BeeWorld server into target/verification/server"]
    fn real_server_install_and_start() {
        use crate::{app::AppPaths, logger::Logger};
        use std::sync::{Arc, atomic::AtomicBool};
        let root =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/verification/server");
        let paths = AppPaths {
            executable: root.join("launcher.exe"),
            data_dir: root.clone(),
            instance_dir: root.join("unused-instance"),
            prism_root: root.join("unused-prism"),
            install_dir: root.join("unused-install"),
        };
        fs::create_dir_all(&root).unwrap();
        let ctx = Context {
            logger: Arc::new(Logger::new(&root.join("logs"), None).unwrap()),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        if !root.join("server/active/fabric-server-launch.jar").exists() {
            println!("{}", install(&paths, &ctx).unwrap());
        }
        assert!(!paths.prism_root.exists());
        let folder = root.join("server/active");
        fs::write(folder.join("server.properties"), "server-ip=127.0.0.1\nserver-port=25579\nview-distance=2\nsimulation-distance=2\nmax-players=2\n").unwrap();
        let java: std::path::PathBuf =
            serde_json::from_slice(&fs::read(folder.join("java-path.json")).unwrap()).unwrap();
        let mut server = crate::server_process::ServerProcess::start(
            &java,
            &folder,
            &root.join("logs/live-console.log"),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let mut ready = false;
        loop {
            if let Some(status) = server.poll().unwrap() {
                panic!(
                    "Server exited before readiness {status}: {}",
                    server.output()
                );
            }
            if server.output().contains("Done (") {
                ready = true;
                break;
            }
            if std::time::Instant::now() > deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        server.stop().unwrap();
        loop {
            if let Some(status) = server.poll().unwrap() {
                assert!(status.success(), "{}", server.output());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        assert!(ready, "Server did not become ready: {}", server.output());
    }
}

fn recover_publication(root: &Path) -> Result<(), Failure> {
    let journal = root.join("publish.json");
    if !journal.exists() {
        return Ok(());
    }
    let name: String = serde_json::from_slice(&fs::read(&journal)?)?;
    if !name.starts_with("backup-")
        || !name[7..].bytes().all(|b| b.is_ascii_digit())
        || name.len() <= 7
    {
        return Err(Failure::plain("Invalid server recovery record."));
    }
    let active = root.join("active");
    let backup = root.join(name);
    system::no_links(&active)?;
    system::no_links(&backup)?;
    if !active.exists() && backup.exists() {
        fs::rename(backup, active)?;
    }
    fs::remove_file(journal)?;
    Ok(())
}
#[cfg(test)]
mod recovery_tests {
    use super::*;
    #[test]
    fn interrupted_server_publication_restores_worlds() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir_all(root.join("backup-123/world")).unwrap();
        fs::write(root.join("backup-123/world/level.dat"), "precious").unwrap();
        fs::write(root.join("publish.json"), "\"backup-123\"").unwrap();
        recover_publication(root).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("active/world/level.dat")).unwrap(),
            "precious"
        );
        assert!(!root.join("publish.json").exists());
    }
}
