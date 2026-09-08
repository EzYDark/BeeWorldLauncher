use crate::{
    app::{AppPaths, Failure},
    system::{self, Context},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Package {
    pub name: &'static str,
    pub folder: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
    pub executable: &'static str,
}
// Pinned upstream releases, with GitHub's release-asset SHA-256 digests.
// MinGW Prism does not require a separately installed MSVC runtime.
pub const PACKAGES: &[Package] = &[
    Package {
        name: "MinGit",
        folder: "mingit-2.55.0.5",
        url: "https://github.com/git-for-windows/git/releases/download/v2.55.0.windows.5/MinGit-2.55.0.5-64-bit.zip",
        sha256: "56d7b226b7693196cfc71fef26568f536c4a021ab6c37ff2db4287bed908e96e",
        bytes: 38989688,
        executable: "cmd/git.exe",
    },
    Package {
        name: "Git LFS",
        folder: "git-lfs-3.8.0",
        url: "https://github.com/git-lfs/git-lfs/releases/download/v3.8.0/git-lfs-windows-amd64-v3.8.0.zip",
        sha256: "b62e7b8ceddee635f691233d77de8eaa4b213e9209e0173811d8cfa77f7882c1",
        bytes: 5862396,
        executable: "git-lfs.exe",
    },
    Package {
        name: "Prism Launcher",
        folder: "prism-11.1.0",
        url: "https://github.com/PrismLauncher/PrismLauncher/releases/download/11.1.0/PrismLauncher-Windows-MinGW-w64-Portable-11.1.0.zip",
        sha256: "2bf5e879ea1c3f6a1aaaa43539667ce296308abf3e6a984d5cc4c48bfe3c431c",
        bytes: 43926838,
        executable: "prismlauncher.exe",
    },
];
pub const OFFLINE_PRISM: Package = Package {
    name: "Prism offline-account fork",
    folder: "prism-offline-11.0.3",
    url: "https://github.com/Diegiwg/PrismLauncher-Cracked/releases/download/11.0.3/PrismLauncher-Windows-MinGW-w64-Portable-11.0.3.zip",
    sha256: "62e7f814e4d5baddee1531b60d174f21ae2b34d2397ccabff97be65004e293d9",
    bytes: 43239177,
    executable: "prismlauncher.exe",
};
pub const SERVER_JAVA: Package = Package {
    name: "Java 21",
    folder: "java21-21.0.12.1",
    url: "https://github.com/adoptium/temurin21-binaries/releases/download/jdk-21.0.12.1%2B1/OpenJDK21U-jre_x64_windows_hotspot_21.0.12.1_1.zip",
    sha256: "d35f31e712f0fcf6ac5a093edc90204fbff22f720ba3950bd09d331d5e621636",
    bytes: 48999141,
    executable: "bin/java.exe",
};
pub fn offline_mode(paths: &AppPaths) -> bool {
    paths.data_dir.join("offline-mode").is_file()
}
pub fn git_path(paths: &AppPaths) -> PathBuf {
    selected_path(paths, 0)
}
pub fn lfs_path(paths: &AppPaths) -> PathBuf {
    selected_path(paths, 1)
}
pub fn prism_path(paths: &AppPaths) -> PathBuf {
    if offline_mode(paths) {
        package_path(paths, &OFFLINE_PRISM)
    } else {
        selected_path(paths, 2)
    }
}
fn package_path(paths: &AppPaths, package: &Package) -> PathBuf {
    paths
        .tools_dir()
        .join(package.folder)
        .join(package.executable)
}
fn selections(paths: &AppPaths) -> std::collections::BTreeMap<String, PathBuf> {
    fs::read(paths.data_dir.join("system-tools.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
fn selected_path(paths: &AppPaths, index: usize) -> PathBuf {
    selections(paths)
        .remove(PACKAGES[index].name)
        .filter(|p| p.is_absolute() && p.is_file())
        .unwrap_or_else(|| package_path(paths, &PACKAGES[index]))
}
pub fn ensure(paths: &AppPaths, ctx: &Context) -> Result<(), Failure> {
    ensure_tools(paths, ctx, &[0, 1, 2])
}
pub fn ensure_tools(paths: &AppPaths, ctx: &Context, indexes: &[usize]) -> Result<(), Failure> {
    if !cfg!(target_arch = "x86_64") {
        return Err(Failure::plain("This build requires Windows x64."));
    }
    system::no_links(&paths.tools_dir())?;
    let mut selected = selections(paths);
    for &index in indexes {
        if index == 2 && offline_mode(paths) {
            fs::create_dir_all(paths.tools_dir())?;
            ensure_package(&paths.tools_dir(), &OFFLINE_PRISM, ctx)?;
            continue;
        }
        let package = &PACKAGES[index];
        let mut candidates = tool_candidates(index);
        if index == 1
            && let Some(git) = selected.get(PACKAGES[0].name).and_then(|p| p.parent())
        {
            candidates.push(git.join("git-lfs.exe"));
            candidates.push(git.join("../mingw64/bin/git-lfs.exe"));
        }
        let mut found = None;
        for candidate in candidates {
            ctx.check()?;
            if !candidate.is_file() {
                continue;
            }
            let mut command = std::process::Command::new(&candidate);
            if index == 2 {
                command.arg("--dir").arg(&paths.prism_root);
            }
            command.arg(if index == 1 { "version" } else { "--version" });
            if let Ok(result) = system::run_timeout(
                &mut command,
                "Checking installed tool",
                ctx,
                Duration::from_secs(15),
            ) {
                let output = format!("{} {}", result.stdout, result.stderr);
                if result.success && compatible_tool(index, &output) {
                    found = Some(candidate);
                    break;
                }
            }
        }
        let executable = if let Some(path) = found {
            ctx.logger.info(format!(
                "Using installed {}: {}",
                package.name,
                path.display()
            ));
            path
        } else {
            ctx.check()?;
            fs::create_dir_all(paths.tools_dir())?;
            ensure_package(&paths.tools_dir(), package, ctx)?;
            package_path(paths, package)
        };
        selected.insert(package.name.to_owned(), executable);
    }
    fs::write(
        paths.data_dir.join("system-tools.json"),
        serde_json::to_vec_pretty(&selected)?,
    )?;
    Ok(())
}
fn compatible_tool(index: usize, output: &str) -> bool {
    let prefix = ["git version ", "git-lfs/", "PrismLauncher "][index];
    output
        .split_once(prefix)
        .and_then(|(_, v)| numeric_version(v))
        .is_some_and(|v| v >= [(2, 30, 0), (3, 0, 0), (9, 0, 0)][index])
}
pub(crate) fn numeric_version(text: &str) -> Option<(u32, u32, u32)> {
    let token = text.split_whitespace().next()?;
    let mut parts = token.split('.');
    let number = |p: &str| {
        p.chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    };
    Some((
        number(parts.next()?)?,
        number(parts.next().unwrap_or("0"))?,
        number(parts.next().unwrap_or("0"))?,
    ))
}
pub(crate) fn on_path(name: &str) -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .filter(|p| p.is_absolute())
                .map(|p| p.join(name))
                .collect()
        })
        .unwrap_or_default()
}
fn tool_candidates(index: usize) -> Vec<PathBuf> {
    let name = ["git.exe", "git-lfs.exe", "prismlauncher.exe"][index];
    let mut result = on_path(name);
    let suffixes: &[&str] = match index {
        0 => &["Git/cmd/git.exe"],
        1 => &[
            "Git/cmd/git-lfs.exe",
            "Git/mingw64/bin/git-lfs.exe",
            "Git LFS/git-lfs.exe",
        ],
        _ => &[
            "PrismLauncher/prismlauncher.exe",
            "Programs/PrismLauncher/prismlauncher.exe",
            "Prism Launcher/prismlauncher.exe",
        ],
    };
    for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(variable) {
            for suffix in suffixes {
                result.push(PathBuf::from(&base).join(suffix));
            }
        }
    }
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        result.push(
            PathBuf::from(profile)
                .join("scoop/apps")
                .join(["git", "git-lfs", "prismlauncher"][index])
                .join("current")
                .join(if index == 0 { "cmd/git.exe" } else { name }),
        );
    }
    result
}
pub fn ensure_package(tools: &Path, package: &Package, ctx: &Context) -> Result<(), Failure> {
    let destination = tools.join(package.folder);
    let receipt = destination.join(".beeworld-verified");
    if fs::read_to_string(&receipt).ok().as_deref() == Some(package.sha256)
        && destination.join(package.executable).is_file()
    {
        return Ok(());
    }
    if destination.exists() {
        return Err(Failure::plain(format!(
            "Incomplete tool folder: {}. Rename it before preparing tools again.",
            destination.display()
        )));
    }
    let staging = tools.join(format!(".{}-download", package.folder));
    // This private scratch folder never contains user data. Clear a failed prior download.
    if staging.exists() {
        system::remove_owned_tree(&staging, tools)?;
    }
    fs::create_dir(&staging)?;
    let result = (|| {
        let archive = staging.join("download.zip");
        download(package, &archive, ctx)?;
        let unpack = staging.join("unpacked");
        fs::create_dir(&unpack)?;
        extract(&archive, &unpack, ctx)?;
        // Git LFS archives use a versioned enclosing directory in some releases.
        if !unpack.join(package.executable).is_file() {
            let entries = fs::read_dir(&unpack)?.collect::<Result<Vec<_>, _>>()?;
            if entries.len() == 1
                && entries[0].path().is_dir()
                && entries[0].path().join(package.executable).is_file()
            {
                let inner = entries[0].path();
                for entry in fs::read_dir(&inner)? {
                    let entry = entry?;
                    fs::rename(entry.path(), unpack.join(entry.file_name()))?;
                }
                fs::remove_dir(inner)?;
            }
        }
        if !unpack.join(package.executable).is_file() {
            return Err(Failure::plain(format!(
                "{} archive is missing its executable.",
                package.name
            )));
        }
        fs::write(unpack.join(".beeworld-verified"), package.sha256)?;
        ctx.check()?;
        fs::rename(&unpack, &destination)?;
        Ok(())
    })();
    let cleanup = system::remove_owned_tree(&staging, tools);
    result?;
    cleanup?;
    ctx.logger.info(format!("{} is ready.", package.name));
    Ok(())
}
fn download(package: &Package, destination: &Path, ctx: &Context) -> Result<(), Failure> {
    ctx.logger.info(format!(
        "Downloading {}: {} MB",
        package.name,
        package.bytes / 1_000_000
    ));
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(600))
        .user_agent("BeeWorldLauncher/0.2.0")
        .build()
        .map_err(|e| Failure::plain(e.to_string()))?;
    let mut response = client
        .get(package.url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| Failure::plain(format!("Download failed: {e}")))?;
    let mut output = File::create(destination)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut bytes = 0u64;
    let mut reported = 0u64;
    loop {
        ctx.check()?;
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > package.bytes {
            return Err(Failure::plain("Download exceeded its pinned size."));
        }
        hasher.update(&buffer[..count]);
        output.write_all(&buffer[..count])?;
        let progress = bytes * 100 / package.bytes;
        if progress >= reported + 10 {
            ctx.logger.info(format!("{}: {progress}%", package.name));
            reported = progress;
        }
    }
    output.sync_all()?;
    if bytes != package.bytes || format!("{:x}", hasher.finalize()) != package.sha256 {
        return Err(Failure::plain(
            "Downloaded archive failed its size or SHA-256 check. Nothing was executed.",
        ));
    }
    Ok(())
}
pub fn extract(archive: &Path, destination: &Path, ctx: &Context) -> Result<(), Failure> {
    let mut archive =
        zip::ZipArchive::new(File::open(archive)?).map_err(|e| Failure::plain(e.to_string()))?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        ctx.check()?;
        let mut entry = archive
            .by_index(i)
            .map_err(|e| Failure::plain(e.to_string()))?;
        let relative = entry
            .enclosed_name()
            .ok_or_else(|| Failure::plain("ZIP contains a path outside its destination."))?;
        // Reject Windows aliases/streams as well as ZIP traversal and symlinks.
        if relative.components().any(|c| {
            let part = c.as_os_str().to_string_lossy();
            part.contains(':') || part.ends_with('.') || part.ends_with(' ')
        }) || entry
            .unix_mode()
            .is_some_and(|mode| mode & 0o170000 == 0o120000)
        {
            return Err(Failure::plain(
                "ZIP contains an unsupported path or symbolic link.",
            ));
        }
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| Failure::plain("ZIP size overflow"))?;
        if total > 2 * 1024 * 1024 * 1024 {
            return Err(Failure::plain("ZIP expands beyond the 2 GiB limit."));
        }
        let target = destination.join(relative);
        system::no_links(&target)?;
        if entry.is_dir() {
            fs::create_dir_all(target)?;
        } else {
            fs::create_dir_all(
                target
                    .parent()
                    .ok_or_else(|| Failure::plain("ZIP path has no parent"))?,
            )?;
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&target)?;
            std::io::copy(&mut entry, &mut file)?;
        }
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
    #[test]
    #[ignore = "Downloads the pinned offline Prism fork into target/verification/offline"]
    fn verified_offline_prism_bootstrap() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/verification/offline");
        fs::create_dir_all(&root).unwrap();
        let ctx = context(&root);
        ensure_package(&root, &OFFLINE_PRISM, &ctx).unwrap();
        let result = system::checked(
            std::process::Command::new(
                root.join(OFFLINE_PRISM.folder)
                    .join(OFFLINE_PRISM.executable),
            )
            .arg("--dir")
            .arg(root.join("profile"))
            .arg("--version"),
            "Checking offline Prism",
            &ctx,
        )
        .unwrap();
        assert!(result.contains("11.0.3"));
    }
    #[test]
    fn installed_tool_versions_are_checked() {
        assert!(compatible_tool(0, "git version 2.55.0.windows.1"));
        assert!(!compatible_tool(0, "git version 2.20.0"));
        assert!(compatible_tool(1, "git-lfs/3.8.0 (GitHub)"));
        assert!(compatible_tool(2, "PrismLauncher 11.1.0"));
        assert!(!compatible_tool(2, "PrismLauncher 8.4"));
    }
    #[test]
    #[ignore = "Checks this PC's installed Git, LFS and Java without downloading"]
    fn reuse_installed_tools_and_java() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let paths = AppPaths {
            executable: root.join("launcher.exe"),
            data_dir: root.to_path_buf(),
            instance_dir: root.join("prism-profile/instances/BeeWorld"),
            prism_root: root.join("prism-profile"),
            install_dir: root.join("installed"),
        };
        let ctx = context(root);
        ensure_tools(&paths, &ctx, &[0, 1, 2]).unwrap();
        assert!(
            !paths.tools_dir().exists(),
            "Installed Git/LFS should avoid downloads"
        );
        fs::create_dir_all(&paths.instance_dir).unwrap();
        fs::write(
            paths.instance_dir.join("mmc-pack.json"),
            r#"{"components":[{"uid":"net.minecraft","version":"1.21.1"}]}"#,
        )
        .unwrap();
        fs::write(paths.instance_dir.join("instance.cfg"), "[General]\n").unwrap();
        crate::java::prepare(&paths, &ctx).unwrap();
        let text = fs::read_to_string(paths.instance_dir.join("instance.cfg")).unwrap();
        assert_eq!(
            crate::workflow::ini_value(&text, "OverrideJavaLocation"),
            Some("true")
        );
        println!("{text}");
    }
    #[test]
    fn archive_traversal_is_rejected() {
        use zip::write::SimpleFileOptions;
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("bad.zip");
        let mut zip = zip::ZipWriter::new(File::create(&archive_path).unwrap());
        zip.start_file("../outside", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        zip.finish().unwrap();
        let unpack = temp.path().join("unpack");
        fs::create_dir(&unpack).unwrap();
        assert!(extract(&archive_path, &unpack, &context(temp.path())).is_err());
        assert!(!temp.path().join("outside").exists());
    }
    #[test]
    fn corrupted_download_is_never_published() {
        let temp = tempfile::tempdir().unwrap();
        let package = Package {
            name: "invalid",
            folder: "invalid",
            url: PACKAGES[1].url,
            sha256: "wrong",
            bytes: 1,
            executable: "tool.exe",
        };
        // Local ZIP validation is also covered without relying on network.
        let archive = temp.path().join("bad.zip");
        fs::write(&archive, "not a ZIP").unwrap();
        let target = temp.path().join(package.folder);
        fs::create_dir(&target).unwrap();
        assert!(extract(&archive, &target, &context(temp.path())).is_err());
        assert!(!target.join(package.executable).exists());
    }
    #[test]
    #[ignore = "Downloads 89 MB from pinned upstream releases into target/verification"]
    fn upstream_portable_bootstrap() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/verification/portable");
        let paths = AppPaths {
            executable: root.join("BeeWorldLauncher.exe"),
            data_dir: root.clone(),
            instance_dir: root.join("prism-profile/instances/BeeWorld"),
            prism_root: root.join("prism-profile"),
            install_dir: root.join("installed"),
        };
        let ctx = context(&root);
        fs::create_dir_all(paths.tools_dir()).unwrap();
        for package in PACKAGES {
            ensure_package(&paths.tools_dir(), package, &ctx).unwrap();
        }
        // Do not rely on installed Git or LFS being on PATH.
        let clean_path = system::windows_tool("").to_string_lossy().into_owned();
        let version = system::checked(
            std::process::Command::new(git_path(&paths))
                .env("PATH", &clean_path)
                .arg("--version"),
            "Isolated Git version",
            &ctx,
        )
        .unwrap();
        assert!(version.contains("2.55.0"));
        let version = system::checked(
            std::process::Command::new(lfs_path(&paths))
                .env("PATH", &clean_path)
                .arg("version"),
            "Isolated LFS version",
            &ctx,
        )
        .unwrap();
        assert!(version.contains("3.8.0"));
        let version = system::checked(
            std::process::Command::new(prism_path(&paths))
                .env("PATH", &clean_path)
                .arg("--dir")
                .arg(&paths.prism_root)
                .arg("--version"),
            "Isolated Prism version",
            &ctx,
        )
        .unwrap();
        println!("Prism version output: {version}");
        println!("Verified tools: {}", root.display());
    }
}
