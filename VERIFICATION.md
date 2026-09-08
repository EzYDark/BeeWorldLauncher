# Verification, 2026-09-08

Current source is version 0.2.1. Build output: `target/release/BeeWorldLauncher.exe`.

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
