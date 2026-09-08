use crate::{
    app::{Action, AppPaths, BRANCH, Failure, REPO_URL},
    dependencies, install,
    system::{self, Context},
    transaction::{self, Transaction},
};
use std::{
    ffi::OsString,
    fs,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub fn execute(action: Action, paths: &AppPaths, ctx: &Context) -> Result<String, Failure> {
    // This function is called only after the TUI confirmation. Even log and lock
    // files are delayed until then.
    let _data_lock = system::lock(&paths.data_dir)?;
    match action {
        Action::AddGame => {
            let marker = paths.data_dir.join("server-only");
            if marker.exists() {
                fs::remove_file(marker)?;
            }
            return Ok("Game menu added. Your server is kept.".into());
        }
        Action::LauncherUpdate => {
            let release = crate::self_update::check()?
                .ok_or_else(|| Failure::plain("No new launcher version is available."))?;
            let downloaded = crate::self_update::download(paths, &release, ctx)?;
            crate::self_update::schedule_replace(&downloaded, &paths.executable)?;
            return Ok("Update verified. Close the launcher, then reopen it.".into());
        }
        Action::ServerOnly => {
            fs::write(paths.data_dir.join("server-only"), "1")?;
            save_launcher_choice(paths, "portable")?;
            return Ok("Server setup selected. Choose Download server.".into());
        }
        Action::ServerInstall => return crate::server::install(paths, ctx),
        Action::ServerStart => return Err(Failure::plain("Start the server through its console.")),
        Action::OfflineAccounts | Action::OnlineAccounts => {
            ensure_closed(paths, ctx)?;
            let marker = paths.data_dir.join("offline-mode");
            if action == Action::OfflineAccounts {
                fs::write(marker, "1")?;
            } else if marker.exists() {
                fs::remove_file(marker)?;
            }
            return Ok("Account mode saved. Choose Play when ready.".into());
        }
        Action::Portable | Action::SkipSetup => {
            save_launcher_choice(
                paths,
                if paths.installed() {
                    "installed"
                } else {
                    "portable"
                },
            )?;
            return Ok("Ready. Choose Download game to get started.".into());
        }
        Action::ReinstallClean => {
            ensure_closed(paths, ctx)?;
            reset_managed_profile(paths)?;
            return Ok("Game data deleted. Choose how to set up BeeWorld again.".into());
        }
        Action::ReinstallKeep => {
            ensure_closed(paths, ctx)?;
            install::install(paths, ctx)?;
            save_launcher_choice(&paths.permanent(), "installed")?;
            return Ok("Launcher installed. Your data was kept.".into());
        }
        Action::Install => {
            ensure_closed(paths, ctx)?;
            install::install(paths, ctx)?;
            save_launcher_choice(&paths.permanent(), "installed")?;
            return Ok(
                "Installed. Your game data is now stored on this PC. The original copy is kept."
                    .into(),
            );
        }
        Action::Uninstall => {
            // Earlier launcher versions had no installation record. The user
            // explicitly confirmed removal of this fixed install location.
            if install::read_record(&paths.install_dir).is_err()
                && paths.install_dir.join("BeeWorldLauncher.exe").is_file()
            {
                system::no_links(&paths.install_dir)?;
                fs::write(
                    paths.install_dir.join("installation.json"),
                    serde_json::to_vec(&install::Installation {
                        data_dir: paths.data_dir.clone(),
                        instance_dir: paths.instance_dir.clone(),
                    })?,
                )?;
            }
            install::uninstall(paths, ctx)?;
            clear_launcher_choice(paths)?;
            return Ok(
                "Launcher removed. Close this window to finish. Your game data is kept.".into(),
            );
        }
        _ => {}
    }
    let tx = Transaction::new(&paths.instance_dir)?;
    let _instance_lock = system::lock(&tx.root)?;
    match action {
        Action::Setup => {
            ensure_closed(paths, ctx)?;
            let revision = crate::versions::desired(paths, crate::versions::Target::Game)?;
            dependencies::ensure(paths, ctx)?;
            update(paths, &tx, ctx, revision)?;
            prepare_profile(paths)?;
            fs::write(paths.data_dir.join("setup-complete"), "1\n")?;
            save_launcher_choice(
                paths,
                if paths.installed() {
                    "installed"
                } else {
                    "portable"
                },
            )?;
            Ok("BeeWorld is ready. Choose Play.".into())
        }
        Action::Play => {
            let versions = crate::versions::Versions::read(paths)?;
            if let Some(selected) = versions.selected(crate::versions::Target::Game)
                && versions
                    .installed(crate::versions::Target::Game)
                    .is_none_or(|installed| installed.sha != selected.sha)
            {
                return Err(Failure::plain(
                    "Your selected game version is not installed yet. Choose Update to install it, then Play.",
                ));
            }
            if tx.pending() {
                return Err(Failure::plain(
                    "Recover the interrupted update before playing.",
                ));
            }
            transaction::validate_pack(&paths.instance_dir)?;
            launch(paths, ctx)
        }
        Action::Update => {
            ensure_closed(paths, ctx)?;
            let revision = crate::versions::desired(paths, crate::versions::Target::Game)?;
            dependencies::ensure_tools(paths, ctx, &[0, 1])?;
            update(paths, &tx, ctx, revision)?;
            Ok("Updated. Your previous game is saved as a backup.".into())
        }
        Action::Restore => {
            ensure_closed(paths, ctx)?;
            tx.restore(ctx)?;
            Ok("Game and worlds restored. The replaced version is kept as a backup.".into())
        }
        Action::Recover => {
            ensure_closed(paths, ctx)?;
            tx.recover(ctx)?;
            Ok("Repair complete. Return to the menu.".into())
        }
        Action::AddGame
        | Action::LauncherUpdate
        | Action::ServerOnly
        | Action::ServerInstall
        | Action::ServerStart
        | Action::OfflineAccounts
        | Action::OnlineAccounts
        | Action::Install
        | Action::Uninstall
        | Action::Portable
        | Action::SkipSetup
        | Action::ReinstallKeep
        | Action::ReinstallClean => unreachable!(),
    }
}

fn save_launcher_choice(paths: &AppPaths, mode: &str) -> Result<(), Failure> {
    fs::write(
        paths.data_dir.join("launcher-choice.json"),
        serde_json::to_vec(
            &serde_json::json!({"version": 2, "mode": mode, "had_game": paths.instance_dir.join("mmc-pack.json").is_file()}),
        )?,
    )?;
    Ok(())
}
fn clear_launcher_choice(paths: &AppPaths) -> Result<(), Failure> {
    let marker = paths.data_dir.join("launcher-choice.json");
    if marker.exists() {
        fs::remove_file(marker)?;
    }
    Ok(())
}
fn reset_managed_profile(paths: &AppPaths) -> Result<(), Failure> {
    // A clean reinstall may only erase the fixed, launcher-owned profile.
    let expected = paths.data_dir.join("prism-profile");
    if paths.prism_root != expected || paths.instance_dir != expected.join("instances/BeeWorld") {
        return Err(Failure::plain(
            "Clean reinstall is unavailable for an external or custom instance. Use reinstall with data kept.",
        ));
    }
    if !paths.data_dir.join("setup-complete").is_file()
        && !paths.data_dir.join("launcher-choice.json").is_file()
    {
        return Err(Failure::plain(
            "This data folder has no launcher ownership record. It was not deleted. Use reinstall with data kept.",
        ));
    }
    system::no_links(&expected)?;
    if expected.exists() {
        system::remove_owned_tree(&expected, &paths.data_dir)?;
    }
    clear_launcher_choice(paths)?;
    let marker = paths.data_dir.join("setup-complete");
    if marker.exists() {
        fs::remove_file(marker)?;
    }
    Ok(())
}

fn require_git(paths: &AppPaths) -> Result<(), Failure> {
    if !dependencies::git_path(paths).is_file() || !dependencies::lfs_path(paths).is_file() {
        return Err(Failure::plain(
            "Portable update tools are missing. Choose Prepare portable tools and BeeWorld.",
        ));
    }
    Ok(())
}
pub(crate) fn git(paths: &AppPaths, directory: &Path) -> Result<Command, Failure> {
    require_git(paths)?;
    let mut command = Command::new(dependencies::git_path(paths));
    let mut search = vec![
        dependencies::git_path(paths)
            .parent()
            .unwrap()
            .to_path_buf(),
        paths
            .tools_dir()
            .join(dependencies::PACKAGES[0].folder)
            .join("cmd"),
        paths
            .tools_dir()
            .join(dependencies::PACKAGES[0].folder)
            .join("mingw64/bin"),
        paths
            .tools_dir()
            .join(dependencies::PACKAGES[0].folder)
            .join("usr/bin"),
        dependencies::lfs_path(paths)
            .parent()
            .unwrap()
            .to_path_buf(),
    ];
    if let Some(path) = std::env::var_os("PATH") {
        search.extend(std::env::split_paths(&path));
    }
    let lfs = dependencies::lfs_path(paths)
        .to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''");
    if directory.join(".git").is_dir() {
        command
            .arg("--git-dir")
            .arg(directory.join(".git"))
            .arg("--work-tree")
            .arg(directory);
    }
    command
        .current_dir(directory)
        .env(
            "PATH",
            std::env::join_paths(search).map_err(|e| Failure::plain(e.to_string()))?,
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args([
            "-c",
            "core.hooksPath=NUL",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "credential.helper=",
            "-c",
            "filter.lfs.required=true",
        ])
        .arg("-c")
        .arg(format!("filter.lfs.clean='{lfs}' clean -- %f"))
        .arg("-c")
        .arg(format!("filter.lfs.smudge='{lfs}' smudge -- %f"))
        .arg("-c")
        .arg(format!("filter.lfs.process='{lfs}' filter-process"));
    Ok(command)
}
pub fn normalize_repo_url(url: &str) -> String {
    url.trim()
        .to_ascii_lowercase()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_owned()
}
fn update(
    paths: &AppPaths,
    tx: &Transaction,
    ctx: &Context,
    revision: crate::versions::Revision,
) -> Result<(), Failure> {
    update_revision_from(paths, tx, ctx, REPO_URL, Some(revision))
}

#[cfg(test)]
fn update_from(
    paths: &AppPaths,
    tx: &Transaction,
    ctx: &Context,
    repository: &str,
) -> Result<(), Failure> {
    let selected = crate::versions::Versions::read(paths)?
        .selected(crate::versions::Target::Game)
        .cloned();
    update_revision_from(paths, tx, ctx, repository, selected)
}

fn update_revision_from(
    paths: &AppPaths,
    tx: &Transaction,
    ctx: &Context,
    repository: &str,
    selected: Option<crate::versions::Revision>,
) -> Result<(), Failure> {
    let mut versions = crate::versions::Versions::read(paths)?;
    if let Some(revision) = &selected {
        crate::versions::validate_sha(&revision.sha)?;
    }
    if tx.pending() {
        return Err(Failure::plain(
            "Choose Recover interrupted update before updating again.",
        ));
    }
    if paths.instance_dir.exists() {
        if !paths.instance_dir.join(".git").is_dir() {
            return Err(Failure::plain(
                "The existing instance is not a Git repository. It was not overwritten. Use an empty portable data folder or an explicit Git instance path.",
            ));
        }
        let origin = system::checked(
            git(paths, &paths.instance_dir)?.args(["remote", "get-url", "origin"]),
            "Checking repository address",
            ctx,
        )?;
        if normalize_repo_url(&origin) != normalize_repo_url(repository) {
            return Err(Failure::plain(
                "The instance points to a different repository. It was not changed.",
            ));
        }
        let branch = system::checked(
            git(paths, &paths.instance_dir)?.args(["branch", "--show-current"]),
            "Checking instance branch",
            ctx,
        )?;
        if branch.trim() != BRANCH && !branch.trim().is_empty() {
            return Err(Failure::plain(format!(
                "Expected branch {BRANCH}; found {:?}. No automatic branch switching.",
                branch.trim()
            )));
        }
    }
    let stage = tx.begin()?;
    if paths.instance_dir.exists() {
        ctx.logger
            .info("Copying the full instance, including worlds, before updating.");
        system::copy_tree(&paths.instance_dir, &stage, ctx)?;
        if selected.is_none() {
            let branch = system::checked(
                git(paths, &stage)?.args(["branch", "--show-current"]),
                "Checking selected version",
                ctx,
            )?;
            if branch.trim().is_empty() {
                system::checked(
                    git(paths, &stage)?.args(["fetch", "origin", BRANCH]),
                    "Checking current pack",
                    ctx,
                )?;
                system::checked(
                    git(paths, &stage)?.args(["checkout", BRANCH]),
                    "Selecting current pack",
                    ctx,
                )?;
            }
            system::checked(
                git(paths, &stage)?.args(["pull", "--ff-only", "origin", BRANCH]),
                "Updating the staged pack",
                ctx,
            )?;
        }
    } else {
        let mut command = git(paths, &tx.root)?;
        if selected.is_some() {
            command.args(["clone", "--no-checkout", repository]);
        } else {
            command.args(["clone", "--branch", BRANCH, "--single-branch", repository]);
        }
        system::checked(command.arg(&stage), "Downloading BeeWorld", ctx)?;
    }
    if let Some(revision) = &selected {
        system::checked(
            git(paths, &stage)?.args(["fetch", "origin", &revision.sha]),
            "Downloading selected version",
            ctx,
        )?;
        system::checked(
            git(paths, &stage)?.args(["checkout", "--detach", &revision.sha]),
            "Selecting pack version",
            ctx,
        )?;
    }
    let revision = system::checked(
        git(paths, &stage)?.args(["rev-parse", "HEAD"]),
        "Reading pack version",
        ctx,
    )?
    .trim()
    .to_owned();
    crate::versions::validate_sha(&revision)?;
    system::checked(
        git(paths, &stage)?.args(["lfs", "pull", "origin", &revision]),
        "Downloading staged mod files",
        ctx,
    )?;
    system::checked(
        git(paths, &stage)?.args(["lfs", "fsck"]),
        "Checking downloaded LFS objects",
        ctx,
    )?;
    let files = system::checked(
        git(paths, &stage)?.args(["lfs", "ls-files"]),
        "Checking materialized mod files",
        ctx,
    )?;
    if files
        .lines()
        .any(|line| line.split_whitespace().nth(1) == Some("-"))
    {
        return Err(Failure::plain(
            "Some Git LFS files are still pointers. Active pack was not changed.",
        ));
    }
    let cfg = stage.join("instance.cfg");
    if !cfg.exists() {
        fs::write(
            &cfg,
            "[General]\nConfigVersion=1.2\nInstanceType=OneSix\nManagedPack=false\nname=BeeWorld\n",
        )?;
    }
    // Never replace a valid player-selected Java executable. Repair only a stale
    // machine-specific path, and only in the staged copy.
    let text = fs::read_to_string(&cfg)?;
    if let Some(java) = ini_value(&text, "JavaPath")
        && !java.is_empty()
        && !Path::new(java).is_file()
    {
        let text = ini_set(&text, "OverrideJavaLocation", "false");
        fs::write(&cfg, ini_set(&text, "JavaPath", ""))?;
    }
    transaction::validate_pack(&stage)?;
    ensure_closed(paths, ctx)?;
    let installed = selected.unwrap_or(crate::versions::Revision {
        sha: revision,
        label: "Current pack".into(),
        date: String::new(),
    });
    fs::write(
        stage.join(".beeworld-version.json"),
        serde_json::to_vec(&installed)?,
    )?;
    tx.publish(ctx)?;
    versions.record_installed(crate::versions::Target::Game, installed)?;
    versions.save(paths)
}

pub fn ini_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let mut general = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            general = line == "[General]";
        }
        if general
            && let Some((name, value)) = line.split_once('=')
            && name.trim() == key
        {
            return Some(value.trim());
        }
    }
    None
}
pub fn ini_set(text: &str, key: &str, value: &str) -> String {
    let mut lines = Vec::new();
    let mut general = false;
    let mut found_section = false;
    let mut written = false;
    for line in text.lines() {
        if line.trim().starts_with('[') {
            if general && !written {
                lines.push(format!("{key}={value}"));
                written = true;
            }
            general = line.trim() == "[General]";
            found_section |= general;
        }
        if general
            && line
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == key)
        {
            if !written {
                lines.push(format!("{key}={value}"));
                written = true;
            }
        } else {
            lines.push(line.to_owned());
        }
    }
    if !written {
        if !found_section {
            lines.push("[General]".to_owned());
        }
        lines.push(format!("{key}={value}"));
    }
    format!("{}\n", lines.join("\n"))
}
fn prepare_profile(paths: &AppPaths) -> Result<(), Failure> {
    system::no_links(&paths.prism_root)?;
    fs::create_dir_all(&paths.prism_root)?;
    let config = paths.prism_root.join("prismlauncher.cfg");
    let mut text = if config.exists() {
        fs::read_to_string(&config)?
    } else {
        "[General]\n".to_owned()
    };
    // A private profile avoids touching the user's existing Prism preferences.
    let instance_parent = paths
        .instance_dir
        .parent()
        .ok_or_else(|| Failure::plain("Instance has no parent"))?;
    text = ini_set(
        &text,
        "InstanceDir",
        &instance_parent.to_string_lossy().replace('\\', "/"),
    );
    text = ini_set(
        &text,
        "JavaDir",
        &paths
            .prism_root
            .join("java")
            .to_string_lossy()
            .replace('\\', "/"),
    );
    text = ini_set(&text, "AutomaticJavaSwitch", "true");
    text = ini_set(&text, "AutomaticJavaDownload", "true");
    fs::write(config, text)?;
    Ok(())
}
pub fn launch_arguments(paths: &AppPaths) -> Result<Vec<OsString>, Failure> {
    let instance = paths
        .instance_dir
        .file_name()
        .ok_or_else(|| Failure::plain("Instance path has no folder name"))?;
    Ok(vec![
        "--dir".into(),
        paths.prism_root.as_os_str().to_owned(),
        "--launch".into(),
        instance.to_owned(),
        "--alive".into(),
    ])
}
fn launch(paths: &AppPaths, ctx: &Context) -> Result<String, Failure> {
    ensure_closed(paths, ctx)?;
    dependencies::ensure_tools(paths, ctx, &[2])?;
    let prism = dependencies::prism_path(paths);
    prepare_profile(paths)?;
    crate::java::prepare(paths, ctx)?;
    let live = paths.prism_root.join("live.check");
    if live.is_file() {
        fs::remove_file(&live)?;
    }
    ctx.check()?;
    let mut child = Command::new(prism)
        .args(launch_arguments(paths)?)
        .current_dir(&paths.prism_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // --alive is a Prism startup signal, not proof Minecraft started.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(exit) = child.try_wait()? {
            return Err(Failure::plain(format!(
                "Prism exited during startup with {exit}. Check the Prism logs in {}.",
                paths.prism_root.display()
            )));
        }
        if live.is_file() {
            return Ok("Prism opened. Continue there to start the game.".into());
        }
        if Instant::now() >= deadline || ctx.cancelled.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok("Prism has not confirmed it opened. Check its window.".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}
fn ensure_closed(paths: &AppPaths, ctx: &Context) -> Result<(), Failure> {
    let instance = system::ps_literal(&paths.instance_dir);
    let profile = system::ps_literal(&paths.prism_root);
    let script = format!(
        r#"$ErrorActionPreference='Stop';
        $instance={instance}; $profile={profile};
        $busy=@(Get-CimInstance Win32_Process -Filter "Name='prismlauncher.exe' OR Name='javaw.exe' OR Name='java.exe'" |
            Where-Object {{
                $_.Name -eq 'prismlauncher.exe' -or
                [string]::IsNullOrEmpty($_.CommandLine) -or
                $_.CommandLine.IndexOf($instance,[StringComparison]::OrdinalIgnoreCase) -ge 0 -or
                $_.CommandLine.IndexOf($profile,[StringComparison]::OrdinalIgnoreCase) -ge 0
            }});
        if($busy.Count -gt 0) {{ Write-Output 'Prism or Minecraft is still running. Close it before continuing.'; exit 9 }}"#
    );
    let result = system::run(
        system::powershell().arg(script),
        "Checking running Prism and Minecraft processes",
        ctx,
    )?;
    if !result.success {
        return Err(Failure::plain(format!(
            "Could not confirm the instance is closed. {} {}",
            result.stdout.trim(),
            result.stderr.trim()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_override_folder_and_private_profile_are_passed_to_prism() {
        let paths = AppPaths {
            executable: "C:/app/Bee.exe".into(),
            data_dir: "C:/app/data".into(),
            instance_dir: "D:/my packs/Changed name".into(),
            prism_root: "C:/app/data/prism-profile".into(),
            install_dir: "C:/local/BeeWorldLauncher".into(),
        };
        let args = launch_arguments(&paths).unwrap();
        assert_eq!(args[1], paths.prism_root.as_os_str());
        assert_eq!(args[3], "Changed name");
        assert!(!args.iter().any(|s| s == "BeeWorld 5.0.0"));
    }
    #[test]
    fn ini_edit_keeps_other_sections_and_replaces_only_general_key() {
        let input = "[General]\nJavaPath=old\nname=Bee\n[UI]\nJavaPath=keep\n";
        let output = ini_set(input, "JavaPath", "");
        assert_eq!(ini_value(&output, "JavaPath"), Some(""));
        assert!(output.contains("[UI]\nJavaPath=keep"));
        let output = ini_set(&output, "AutomaticJavaDownload", "true");
        assert_eq!(ini_value(&output, "AutomaticJavaDownload"), Some("true"));
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::logger::Logger;
    use std::sync::{Arc, atomic::AtomicBool};

    #[test]
    #[ignore = "Run upstream_portable_bootstrap first; uses its verified private Git/LFS"]
    fn real_git_lfs_update_conflict_and_rollback() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let data = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/verification/portable");
        let paths = AppPaths {
            executable: root.join("BeeWorldLauncher.exe"),
            data_dir: data,
            instance_dir: root.join("instances/Custom instance name"),
            prism_root: root.join("profile"),
            install_dir: root.join("installed"),
        };
        let ctx = Context {
            logger: Arc::new(Logger::new(&root.join("logs"), None).unwrap()),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let upstream = root.join("upstream");
        fs::create_dir(&upstream).unwrap();
        let run_git = |dir: &Path, args: &[&str]| {
            system::checked(git(&paths, dir).unwrap().args(args), "Fixture Git", &ctx).unwrap()
        };
        run_git(&upstream, &["init", "-b", "master"]);
        fs::create_dir_all(upstream.join("minecraft/mods")).unwrap();
        fs::write(
            upstream.join("mmc-pack.json"),
            r#"{"components":[{"uid":"net.minecraft","version":"1.21.1"}]}"#,
        )
        .unwrap();
        fs::write(upstream.join("instance.cfg"), "[General]\nname=BeeWorld\nJavaPath=C:/does-not-exist/java.exe\nOverrideJavaLocation=true\n").unwrap();
        fs::write(
            upstream.join(".gitattributes"),
            "minecraft/mods/*.jar filter=lfs diff=lfs merge=lfs -text\n",
        )
        .unwrap();
        fs::write(upstream.join(".gitignore"), "minecraft/saves/\n").unwrap();
        fs::write(
            upstream.join("minecraft/mods/fixture.jar"),
            b"first binary payload",
        )
        .unwrap();
        fs::write(upstream.join("config.txt"), "first config").unwrap();
        run_git(&upstream, &["add", "."]);
        run_git(
            &upstream,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "first",
            ],
        );
        let tx = Transaction::new(&paths.instance_dir).unwrap();
        let first_sha = run_git(&upstream, &["rev-parse", "HEAD"]).trim().to_owned();
        let mut initial_selection = crate::versions::Versions::read(&paths).unwrap();
        initial_selection
            .select(
                crate::versions::Target::Game,
                Some(crate::versions::Revision {
                    sha: first_sha.clone(),
                    label: "v1 fixture".into(),
                    date: String::new(),
                }),
            )
            .unwrap();
        initial_selection.save(&paths).unwrap();
        update_from(&paths, &tx, &ctx, &upstream.to_string_lossy()).unwrap();
        initial_selection
            .select(crate::versions::Target::Game, None)
            .unwrap();
        initial_selection.save(&paths).unwrap();
        assert_eq!(
            fs::read(paths.instance_dir.join("minecraft/mods/fixture.jar")).unwrap(),
            b"first binary payload"
        );
        assert_eq!(
            ini_value(
                &fs::read_to_string(paths.instance_dir.join("instance.cfg")).unwrap(),
                "OverrideJavaLocation"
            ),
            Some("false")
        );

        fs::create_dir_all(paths.instance_dir.join("minecraft/saves")).unwrap();
        fs::write(
            paths.instance_dir.join("minecraft/saves/player-world"),
            "precious world",
        )
        .unwrap();
        fs::write(
            upstream.join("minecraft/mods/fixture.jar"),
            b"second binary payload",
        )
        .unwrap();
        fs::write(upstream.join("config.txt"), "second config").unwrap();
        run_git(&upstream, &["add", "."]);
        run_git(
            &upstream,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "second",
            ],
        );
        update_from(&paths, &tx, &ctx, &upstream.to_string_lossy()).unwrap();
        assert_eq!(
            fs::read(paths.instance_dir.join("minecraft/mods/fixture.jar")).unwrap(),
            b"second binary payload"
        );
        assert_eq!(
            fs::read_to_string(paths.instance_dir.join("minecraft/saves/player-world")).unwrap(),
            "precious world"
        );

        let mut versions = crate::versions::Versions::read(&paths).unwrap();
        versions
            .select(
                crate::versions::Target::Game,
                Some(crate::versions::Revision {
                    sha: first_sha,
                    label: "v1 fixture".into(),
                    date: String::new(),
                }),
            )
            .unwrap();
        versions.save(&paths).unwrap();
        // Selecting a version changes only the preference until update is confirmed.
        assert_eq!(
            fs::read(paths.instance_dir.join("minecraft/mods/fixture.jar")).unwrap(),
            b"second binary payload"
        );
        update_from(&paths, &tx, &ctx, &upstream.to_string_lossy()).unwrap();
        assert_eq!(
            fs::read(paths.instance_dir.join("minecraft/mods/fixture.jar")).unwrap(),
            b"first binary payload"
        );
        assert_eq!(
            fs::read_to_string(paths.instance_dir.join("minecraft/saves/player-world")).unwrap(),
            "precious world"
        );
        versions
            .select(crate::versions::Target::Game, None)
            .unwrap();
        versions.save(&paths).unwrap();
        update_from(&paths, &tx, &ctx, &upstream.to_string_lossy()).unwrap();
        assert_eq!(
            fs::read(paths.instance_dir.join("minecraft/mods/fixture.jar")).unwrap(),
            b"second binary payload"
        );

        fs::write(paths.instance_dir.join("config.txt"), "player edit").unwrap();
        fs::write(upstream.join("config.txt"), "conflicting upstream edit").unwrap();
        run_git(&upstream, &["add", "."]);
        run_git(
            &upstream,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                "third",
            ],
        );
        assert!(update_from(&paths, &tx, &ctx, &upstream.to_string_lossy()).is_err());
        assert_eq!(
            fs::read_to_string(paths.instance_dir.join("config.txt")).unwrap(),
            "player edit"
        );
        assert_eq!(
            fs::read_to_string(paths.instance_dir.join("minecraft/saves/player-world")).unwrap(),
            "precious world"
        );
        tx.recover(&ctx).unwrap();
        tx.restore(&ctx).unwrap();
        assert_eq!(
            fs::read(paths.instance_dir.join("minecraft/mods/fixture.jar")).unwrap(),
            b"first binary payload"
        );
        assert_eq!(
            fs::read_to_string(paths.instance_dir.join("minecraft/saves/player-world")).unwrap(),
            "precious world"
        );
    }
}

#[cfg(test)]
mod onboarding_tests {
    use super::*;
    fn paths(root: &Path) -> AppPaths {
        AppPaths {
            executable: root.join("launcher.exe"),
            data_dir: root.join("data"),
            prism_root: root.join("data/prism-profile"),
            instance_dir: root.join("data/prism-profile/instances/BeeWorld"),
            install_dir: root.join("installed"),
        }
    }
    #[test]
    fn clean_reinstall_removes_managed_worlds_but_not_unrelated_files() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        fs::create_dir_all(&paths.instance_dir).unwrap();
        fs::write(paths.instance_dir.join("world"), "old world").unwrap();
        fs::write(paths.data_dir.join("setup-complete"), "1").unwrap();
        fs::write(paths.data_dir.join("unrelated"), "keep").unwrap();
        reset_managed_profile(&paths).unwrap();
        assert!(!paths.prism_root.exists());
        assert!(!paths.ready());
        assert_eq!(
            fs::read_to_string(paths.data_dir.join("unrelated")).unwrap(),
            "keep"
        );
    }
    #[test]
    fn clean_reinstall_refuses_external_instances_and_unowned_profiles() {
        let temp = tempfile::tempdir().unwrap();
        let mut paths = paths(temp.path());
        fs::create_dir_all(&paths.instance_dir).unwrap();
        assert!(reset_managed_profile(&paths).is_err());
        fs::write(paths.data_dir.join("setup-complete"), "1").unwrap();
        paths.instance_dir = temp.path().join("external");
        fs::create_dir_all(&paths.instance_dir).unwrap();
        fs::write(paths.instance_dir.join("world"), "keep").unwrap();
        assert!(reset_managed_profile(&paths).is_err());
        assert_eq!(
            fs::read_to_string(paths.instance_dir.join("world")).unwrap(),
            "keep"
        );
    }
    #[test]
    fn reset_clears_setup_choice_and_preserves_installation_state() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        fs::create_dir_all(&paths.instance_dir).unwrap();
        fs::write(paths.instance_dir.join("mmc-pack.json"), "{}").unwrap();
        fs::write(paths.data_dir.join("setup-complete"), "1").unwrap();
        save_launcher_choice(&paths, "portable").unwrap();
        reset_managed_profile(&paths).unwrap();
        assert!(!paths.onboarding_complete());
        assert!(!paths.ready());
        assert!(
            !paths.install_dir.exists(),
            "Reset must not install the launcher"
        );
        save_launcher_choice(&paths, "portable").unwrap();
        assert!(paths.onboarding_complete());
        assert!(!paths.installed());
    }
    #[test]
    fn choosing_portable_only_saves_preference() {
        let temp = tempfile::tempdir().unwrap();
        let paths = paths(temp.path());
        fs::create_dir_all(&paths.data_dir).unwrap();
        save_launcher_choice(&paths, "portable").unwrap();
        assert!(paths.onboarding_complete());
        assert!(!paths.tools_dir().exists());
        assert!(!paths.install_dir.exists());
        assert!(!paths.prism_root.exists());
    }
}
