use std::{
    env,
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
};
pub const APP_NAME: &str = "BeeWorld Launcher";
pub const REPO_URL: &str = "https://github.com/EzYDark/BeeWorld.git";
pub const BRANCH: &str = "master";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    AddGame,
    LauncherUpdate,
    ServerOnly,
    ServerInstall,
    ServerStart,
    OfflineAccounts,
    OnlineAccounts,
    Portable,
    SkipSetup,
    ReinstallKeep,
    ReinstallClean,
    Setup,
    Play,
    Update,
    Restore,
    Recover,
    Install,
    Uninstall,
}
impl Action {
    pub fn title(self) -> &'static str {
        match self {
            Self::AddGame => "Add game client",
            Self::LauncherUpdate => "Update launcher",
            Self::ServerOnly => "Set up a server only",
            Self::ServerInstall => "Download or update server",
            Self::ServerStart => "Start server",
            Self::OfflineAccounts => "Use offline accounts",
            Self::OnlineAccounts => "Use Microsoft accounts",
            Self::Portable => "Use portable mode",
            Self::SkipSetup => "Use existing setup",
            Self::ReinstallKeep => "Reinstall - keep my data",
            Self::ReinstallClean => "Start over - delete my data",
            Self::Setup => "Download game",
            Self::Play => "Play",
            Self::Update => "Update BeeWorld",
            Self::Restore => "Restore backup",
            Self::Recover => "Repair interrupted update",
            Self::Install => "Install launcher shortcuts",
            Self::Uninstall => "Uninstall launcher",
        }
    }
    pub fn description(self, _paths: &AppPaths) -> String {
        match self {
            Self::AddGame => "Add the game menu beside your server. Choose Download game when ready.",
            Self::LauncherUpdate => "Download and verify the new launcher. Close this window to finish replacing it. Your game data is kept.",
            Self::ServerOnly => "Set up a headless server. No Prism or game client will be installed.",
            Self::ServerInstall => "Download the selected server version. Keep existing worlds. By continuing you accept the Minecraft EULA: https://www.minecraft.net/eula",
            Self::ServerStart => "Start your installed server. Use the console to send commands or stop it.",
            Self::OfflineAccounts => "Use the separate Prism offline-account fork. It asks for a player name without Microsoft sign-in. The next game download or launch downloads this fork.",
            Self::OnlineAccounts => "Use official Prism for Microsoft sign-in. Your accounts and game data are kept.",
            Self::Portable => "Run without installing the launcher. Keep your existing saved data.",
            Self::SkipSetup => "Continue with your existing game and settings.",
            Self::ReinstallKeep => "Reinstall the launcher. Keep your worlds and settings.",
            Self::ReinstallClean => "Delete BeeWorld worlds, saved sign-ins, settings and backups. This cannot be undone.",
            Self::Setup => "Download BeeWorld and what it needs to run. Internet required.",
            Self::Play => "Open BeeWorld. You may need to sign in through Prism.",
            Self::Update => "Update BeeWorld. Keep a backup of your current game and worlds.",
            Self::Restore => "Restore the previous game and worlds. Progress since that backup will be set aside.",
            Self::Recover => "Repair the interrupted update so you can play again.",
            Self::Install => "Keep your game in a permanent folder and add Desktop and Start Menu shortcuts.",
            Self::Uninstall => "Remove the launcher and shortcuts. Keep your game data.",
        }.into()
    }
}

