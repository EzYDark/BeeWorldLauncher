use crate::{
    app::{AppPaths, Failure},
    system::{self, Context},
};
use serde::{Deserialize, Serialize};
use std::os::windows::process::CommandExt;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

#[derive(Serialize, Deserialize)]
pub struct Installation {
    pub data_dir: PathBuf,
    pub instance_dir: PathBuf,
}
pub fn read_record(directory: &Path) -> Result<Installation, Failure> {
    Ok(serde_json::from_slice(&fs::read(
        directory.join("installation.json"),
    )?)?)
}
pub fn install(paths: &AppPaths, ctx: &Context) -> Result<(), Failure> {
    system::no_links(&paths.install_dir)?;
    let _lock = system::lock(&paths.install_dir)?;
    if paths.install_dir.join("uninstall-pending").exists() {
        return Err(Failure::plain(
            "Close the previously uninstalled launcher before installing again.",
        ));
    }
    let migrated = crate::migration::copy_to_permanent(paths, ctx)?;
    let paths = &migrated;
    let executable = paths.install_dir.join("BeeWorldLauncher.exe");
    if !same_file(&paths.executable, &executable) {
        fs::copy(&paths.executable, &executable)?;
    }
    let script = shortcut_script(&executable, &paths.install_dir, false);
    system::checked(
        system::powershell().arg(script),
        "Creating launcher shortcuts",
        ctx,
    )?;
    fs::write(
        paths.install_dir.join("installation.json"),
        serde_json::to_vec_pretty(&Installation {
            data_dir: paths.data_dir.clone(),
            instance_dir: paths.instance_dir.clone(),
        })?,
    )?;
    ctx.logger
        .info("Installed. Game data is in the permanent Windows app-data folder.");
    Ok(())
}
fn same_file(a: &Path, b: &Path) -> bool {
    a.canonicalize()
        .ok()
        .zip(b.canonicalize().ok())
        .is_some_and(|(a, b)| a == b)
}
fn shortcut_script(executable: &Path, directory: &Path, remove: bool) -> String {
    registration_script(
        executable,
        directory,
        remove,
        "$targets=@((Join-Path $shell.SpecialFolders.Item('Desktop') 'BeeWorld Launcher.lnk'), (Join-Path ([Environment]::GetFolderPath('Programs')) 'BeeWorld Launcher.lnk'));",
        "HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\BeeWorldLauncher",
    )
}
fn registration_script(
    executable: &Path,
    directory: &Path,
    remove: bool,
    targets: &str,
    registry_key: &str,
) -> String {
    let operation = if remove {
        r#"foreach($path in $targets) {
            if(Test-Path -LiteralPath $path) {
                $link=$shell.CreateShortcut($path);
                if($link.TargetPath -eq $exe) { Remove-Item -LiteralPath $path -Force }
            }
        }
        if(Test-Path -LiteralPath $key) {
            if((Get-ItemProperty -LiteralPath $key).InstallLocation -eq $dir) {
                Remove-Item -LiteralPath $key
            }
        }"#
    } else {
        r#"foreach($path in $targets) {
            if(Test-Path -LiteralPath $path) {
                if($shell.CreateShortcut($path).TargetPath -ne $exe) {
                    throw "A shortcut with this name belongs to another application."
                }
            }
        }
        foreach($path in $targets) {
            $link=$shell.CreateShortcut($path);
            $link.TargetPath=$exe; $link.WorkingDirectory=$dir;
            $link.Description='Open the BeeWorld menu'; $link.Save()
        }
        New-Item -Path $key -Force | Out-Null;
        New-ItemProperty -LiteralPath $key -Name DisplayName -Value 'BeeWorld Launcher' -PropertyType String -Force | Out-Null;
        New-ItemProperty -LiteralPath $key -Name InstallLocation -Value $dir -PropertyType String -Force | Out-Null;
        New-ItemProperty -LiteralPath $key -Name UninstallString -Value ('"'+$exe+'" --uninstall') -PropertyType String -Force | Out-Null;
        New-ItemProperty -LiteralPath $key -Name NoModify -Value 1 -PropertyType DWord -Force | Out-Null;
        New-ItemProperty -LiteralPath $key -Name NoRepair -Value 1 -PropertyType DWord -Force | Out-Null;"#
    };
    format!(
        r#"$ErrorActionPreference='Stop';
        $exe={}; $dir={};
        $shell=New-Object -ComObject WScript.Shell;
        {targets}
        $key={};
        {operation}"#,
        system::ps_literal(executable),
        system::ps_literal(directory),
        system::ps_literal(Path::new(registry_key))
    )
}
pub fn uninstall(paths: &AppPaths, ctx: &Context) -> Result<(), Failure> {
    system::no_links(&paths.install_dir)?;
    // The marker distinguishes our optional installation from unrelated folders.
    let _record = read_record(&paths.install_dir)?;
    let _lock = system::lock(&paths.install_dir)?;
    let executable = paths.install_dir.join("BeeWorldLauncher.exe");
    system::checked(
        system::powershell().arg(shortcut_script(&executable, &paths.install_dir, true)),
        "Removing registered launcher shortcuts",
        ctx,
    )?;
    fs::write(paths.install_dir.join("uninstall-pending"), "1")?;
    if same_file(&paths.executable, &executable) {
        // Windows cannot unlink the running executable. Only these exact files
        // are removed by a hidden helper after this process exits. Data is never
        // recursively removed and the containing directory is retained.
        let script = deferred_removal_script(&paths.install_dir, std::process::id());
        system::powershell()
            .arg(script)
            .creation_flags(0x08000000)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        ctx.logger.info("Shortcuts removed. Close this launcher to finish deleting the installed executable. Data is kept.");
    } else {
        remove_installed_files(&paths.install_dir)?;
        ctx.logger.info(
            "Installed executable and shortcuts removed. Portable executable and data are kept.",
        );
    }
    Ok(())
}
pub fn remove_installed_files(directory: &Path) -> Result<(), Failure> {
    system::no_links(directory)?;
    read_record(directory)?;
    let exe = directory.join("BeeWorldLauncher.exe");
    if exe.is_file() {
        fs::remove_file(exe)?;
    }
    fs::remove_file(directory.join("installation.json"))?;
    let pending = directory.join("uninstall-pending");
    if pending.exists() {
        fs::remove_file(pending)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removal_keeps_game_data_and_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("BeeWorldLauncher.exe"), b"fixture").unwrap();
        fs::write(
            root.join("installation.json"),
            serde_json::to_vec(&Installation {
                data_dir: root.join("data"),
                instance_dir: root.join("instance"),
            })
            .unwrap(),
        )
        .unwrap();
        fs::create_dir(root.join("data")).unwrap();
        fs::write(root.join("data/world"), "precious").unwrap();
        fs::write(root.join("unrelated"), "keep").unwrap();
        remove_installed_files(root).unwrap();
        assert!(!root.join("BeeWorldLauncher.exe").exists());
        assert_eq!(
            fs::read_to_string(root.join("data/world")).unwrap(),
            "precious"
        );
        assert!(root.join("unrelated").exists());
    }
    #[test]
    fn refuses_to_uninstall_unmarked_folder() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join("BeeWorldLauncher.exe"), "unrelated").unwrap();
        assert!(remove_installed_files(temp.path()).is_err());
        assert!(temp.path().join("BeeWorldLauncher.exe").exists());
    }
    #[test]
    fn powershell_path_quotes_are_literal() {
        assert_eq!(
            system::ps_literal(Path::new(r"C:\User's Files\Bee.exe")),
            r"'C:\User''s Files\Bee.exe'"
        );
    }
}

