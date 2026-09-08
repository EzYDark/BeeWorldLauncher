# BeeWorld server on Linux

Linux x64 / amd64, with no desktop, Prism or game client required.

## First start

Install the required tools once. On Ubuntu 24.04:

```sh
sudo apt update
sudo apt install git git-lfs openjdk-21-jre-headless
```

Other distributions need Git, Git LFS and Java 21 x64 from their package manager. The launcher checks these tools; it does not install system packages.

Extract the Linux release archive and run:

```sh
./beeworld-server install
./beeworld-server start
```

Installation asks for confirmation and Minecraft EULA acceptance. Only published stable BeeWorld releases are available. Type Minecraft commands in the console. Type `stop` or press Ctrl+C to save and close.

Running `./beeworld-server` without arguments opens a numbered menu.

## Versions and updates

```sh
./beeworld-server versions
./beeworld-server install --version v5.0.0
./beeworld-server update --version latest
```

A named release stays selected. Latest follows the newest stable publication. Starting checks updates but never installs them. A failed check still lets you start the installed server.

Stop the server before updating. Worlds and server.properties are kept; pack configuration is replaced. A complete previous-server backup stays under `server/backup-*`.

The menu and server startup check for launcher updates too. Choose **Update launcher**, or run:

```sh
./beeworld-server update-launcher
```

Confirm the update when asked. For an explicitly approved unattended update, add `--yes`. The launcher verifies GitHub's SHA-256 checksum, replaces its executable atomically and keeps `beeworld-server.update-old` as a backup. Reopen the launcher to use the new version. An already running server is not interrupted.

The account running the update needs write access to the executable's folder. For the root-owned systemd installation below, use `sudo /usr/local/bin/beeworld-server update-launcher`, then restart the service when convenient. Server data is not changed by a launcher update.

Version 0.3.0 needs one manual upgrade to 0.3.1 to gain this feature.

## Data and commands from another session

Default data is `$XDG_DATA_HOME/beeworld-launcher`, or `~/.local/share/beeworld-launcher`. Use the same data folder for all commands:

```sh
./beeworld-server status --data-dir /srv/beeworld
./beeworld-server logs --data-dir /srv/beeworld
./beeworld-server command --data-dir /srv/beeworld --command "list"
./beeworld-server stop --data-dir /srv/beeworld
```

Commands use a local Unix socket accessible to the server's account. They go directly to Minecraft, never through a shell. Responses appear in the server log.

Minecraft uses its configured port, normally 25565. Firewall configuration remains yours. Server authentication remains enabled by default.

## Optional systemd service

Install the binary, create a dedicated account and install the pack:

```sh
sudo install -m 755 beeworld-server /usr/local/bin/beeworld-server
sudo useradd --system --home-dir /srv/beeworld --create-home --shell /usr/sbin/nologin beeworld
sudo -u beeworld /usr/local/bin/beeworld-server install --data-dir /srv/beeworld --yes --accept-eula
```

The last command explicitly accepts https://www.minecraft.net/eula.

Install the supplied service file:

```sh
sudo install -m 644 beeworld.service /etc/systemd/system/beeworld.service
sudo systemctl daemon-reload
sudo systemctl enable --now beeworld
```

View output, send a command or stop:

```sh
sudo journalctl -u beeworld -f
sudo -u beeworld beeworld-server command --data-dir /srv/beeworld --command "list"
sudo systemctl stop beeworld
```

Service stop sends Minecraft `stop` and waits for world saving. Updates remain separate:

```sh
sudo systemctl stop beeworld
sudo -u beeworld beeworld-server update --data-dir /srv/beeworld --yes --accept-eula
sudo systemctl start beeworld
```

## Build

Native build: `cargo build --release`.

Portable static build, with musl-tools installed:

```sh
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl --locked
```

The release archive uses the static binary, so it does not depend on the build machine's glibc version. Git and Java are still external executables.
