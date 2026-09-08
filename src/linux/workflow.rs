use crate::{
    app::{AppPaths, Failure},
    dependencies,
};
use std::{path::Path, process::Command};
pub fn git(_: &AppPaths, directory: &Path) -> Result<Command, Failure> {
    let mut command = Command::new(dependencies::executable("git")?);
    command
        .current_dir(directory)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "credential.helper=",
            "-c",
            "filter.lfs.required=true",
            "-c",
            "filter.lfs.clean=git-lfs clean -- %f",
            "-c",
            "filter.lfs.smudge=git-lfs smudge -- %f",
            "-c",
            "filter.lfs.process=git-lfs filter-process",
        ]);
    Ok(command)
}
