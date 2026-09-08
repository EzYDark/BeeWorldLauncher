use crate::{
    app::{AppPaths, Failure},
    system,
};
use serde::{Deserialize, Serialize};
use std::{fs, time::Duration};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    Game,
    Server,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Revision {
    pub sha: String,
    pub label: String,
    pub date: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Versions {
    pub game: Option<Revision>,
    pub server: Option<Revision>,
    pub installed_game: Option<Revision>,
    pub installed_server: Option<Revision>,
}
impl Versions {
    pub fn read(paths: &AppPaths) -> Result<Self, Failure> {
        let file = paths.data_dir.join("versions.json");
        let mut settings: Self = match fs::read(file) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        // Version receipts travel with the published folder, including on rollback.
        for (folder, target) in [
            (paths.instance_dir.clone(), Target::Game),
            (paths.data_dir.join("server/active"), Target::Server),
        ] {
            match fs::read(folder.join(".beeworld-version.json")) {
                Ok(bytes) => settings.record_installed(target, serde_json::from_slice(&bytes)?)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(settings)
    }
    pub fn selected(&self, target: Target) -> Option<&Revision> {
        match target {
            Target::Game => self.game.as_ref(),
            Target::Server => self.server.as_ref(),
        }
    }
    pub fn installed(&self, target: Target) -> Option<&Revision> {
        match target {
            Target::Game => self.installed_game.as_ref(),
            Target::Server => self.installed_server.as_ref(),
        }
    }
    pub fn select(&mut self, target: Target, revision: Option<Revision>) -> Result<(), Failure> {
        if let Some(revision) = &revision {
            validate_sha(&revision.sha)?;
        }
        match target {
            Target::Game => self.game = revision,
            Target::Server => self.server = revision,
        }
        Ok(())
    }
    pub fn record_installed(&mut self, target: Target, revision: Revision) -> Result<(), Failure> {
        validate_sha(&revision.sha)?;
        match target {
            Target::Game => self.installed_game = Some(revision),
            Target::Server => self.installed_server = Some(revision),
        }
        Ok(())
    }
    pub fn save(&self, paths: &AppPaths) -> Result<(), Failure> {
        system::no_links(&paths.data_dir)?;
        fs::create_dir_all(&paths.data_dir)?;
        // A truncated settings file must never silently select another version.
        let temporary = paths.data_dir.join("versions.json.new");
        fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temporary, paths.data_dir.join("versions.json"))?;
        Ok(())
    }
}
pub fn validate_sha(sha: &str) -> Result<(), Failure> {
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Failure::plain(
            "Invalid version identifier. No files were changed.",
        ));
    }
    Ok(())
}
pub fn catalogue() -> Result<Vec<Revision>, Failure> {
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .user_agent("BeeWorldLauncher/0.2.0")
        .build()
        .map_err(|e| Failure::plain(e.to_string()))?;
    let bytes = client
        .get("https://api.github.com/repos/EzYDark/BeeWorld/tags?per_page=100")
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| Failure::plain(format!("Could not check BeeWorld versions: {e}")))?;
    let tags: Vec<serde_json::Value> = serde_json::from_slice(&bytes)?;
    tags.iter()
        .map(|tag| {
            let sha = tag["commit"]["sha"]
                .as_str()
                .ok_or_else(|| Failure::plain("Tag is missing its commit."))?;
            validate_sha(sha)?;
            let label: String = tag["name"]
                .as_str()
                .unwrap_or("Unnamed version")
                .chars()
                .filter(|c| !c.is_control())
                .take(80)
                .collect();
            Ok(Revision {
                sha: sha.into(),
                label,
                date: String::new(),
            })
        })
        .collect()
}
pub fn latest() -> Result<Revision, Failure> {
    let client = reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .user_agent("BeeWorldLauncher/0.2.0")
        .build()
        .map_err(|e| Failure::plain(e.to_string()))?;
    let bytes = client
        .get("https://api.github.com/repos/EzYDark/BeeWorld/commits?sha=master&per_page=1")
        .send()
        .and_then(|r| r.error_for_status())
        .and_then(|r| r.bytes())
        .map_err(|e| Failure::plain(format!("Could not check for updates: {e}")))?;
    parse_catalogue(&bytes)?
        .into_iter()
        .next()
        .ok_or_else(|| Failure::plain("No pack revisions found."))
}

fn parse_catalogue(bytes: &[u8]) -> Result<Vec<Revision>, Failure> {
    let rows: Vec<serde_json::Value> = serde_json::from_slice(bytes)?;
    rows.iter()
        .map(|row| {
            let sha = row["sha"]
                .as_str()
                .ok_or_else(|| Failure::plain("Version is missing its identifier."))?;
            validate_sha(sha)?;
            let date = row["commit"]["committer"]["date"]
                .as_str()
                .unwrap_or("")
                .to_owned();
            let message = row["commit"]["message"]
                .as_str()
                .unwrap_or("Repository revision")
                .lines()
                .next()
                .unwrap_or("");
            let message: String = message
                .chars()
                .filter(|c| !c.is_control())
                .take(60)
                .collect();
            Ok(Revision {
                sha: sha.to_owned(),
                label: format!(
                    "{} · {} · {}",
                    date.get(..10).unwrap_or("Unknown date"),
                    &sha[..7],
                    message
                ),
                date,
            })
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    fn revision(ch: char) -> Revision {
        Revision {
            sha: ch.to_string().repeat(40),
            label: "test".into(),
            date: "2026-09-08".into(),
        }
    }
    #[test]
    fn game_and_server_selections_do_not_install_or_change_each_other() {
        let mut settings = Versions::default();
        settings.select(Target::Game, Some(revision('a'))).unwrap();
        settings
            .select(Target::Server, Some(revision('b')))
            .unwrap();
        assert_eq!(settings.selected(Target::Game).unwrap().sha, "a".repeat(40));
        assert_eq!(
            settings.selected(Target::Server).unwrap().sha,
            "b".repeat(40)
        );
        assert!(settings.installed(Target::Game).is_none());
        assert!(settings.installed(Target::Server).is_none());
        settings
            .record_installed(Target::Server, revision('c'))
            .unwrap();
        assert_eq!(
            settings.selected(Target::Server).unwrap().sha,
            "b".repeat(40)
        );
    }
    #[test]
    fn version_identifiers_cannot_be_git_options_or_paths() {
        for invalid in ["--upload-pack=evil", "../world", "master", "", "abcd"] {
            assert!(validate_sha(invalid).is_err());
        }
    }
    #[test]
    fn remote_labels_cannot_contain_terminal_control_sequences() {
        let bytes = format!(
            r#"[{{"sha":"{}","commit":{{"message":"hello\u001b[31m\nmore","committer":{{"date":"2026-09-08T00:00:00Z"}}}}}}]"#,
            "a".repeat(40)
        );
        let rows = parse_catalogue(bytes.as_bytes()).unwrap();
        assert!(!rows[0].label.contains('\u{1b}'));
        assert!(!rows[0].label.contains("more"));
    }
}
