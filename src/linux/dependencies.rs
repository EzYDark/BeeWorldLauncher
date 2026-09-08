use crate::{
    app::{AppPaths, Failure},
    system::{self, Context},
};
use std::{path::PathBuf, process::Command};
pub fn executable(name: &str) -> Result<PathBuf, Failure> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|p| p.join(name))
        .find(|p| p.is_file())
        .ok_or_else(|| {
            Failure::plain(format!(
                "Install {name} using your Linux package manager, then retry."
            ))
        })
}
pub fn ensure_tools(_: &AppPaths, ctx: &Context, _: &[usize]) -> Result<(), Failure> {
    for name in ["git", "git-lfs"] {
        system::checked(
            Command::new(executable(name)?).arg("--version"),
            "Checking server tools",
            ctx,
        )?;
    }
    Ok(())
}