#[cfg(test)]
mod windows_tests {
    use super::*;
    use crate::logger::Logger;
    use std::sync::{Arc, atomic::AtomicBool};
    #[test]
    fn actual_windows_shortcut_and_registration_roundtrip() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let exe = root.join("BeeWorldLauncher.exe");
        fs::write(&exe, "fixture").unwrap();
        let desktop = root.join("Desktop shortcut.lnk");
        let menu = root.join("Start menu shortcut.lnk");
        let targets = format!(
            "$targets=@({},{});",
            system::ps_literal(&desktop),
            system::ps_literal(&menu)
        );
        let key = format!(
            r"HKCU:\Software\BeeWorldLauncher-Test-{}",
            std::process::id()
        );
        let ctx = Context {
            logger: Arc::new(Logger::new(&root.join("logs"), None).unwrap()),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        system::checked(
            system::powershell().arg(registration_script(&exe, root, false, &targets, &key)),
            "Test registration",
            &ctx,
        )
        .unwrap();
        let inspect = format!(
            "$shell=New-Object -ComObject WScript.Shell; $shell.CreateShortcut({}).TargetPath; (Get-ItemProperty -LiteralPath {}).UninstallString",
            system::ps_literal(&desktop),
            system::ps_literal(Path::new(&key))
        );
        let observed = system::checked(
            system::powershell().arg(inspect),
            "Inspect test registration",
            &ctx,
        )
        .unwrap();
        let removed = system::checked(
            system::powershell().arg(registration_script(&exe, root, true, &targets, &key)),
            "Remove test registration",
            &ctx,
        );
        removed.unwrap();
        assert!(observed.contains(&exe.to_string_lossy().to_string()));
        assert!(observed.contains("--uninstall"));
        assert!(!desktop.exists());
        assert!(!menu.exists());
        let remaining = system::checked(
            system::powershell().arg(format!(
                "Test-Path -LiteralPath {}",
                system::ps_literal(Path::new(&key))
            )),
            "Check removed test registration",
            &ctx,
        )
        .unwrap();
        assert_eq!(remaining.trim(), "False");
    }
}

