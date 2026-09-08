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
fn client() -> Result<reqwest::blocking::Client, Failure> {
    reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .user_agent(concat!("BeeWorldLauncher/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| Failure::plain(e.to_string()))
}
fn pages(
    client: &reqwest::blocking::Client,
    endpoint: &str,
) -> Result<Vec<serde_json::Value>, Failure> {
    let mut rows = Vec::new();
    for page in 1..=100 {
        let bytes = client
            .get(format!(
                "https://api.github.com/repos/EzYDark/BeeWorld/{endpoint}?per_page=100&page={page}"
            ))
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.bytes())
            .map_err(|e| Failure::plain(format!("Could not check BeeWorld releases: {e}")))?;
        let batch: Vec<serde_json::Value> = serde_json::from_slice(&bytes)?;
        let complete = batch.len() < 100;
        rows.extend(batch);
        if complete {
            return Ok(rows);
        }
    }
    Err(Failure::plain(
        "Too many release pages. No version was selected.",
    ))
}
pub fn catalogue() -> Result<Vec<Revision>, Failure> {
    let client = client()?;
    let releases = pages(&client, "releases")?;
    if releases.is_empty() {
        return Ok(Vec::new());
    }
    let tags = pages(&client, "tags")?;
    release_catalogue(&releases, &tags)
}
fn release_catalogue(
    releases: &[serde_json::Value],
    tags: &[serde_json::Value],
) -> Result<Vec<Revision>, Failure> {
    let mut versions = Vec::new();
    for release in releases {
        if release["draft"].as_bool() != Some(false)
            || release["prerelease"].as_bool() != Some(false)
        {
            continue;
        }
        let Some(date) = release["published_at"].as_str() else {
            continue;
        };
        let name = release["tag_name"]
            .as_str()
            .ok_or_else(|| Failure::plain("Release is missing its tag."))?;
        let sha = tags
            .iter()
            .find(|tag| tag["name"].as_str() == Some(name))
            .and_then(|tag| tag["commit"]["sha"].as_str())
            .ok_or_else(|| {
                Failure::plain("A published release tag is unavailable. Try again later.")
            })?;
        validate_sha(sha)?;
        versions.push(Revision {
            sha: sha.into(),
            label: name.chars().filter(|c| !c.is_control()).take(80).collect(),
            date: date.into(),
        });
    }
    versions.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.label.cmp(&b.label)));
    Ok(versions)
}
fn choose_release(versions: &[Revision], selected: Option<&Revision>) -> Result<Revision, Failure> {
    if let Some(selected) = selected {
        return versions.iter()
            .find(|v| v.sha == selected.sha && v.label == selected.label)
            .cloned()
            .ok_or_else(|| Failure::plain("Your selected version is not a published stable release. Choose a release in Versions."));
    }
    versions.first().cloned().ok_or_else(|| {
        Failure::plain(
            "No stable BeeWorld release is available yet. Your installed version is kept.",
        )
    })
}
#[cfg(windows)]
pub fn latest() -> Result<Revision, Failure> {
    choose_release(&catalogue()?, None)
}
pub fn desired(paths: &AppPaths, target: Target) -> Result<Revision, Failure> {
    choose_release(&catalogue()?, Versions::read(paths)?.selected(target))
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
    fn only_published_stable_releases_are_available_newest_first() {
        let tags = serde_json::json!([
            {"name":"v1", "commit":{"sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}},
            {"name":"v2", "commit":{"sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}},
            {"name":"draft", "commit":{"sha":"cccccccccccccccccccccccccccccccccccccccc"}},
            {"name":"beta", "commit":{"sha":"dddddddddddddddddddddddddddddddddddddddd"}},
            {"name":"tag-only", "commit":{"sha":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"}}
        ]);
        let releases = serde_json::json!([
            {"tag_name":"v1","draft":false,"prerelease":false,"published_at":"2026-09-01T00:00:00Z"},
            {"tag_name":"draft","draft":true,"prerelease":false,"published_at":null},
            {"tag_name":"beta","draft":false,"prerelease":true,"published_at":"2026-09-09T00:00:00Z"},
            {"tag_name":"v2","draft":false,"prerelease":false,"published_at":"2026-09-08T00:00:00Z"}
        ]);
        let catalogue =
            release_catalogue(releases.as_array().unwrap(), tags.as_array().unwrap()).unwrap();
        assert_eq!(
            catalogue
                .iter()
                .map(|v| v.label.as_str())
                .collect::<Vec<_>>(),
            ["v2", "v1"]
        );
        assert_eq!(choose_release(&catalogue, None).unwrap().label, "v2");
        assert_eq!(
            choose_release(&catalogue, Some(&catalogue[1]))
                .unwrap()
                .label,
            "v1"
        );
        assert!(choose_release(&catalogue, Some(&revision('e'))).is_err());
        let mut moved_tag = catalogue[1].clone();
        moved_tag.sha = "f".repeat(40);
        assert!(choose_release(&catalogue, Some(&moved_tag)).is_err());
        assert!(choose_release(&[], None).is_err());
        assert!(
            release_catalogue(&[], tags.as_array().unwrap())
                .unwrap()
                .is_empty()
        );
        assert!(release_catalogue(releases.as_array().unwrap(), &[]).is_err());
    }
}
