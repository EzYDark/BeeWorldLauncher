# BeeWorld Launcher

Windows launcher and Linux x64 headless server manager for the [BeeWorld pack](https://github.com/EzYDark/BeeWorld). Source and launcher releases belong in [EzYDark/BeeWorldLauncher](https://github.com/EzYDark/BeeWorldLauncher).

Linux setup and background service instructions: [LINUX.md](LINUX.md).

## Start here

Open `BeeWorldLauncher.exe`. Choose portable mode, install shortcuts, or server only. Every download, update and installation asks for confirmation.

- **Play:** download the game once, then open it through Prism.
- **Offline accounts:** choose Tools & settings > Use offline accounts. The next download or launch uses a separate, verified Prism offline-account fork that supports a player name without a Microsoft account. Microsoft mode uses official Prism.
- **Server:** choose server only during setup, or Server from the tools menu. Download the server, accept the linked Minecraft EULA, then start it. No Prism or client installation is needed for server-only use.
- **Console:** view live server logs, type commands and press Enter. Stop server saves and closes it. Page Up or the wheel pauses following the latest output; End resumes it. Stop the server before closing the launcher.
- **Versions:** game and server have separate selectors. Only published stable BeeWorld releases appear here. Latest stable release follows the newest publication date; selecting an older release pins it. Plain tags, drafts, prereleases and unreleased branch changes are excluded. Downloads use the release tag's exact commit and its Git LFS files, not the current branch. Selecting a version saves a preference; downloading it still needs confirmation.

Internet is needed for initial downloads. Offline account mode does not provide access to servers that require Microsoft authentication. Server authentication remains enabled by default.

## Updates and saved data

The launcher checks for game and launcher updates when opened. Before server start it checks the selected server version. An unavailable update check still lets you start the installed server. Updates never install just because a check found one.

Game updates stage a complete copy, including worlds, before switching folders. Conflicting local edits stop the update. Restore backup restores the game **and worlds from that backup's date**. An interrupted folder switch can be repaired from the tools menu. Backups are kept until you remove them and need extra disk space.

Server updates keep worlds, player files and `server.properties`, replace pack files, and retain the previous server folder as a backup. Custom changes inside pack configuration folders are replaced by the selected pack's defaults. The current server build supports Minecraft 1.21.1 and Java 21. Mod compatibility beyond startup needs gameplay testing.

Portable data normally lives in `BeeWorld-data` beside the executable. If permanent data already exists, both executable modes reuse `%LOCALAPPDATA%\BeeWorldLauncher\data`. `--data-dir` explicitly selects another folder. Existing Prism instances are not adopted automatically.

Installing creates Desktop and Start Menu shortcuts. It copies portable data into permanent storage and verifies the copied files. The source copy is kept. Uninstall removes the launcher and shortcuts but keeps saved data. Start over deletes only the managed game profile after exact typed confirmation with `DELETE MY DATA`; server data and downloaded tools are kept.

## Controls

Click a choice, or use Tab/arrows and Enter. Escape goes back. Paths and technical information live under Help & folders. Operation output appears only when Show details is opened. Cancel stops work before publication when possible.

## Build

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

The distributable is `target/release/BeeWorldLauncher.exe`. This is a Windows x64 Rust project. Downloads use pinned hashes. Suitable installed Git, Git LFS and Java are reused; missing tools use private copies.

Opt-in integration tests and current verification limits are documented in [VERIFICATION.md](VERIFICATION.md).

## Publish versions

For launcher updates, publish a stable GitHub Release with `BeeWorldLauncher.exe` for Windows and `BeeWorldLauncher-linux-x64.tar.gz` for Linux. The Linux archive must contain the regular executable file `beeworld-server`. The release's numeric version must exceed the running Cargo package version. Both platforms require confirmation and verify GitHub's SHA-256 asset digest. Windows replaces the executable after exit; Linux replaces it atomically and uses the new version on the next start. Both keep a `.update-old` backup.

Publish a stable GitHub Release with its version tag in **EzYDark/BeeWorld**, independently of launcher releases. A tag alone is not enough. No attached ZIP is required: the launcher installs the repository snapshot at that release tag, including Git LFS files. Until a stable release exists, new game/server downloads and updates stop; an already installed version can still be started. Saved selections must still match a published release's tag and commit before downloading.

## Optional command line

```powershell
.\BeeWorldLauncher.exe --check
.\BeeWorldLauncher.exe --data-dir "D:\Games\BeeWorld-data"
.\BeeWorldLauncher.exe --install
.\BeeWorldLauncher.exe --uninstall
.\BeeWorldLauncher.exe --update-only
```

Action flags open confirmation screens. `--check` reports paths and local file presence without downloads. It does not prove the game can run.

`BEEWORLD_INSTANCE_PATH` opts an existing Git instance into management. Test with a copy first. Installation copies an external instance into permanent storage; remove the override afterward to use that copy.

## Upstream

[Official Prism](https://prismlauncher.org), [Prism offline-account fork](https://github.com/Diegiwg/PrismLauncher-Cracked), [Fabric](https://fabricmc.net), [Minecraft EULA](https://www.minecraft.net/eula).

Launcher source is licensed under [AGPL-3.0](LICENSE). Bundled downloads retain their upstream licenses.