fn deferred_removal_script(directory: &Path, process_id: u32) -> String {
    format!(
        r#"$ErrorActionPreference='Stop';
            Wait-Process -Id {} -ErrorAction SilentlyContinue;
            $exe={}; $record={};
            for($attempt=0;$attempt -lt 30;$attempt++) {{
                try {{
                    if(Test-Path -LiteralPath $exe) {{ Remove-Item -LiteralPath $exe -Force }};
                    if(Test-Path -LiteralPath $record) {{ Remove-Item -LiteralPath $record -Force }};
                    Remove-Item -LiteralPath (Join-Path (Split-Path $exe) 'uninstall-pending') -ErrorAction SilentlyContinue;
                    exit 0
                }} catch {{ Start-Sleep -Seconds 1 }}
            }}
            exit 1"#,
        process_id,
        system::ps_literal(&directory.join("BeeWorldLauncher.exe")),
        system::ps_literal(&directory.join("installation.json"))
    )
}

#[cfg(test)]
mod deferred_tests {
    use super::*;
    #[test]
    fn deferred_removal_waits_for_process_exit_and_preserves_data() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("BeeWorldLauncher.exe"), "fixture").unwrap();
        fs::write(root.join("installation.json"), "{}").unwrap();
        fs::write(root.join("world"), "keep").unwrap();
        let mut parent = system::powershell()
            .arg("Start-Sleep -Seconds 30")
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut helper = system::powershell()
            .arg(deferred_removal_script(root, parent.id()))
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(700));
        let existed_before_exit = root.join("BeeWorldLauncher.exe").exists();
        parent.kill().unwrap();
        parent.wait().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let status = loop {
            if let Some(status) = helper.try_wait().unwrap() {
                break Some(status);
            }
            if std::time::Instant::now() > deadline {
                helper.kill().unwrap();
                helper.wait().unwrap();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        assert!(existed_before_exit);
        assert!(status.is_some_and(|s| s.success()));
        assert!(!root.join("BeeWorldLauncher.exe").exists());
        assert!(!root.join("installation.json").exists());
        assert_eq!(fs::read_to_string(root.join("world")).unwrap(), "keep");
    }
}