#[derive(Default)]
pub struct Options {
    pub data_dir: Option<PathBuf>,
    pub help: bool,
    pub check: bool,
    pub selection: Option<Action>,
}
impl Options {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, Failure> {
        let mut result = Self::default();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--help" | "-h") => result.help = true,
                Some("--check") => result.check = true,
                Some("--install") => result.selection = Some(Action::Install),
                Some("--uninstall") => result.selection = Some(Action::Uninstall),
                Some("--update-only") => result.selection = Some(Action::Update),
                Some("--data-dir") => {
                    result.data_dir = Some(
                        args.next()
                            .map(PathBuf::from)
                            .ok_or_else(|| Failure::plain("--data-dir needs a path"))?,
                    )
                }
                _ => {
                    return Err(Failure::plain(format!(
                        "Unknown option: {}",
                        arg.to_string_lossy()
                    )));
                }
            }
        }
        Ok(result)
    }
}

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub executable: PathBuf,
    pub data_dir: PathBuf,
    pub instance_dir: PathBuf,
    pub prism_root: PathBuf,
    pub install_dir: PathBuf,
}
impl AppPaths {
    pub fn discover(data_override: Option<PathBuf>) -> Result<Self, Failure> {
        let executable = env::current_exe()?;
        let base = executable
            .parent()
            .ok_or_else(|| Failure::plain("Executable has no parent folder"))?;
        let install_dir = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| Failure::plain("LOCALAPPDATA is missing"))?
            .join("BeeWorldLauncher");
        let stored = preferred_installation(&install_dir);
        let explicit_data = data_override.is_some();
        let data_dir = std::path::absolute(
            data_override
                .or_else(|| stored.as_ref().map(|s| s.data_dir.clone()))
                .unwrap_or_else(|| base.join("BeeWorld-data")),
        )?;
        let prism_root = data_dir.join("prism-profile");
        let instance_dir = std::path::absolute(
            env::var_os("BEEWORLD_INSTANCE_PATH")
                .map(PathBuf::from)
                .or_else(|| {
                    if explicit_data {
                        None
                    } else {
                        stored.map(|s| s.instance_dir)
                    }
                })
                .unwrap_or_else(|| prism_root.join("instances").join("BeeWorld")),
        )?;
        validate_instance_path(&instance_dir, &data_dir)?;
        Ok(Self {
            executable,
            data_dir,
            instance_dir,
            prism_root,
            install_dir,
        })
    }
    pub fn permanent(&self) -> Self {
        let data_dir = self.install_dir.join("data");
        Self {
            data_dir: data_dir.clone(),
            prism_root: data_dir.join("prism-profile"),
            instance_dir: data_dir.join("prism-profile/instances/BeeWorld"),
            ..self.clone()
        }
    }
    pub fn onboarding_complete(&self) -> bool {
        let Some(record) = std::fs::read(self.data_dir.join("launcher-choice.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        else {
            return false;
        };
        let mode = record["mode"].as_str().unwrap_or("");
        if mode == "installed" && !self.installed() {
            return false;
        }
        if (record["had_game"].as_bool() == Some(true)
            || self.data_dir.join("setup-complete").exists())
            && !self.instance_dir.join("mmc-pack.json").is_file()
        {
            return false;
        }
        matches!(mode, "installed" | "portable" | "portable-or-existing")
    }
    pub fn existing_setup(&self) -> bool {
        self.ready()
            || self.prism_root.join("prismlauncher.cfg").is_file()
            || self.instance_dir.join("instance.cfg").is_file()
    }
    pub fn installed(&self) -> bool {
        self.install_dir.join("BeeWorldLauncher.exe").is_file()
            && !self.install_dir.join("uninstall-pending").exists()
            && crate::install::read_record(&self.install_dir).is_ok()
    }
    pub fn tools_dir(&self) -> PathBuf {
        self.data_dir.join("tools")
    }
    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
    pub fn ready(&self) -> bool {
        self.data_dir.join("setup-complete").is_file()
            && self.instance_dir.join("mmc-pack.json").is_file()
    }
    pub fn summary(&self) -> String {
        format!(
            "Data: {}\nInstance: {}\nGame setup: {}\nPrism: {}\nPack: {}\nInstalled shortcuts: {}\nRecovery: {}",
            self.data_dir.display(),
            self.instance_dir.display(),
            if self.ready() {
                "prepared"
            } else {
                "not prepared"
            },
            if crate::dependencies::prism_path(self).is_file() {
                "present"
            } else {
                "missing"
            },
            if self.instance_dir.join("mmc-pack.json").is_file() {
                "present"
            } else {
                "missing"
            },
            if self.installed() {
                "registered"
            } else {
                "not registered"
            },
            if crate::transaction::Transaction::new(&self.instance_dir).is_ok_and(|t| t.pending()) {
                "needed - choose Manage > Recover"
            } else {
                "not needed"
            }
        )
    }
}
pub fn validate_instance_path(instance: &Path, data: &Path) -> Result<(), Failure> {
    if instance.file_name().is_none() || data.starts_with(instance) {
        return Err(Failure::plain(
            "The instance must be a dedicated folder, not a drive root or a parent of the launcher data.",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Failure {
    pub title: String,
    pub detail: String,
    pub next_step: String,
}
impl Failure {
    pub fn new(
        title: impl Into<String>,
        detail: impl Into<String>,
        next_step: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            detail: detail.into(),
            next_step: next_step.into(),
        }
    }
    pub fn plain(detail: impl Into<String>) -> Self {
        Self::new(
            "Action stopped",
            detail,
            "Return to the menu. Check the details before trying again.",
        )
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}\n{}", self.title, self.detail, self.next_step)
    }
}
impl std::error::Error for Failure {}
impl From<std::io::Error> for Failure {
    fn from(e: std::io::Error) -> Self {
        Self::plain(e.to_string())
    }
}
impl From<serde_json::Error> for Failure {
    fn from(e: serde_json::Error) -> Self {
        Self::plain(e.to_string())
    }
}

fn preferred_installation(install_dir: &Path) -> Option<crate::install::Installation> {
    let permanent = install_dir.join("data");
    if permanent.is_dir() {
        Some(crate::install::Installation {
            data_dir: permanent.clone(),
            instance_dir: permanent.join("prism-profile/instances/BeeWorld"),
        })
    } else {
        crate::install::read_record(install_dir).ok()
    }
}
#[cfg(test)]
mod path_tests {
    use super::*;
    #[test]
    fn permanent_data_wins_over_old_portable_record_and_survives_uninstall() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir(root.join("data")).unwrap();
        std::fs::write(
            root.join("installation.json"),
            r#"{"data_dir":"C:/Downloads/old", "instance_dir":"C:/Downloads/old/instance"}"#,
        )
        .unwrap();
        assert_eq!(
            preferred_installation(root).unwrap().data_dir,
            root.join("data")
        );
        std::fs::remove_file(root.join("installation.json")).unwrap();
        assert_eq!(
            preferred_installation(root).unwrap().data_dir,
            root.join("data")
        );
    }
}
