use crate::app::Failure;
use serde::Deserialize;
use std::time::Duration;
pub const RELEASES_URL: &str = "https://github.com/EzYDark/BeeWorldLauncher/releases";
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    pub assets: Vec<Asset>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}
pub fn check() -> Result<Option<Release>, Failure> {
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .user_agent("BeeWorldLauncher/0.2.0")
        .build()
        .map_err(|e| Failure::plain(e.to_string()))?;
    let response = client
        .get("https://api.github.com/repos/EzYDark/BeeWorldLauncher/releases/latest")
        .send()
        .map_err(|e| Failure::plain(e.to_string()))?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let bytes = response
        .error_for_status()
        .and_then(|r| r.bytes())
        .map_err(|e| Failure::plain(e.to_string()))?;
    let release: Release = serde_json::from_slice(&bytes)?;
    if newer(&release.tag_name, env!("CARGO_PKG_VERSION")) {
        Ok(Some(release))
    } else {
        Ok(None)
    }
}
fn newer(candidate: &str, current: &str) -> bool {
    fn version(value: &str) -> Option<Vec<u64>> {
        let parts: Vec<_> = value
            .trim_start_matches('v')
            .split('.')
            .map(str::parse)
            .collect::<Result<_, _>>()
            .ok()?;
        if parts.len() == 3 { Some(parts) } else { None }
    }
    version(candidate)
        .zip(version(current))
        .is_some_and(|(a, b)| a > b)
}
pub fn download(
    paths: &crate::app::AppPaths,
    release: &Release,
    ctx: &crate::system::Context,
) -> Result<std::path::PathBuf, Failure> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    let asset = release
        .assets
        .iter()
        .find(|a| a.name == "BeeWorldLauncher.exe")
        .ok_or_else(|| Failure::plain("This release has no Windows launcher executable."))?;
    if !asset
        .browser_download_url
        .starts_with("https://github.com/EzYDark/BeeWorldLauncher/releases/download/")
        || asset.size > 100_000_000
    {
        return Err(Failure::plain("Unexpected launcher release asset."));
    }
    let digest = asset
        .digest
        .as_deref()
        .and_then(|s| s.strip_prefix("sha256:"))
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| {
            Failure::plain("The release has no SHA-256 checksum. Update was not downloaded.")
        })?;
    let directory = paths.data_dir.join("launcher-update");
    crate::system::no_links(&directory)?;
    std::fs::create_dir_all(&directory)?;
    let temporary = directory.join("download.tmp");
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| Failure::plain(e.to_string()))?;
    let mut response = client
        .get(&asset.browser_download_url)
        .send()
        .and_then(|r| r.error_for_status())
        .map_err(|e| Failure::plain(e.to_string()))?;
    let mut file = std::fs::File::create(&temporary)?;
    let mut hasher = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = vec![0; 65536];
    loop {
        ctx.check()?;
        let n = response.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > asset.size {
            return Err(Failure::plain("Launcher download exceeded expected size."));
        }
        file.write_all(&buffer[..n])?;
        hasher.update(&buffer[..n]);
    }
    file.sync_all()?;
    drop(file);
    if bytes != asset.size || format!("{:x}", hasher.finalize()) != digest {
        return Err(Failure::plain("Launcher download checksum failed."));
    }
    let ready = directory.join("BeeWorldLauncher.exe");
    std::fs::rename(temporary, &ready)?;
    Ok(ready)
}

pub fn schedule_replace(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<(), Failure> {
    use std::{os::windows::process::CommandExt, process::Stdio};
    crate::system::no_links(source)?;
    crate::system::no_links(destination)?;
    crate::system::powershell()
        .arg(replacement_script(source, destination, std::process::id()))
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}
fn replacement_script(source: &std::path::Path, destination: &std::path::Path, pid: u32) -> String {
    format!(
        r#"$ErrorActionPreference='Stop'; Wait-Process -Id {pid} -ErrorAction SilentlyContinue;
$source={}; $target={}; $staged=$target+'.update-new'; $backup=$target+'.update-old';
try {{
    Copy-Item -LiteralPath $source -Destination $staged -Force;
    $hash=[Security.Cryptography.SHA256]::Create(); try {{ $originalHash=[BitConverter]::ToString($hash.ComputeHash([IO.File]::ReadAllBytes($source))); $copiedHash=[BitConverter]::ToString($hash.ComputeHash([IO.File]::ReadAllBytes($staged))); if ($originalHash -ne $copiedHash) {{ throw 'Update copy checksum mismatch' }} }} finally {{ $hash.Dispose() }};
    if (Test-Path -LiteralPath $target) {{ [IO.File]::Replace($staged,$target,$backup,$true) }} else {{ [IO.File]::Move($staged,$target) }};
    'Update installed' | Set-Content -LiteralPath ($source+'.result');
}} catch {{ $_.Exception.Message | Set-Content -LiteralPath ($source+'.result'); exit 1 }}"#,
        crate::system::ps_literal(source),
        crate::system::ps_literal(destination)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replacement_waits_for_exit_and_keeps_previous_executable() {
        use std::{fs, os::windows::process::CommandExt, process::Stdio};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let source = root.join("new.exe");
        let destination = root.join("launcher.exe");
        fs::write(&source, "new-version").unwrap();
        fs::write(&destination, "old-version").unwrap();
        fs::write(root.join("world"), "precious").unwrap();
        let mut parent = crate::system::powershell()
            .arg("Start-Sleep -Seconds 30")
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut helper = crate::system::powershell()
            .arg(replacement_script(&source, &destination, parent.id()))
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(fs::read_to_string(&destination).unwrap(), "old-version");
        parent.kill().unwrap();
        parent.wait().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = helper.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "{}",
                    fs::read_to_string(root.join("new.exe.result")).unwrap_or_default()
                );
                break;
            }
            if std::time::Instant::now() > deadline {
                helper.kill().unwrap();
                helper.wait().unwrap();
                panic!("Update helper timed out");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(fs::read_to_string(&destination).unwrap(), "new-version");
        assert_eq!(
            fs::read_to_string(root.join("launcher.exe.update-old")).unwrap(),
            "old-version"
        );
        assert_eq!(fs::read_to_string(root.join("world")).unwrap(), "precious");
    }
    #[test]
    fn release_versions_are_numeric_and_prereleases_are_not_auto_selected() {
        assert!(newer("v0.10.0", "0.2.0"));
        assert!(!newer("v0.2.0", "0.2.0"));
        assert!(!newer("1.0.0-beta", "0.2.0"));
        assert!(!newer("nonsense", "0.2.0"));
    }
}
