use crate::{
    app::{AppPaths, Failure},
    dependencies,
    system::{self, Context},
};
use std::{path::PathBuf, process::Command};
pub fn server_java(_: &AppPaths, ctx: &Context) -> Result<PathBuf, Failure> {
    let java = match std::env::var_os("JAVA_HOME") {
        Some(root) => PathBuf::from(root).join("bin/java"),
        None => dependencies::executable("java")?,
    };
    let result = system::run(
        Command::new(&java).args(["-XshowSettings:properties", "-version"]),
        "Checking Java 21",
        ctx,
    )?;
    let text = format!("{}\n{}", result.stdout, result.stderr);
    let property = |key: &str| {
        text.lines()
            .filter_map(|l| l.trim().split_once('='))
            .find(|(k, _)| k.trim() == key)
            .map(|(_, v)| v.trim())
    };
    if !result.success
        || !property("java.version").is_some_and(|v| v == "21" || v.starts_with("21."))
        || !property("os.arch").is_some_and(|v| v == "amd64" || v == "x86_64")
    {
        return Err(Failure::plain(
            "Java 21 x64 is required. Install a headless Java 21 runtime or set JAVA_HOME.",
        ));
    }
    Ok(java.canonicalize()?)
}
