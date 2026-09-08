# Verification, 2026-09-08

Current source is version 0.3.0. Windows build output: `target/release/BeeWorldLauncher.exe`. Linux release uses the `x86_64-unknown-linux-musl` target.

## Linux x64

- Tested in an isolated headless Ubuntu 24.04 WSL2 environment, with Git, Git LFS and OpenJDK 21. No desktop or Prism was installed.
- Both platforms pass Clippy with warnings denied. Windows: 51 regular tests. Linux: 7 regular tests plus the opt-in real-server installation/update test.
- The real-server test installs a fixed pack revision, reaches Done, sends commands, stops Java, then reinstalls and compares saved world data and server.properties byte for byte. It passed in 131 seconds. Its first attempt compared properties against the pre-launch text; Minecraft legitimately expands that file. The corrected test compares the post-shutdown file.
- The production static Linux executable installed published BeeWorld v5.0.0 through the normal release-only path. The installed receipt and selected version both report v5.0.0.
- CLI checks ran the server with no stdin console and sent commands through its mode-0600 Unix socket. SIGTERM, SIGINT and the separate stop command each saved the world and returned exit code 0. The control socket was removed after exit.
- The sample systemd unit passes systemd-analyze verification. Its shutdown signal behavior was exercised directly; WSL's test environment does not run systemd as PID 1.
- The Linux binary has no ELF interpreter dependency and is statically linked with musl. Actual runtime testing was on Ubuntu 24.04 x64; this does not claim every Linux distribution has been tested.
- Linux launcher updates are manual binary replacements. Pack updates retain explicit confirmation and EULA acceptance. System Git, Git LFS and Java are prerequisites; no package manager is invoked by the launcher.

To repeat the real server fixture on Linux:

```sh
cargo test linux_server_install_update_and_shutdown -- --ignored --nocapture
```

## Release-only pack selection

- Both game and server resolve downloads against published stable GitHub releases. Plain tags, drafts, prereleases and branch-head changes cannot be selected. Releases are ordered by publication date; an explicit older release remains pinned.
- Catalogue fixtures cover multiple releases, unpublished tags, draft/prerelease exclusion, an empty catalogue, missing tags and a moved tag. No-release results return an error rather than falling back to master.
- Fresh game staging does not check out branch files before selecting the release commit. The real Git/LFS fixture now starts from an explicitly selected revision.
- The live BeeWorld releases API returned zero releases during verification. Until the first stable release is published, real server install/update tests below intentionally stop before pack downloading. Their earlier startup/world-preservation results describe the 0.2.0 implementation.

## Passed

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`: 51 passed, 6 opt-in tests skipped by the default run. Release build passed.
- Real Git/LFS fixture: download binary objects, update, select an older revision without installing immediately, install it, return to current pack, refuse conflicting local edits, recover and restore. The test world survived every operation. Rerun after adding folder version receipts passed.
- Real BeeWorld server: master `1340bf3029fe7e4e0766c25175a299d4870e2119`, 146 common/server mods, Fabric 0.19.5, Minecraft 1.21.1, Java 21. Logged Done and exited after stdin stop. A second install preserved `level.dat`, a player-state fixture and `server.properties` byte for byte. No Prism was installed in server-only data.
- Offline Prism: pinned archive SHA-256 and executable version verified. Starting a disposable instance with no `accounts.json` displayed Create Offline Account. Entering BeeWorldTest created one account of type Offline, without any Microsoft account. The test proceeded to game preparation.
- Server process fixture: concurrent log capture, literal stdin commands, exclusive runtime lock and clean stop. UI fixtures check update consent, offline update-check failure, server-only setup, separate version selection and long-list scrolling.
- Launcher replacement fixture: helper waits for the running process to exit, copies and hashes the new executable, atomically replaces the old one and retains its backup. The world fixture is unchanged.
- Migration tests: worlds, account fixtures, game backups and server world survive removal of the source copy. Private server Java paths are rewritten for permanent storage. Runtime lock prevents copying an active server.
- Windows tests: real shortcut/uninstall registration roundtrip, delayed removal, data preservation, installation receipt handling. Deletion requires exact typed acknowledgement followed by a separate confirmation. Routine screens hide paths and logs.

All writes used launcher source or isolated fixtures. The user's live BeeWorld instance was not used as a test target.

## Limits

- Launcher 0.2.0 is published. Its uploaded executable's GitHub SHA-256 digest matched the local build. Replacement and version validation have local tests; an in-place update of a user's running installation was not performed.
- Offline account creation is verified; a complete modded client gameplay session is not.
- Server startup and update preservation are verified; multiplayer gameplay and every mod interaction are not. The server emitted pack/mod warnings during startup. Server updates replace pack configuration folders but retain the previous complete server as a backup.
- Default tests do not download dependencies. Private Java download fallback was not exercised because compatible Java 21 is installed on this PC.
- Native UI automation could not click the Prism toolbar because capture geometry was unavailable. The offline-account test succeeded using its launch dialog and keyboard input.

## Repeat opt-in checks

```powershell
cargo test upstream_portable_bootstrap -- --ignored --nocapture
cargo test reuse_installed_tools_and_java -- --ignored --nocapture
cargo test verified_offline_prism_bootstrap -- --ignored --nocapture
cargo test real_git_lfs_update_conflict_and_rollback -- --ignored --nocapture
cargo test real_server_install_and_start -- --ignored --nocapture
cargo test real_server_update_preserves_world_and_properties -- --ignored --nocapture
```

These use `target/verification` and temporary directories. Close Prism before the Git/LFS test. The real server fixture binds only to loopback port 25579. Do not run these against live player data.
