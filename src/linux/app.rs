use std::{fmt, path::PathBuf};
pub const REPO_URL: &str = "https://github.com/EzYDark/BeeWorld.git";
#[derive(Clone, Debug)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub instance_dir: PathBuf,
}
impl AppPaths {
    pub fn new(data_dir: PathBuf) -> Result<Self, Failure> {
        let data_dir = std::path::absolute(data_dir)?;
        crate::system::no_links(&data_dir)?;
        Ok(Self {
            instance_dir: data_dir.join("unused-client"),
            data_dir,
        })
    }
}
#[derive(Debug)]
pub struct Failure(String);
impl Failure {
    pub fn plain(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }
    pub fn new(
        title: impl Into<String>,
        detail: impl Into<String>,
        next: impl Into<String>,
    ) -> Self {
        Self(format!(
            "{}: {}\n{}",
            title.into(),
            detail.into(),
            next.into()
        ))
    }
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
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
