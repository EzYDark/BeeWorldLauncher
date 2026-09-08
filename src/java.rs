use crate::{
    app::{AppPaths, Failure},
    dependencies,
    system::{self, Context},
    workflow::{ini_set, ini_value},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub fn prepare(paths: &AppPaths, ctx: &Context) -> Result<(), Failure> {
    let pack: serde_json::Value =
        serde_json::from_slice(&fs::read(paths.instance_dir.join("mmc-pack.json"))?)?;
    let version = pack["components"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["uid"] == "net.minecraft"))
        .and_then(|c| c["version"].as_str())
        .ok_or_else(|| Failure::plain("Minecraft version is missing."))?;
    let required = required_major(version);
    let config = paths.instance_dir.join("instance.cfg");
    let text = fs::read_to_string(&config)?;
    let mut candidates = Vec::new();
    if let Some(path) = ini_value(&text, "JavaPath").filter(|s| !s.is_empty()) {
        candidates.push(PathBuf::from(path));
    }
    if let Some(base) = std::env::var_os("JAVA_HOME") {
        candidates.push(PathBuf::from(base).join("bin/java.exe"));
    }
    candidates.extend(dependencies::on_path("java.exe"));
    for variable in ["ProgramFiles", "LOCALAPPDATA", "USERPROFILE"] {
        if let Some(base) = std::env::var_os(variable) {
            for folder in [
                "Java",
                "Eclipse Adoptium",
                "Microsoft",
                "Zulu",
                "Amazon Corretto",
                "Programs/Eclipse Adoptium",
                ".jdks",
                "scoop/apps/temurin21-jdk/current",
            ] {
                collect_java(&PathBuf::from(&base).join(folder), 3, &mut candidates);
            }
        }
    }
    collect_java(&paths.prism_root.join("java"), 5, &mut candidates);
    let mut chosen = None;
    if let Some(major) = required {
        let mut seen = std::collections::HashSet::new();
        for candidate in candidates {
            ctx.check()?;
            let candidate = if candidate.file_name().is_some_and(|n| n == "javaw.exe") {
                candidate.with_file_name("java.exe")
            } else {
                candidate
            };
            if !candidate.is_absolute() || !candidate.is_file() || !seen.insert(candidate.clone()) {
                continue;
            }
            if let Ok(result) = system::run_timeout(
                Command::new(&candidate).args(["-XshowSettings:properties", "-version"]),
                "Checking Java compatibility",
                ctx,
                Duration::from_secs(15),
            ) && result.success
                && compatible(&format!("{}\n{}", result.stdout, result.stderr), major)
            {
                chosen = Some(candidate);
                break;
            }
        }
    }
    ctx.check()?;
    fs::write(config, configure(&text, chosen.as_deref()))?;
    if let Some(path) = chosen {
        ctx.logger
            .info(format!("Using compatible Java: {}", path.display()));
    } else {
        ctx.logger.info("No verified compatible Java found. Prism will download the required runtime into the BeeWorld profile when launching.");
    }
    Ok(())
}
fn collect_java(root: &Path, depth: usize, result: &mut Vec<PathBuf>) {
    result.push(root.join("bin/java.exe"));
    if depth == 0 {
        return;
    }
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir())
                && system::no_links(&entry.path()).is_ok()
            {
                collect_java(&entry.path(), depth - 1, result);
            }
        }
    }
}
fn required_major(version: &str) -> Option<u32> {
    let (major, minor, patch) = dependencies::numeric_version(version)?;
    // Unknown future releases/snapshots are resolved by Prism's Mojang metadata.
    if major != 1 || minor > 21 || version.contains('-') {
        return None;
    }
    Some(if minor > 20 || minor == 20 && patch >= 5 {
        21
    } else if minor >= 18 {
        17
    } else if minor == 17 {
        16
    } else {
        8
    })
}
pub(crate) fn compatible(output: &str, required: u32) -> bool {
    let property = |key: &str| {
        output.lines().find_map(|line| {
            line.trim()
                .split_once('=')
                .filter(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim())
        })
    };
    let major = property("java.version")
        .and_then(dependencies::numeric_version)
        .map(|(a, b, _)| if a == 1 { b } else { a });
    major == Some(required) && property("os.arch").is_some_and(|a| a == "amd64" || a == "x86_64")
}
fn configure(text: &str, selected: Option<&Path>) -> String {
    let text = ini_set(
        text,
        "OverrideJavaLocation",
        if selected.is_some() { "true" } else { "false" },
    );
    ini_set(
        &text,
        "JavaPath",
        &selected
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default(),
    )
}
pub fn server_java(paths: &AppPaths, ctx: &Context) -> Result<PathBuf, Failure> {
    let mut candidates = dependencies::on_path("java.exe");
    if let Some(root) = std::env::var_os("JAVA_HOME") {
        candidates.insert(0, PathBuf::from(root).join("bin/java.exe"));
    }
    if let Some(root) = std::env::var_os("ProgramFiles") {
        for folder in [
            "Java",
            "Eclipse Adoptium",
            "Microsoft",
            "Zulu",
            "Amazon Corretto",
        ] {
            collect_java(&PathBuf::from(&root).join(folder), 3, &mut candidates);
        }
    }
    for candidate in candidates {
        if !candidate.is_file() {
            continue;
        }
        if let Ok(result) = system::run_timeout(
            Command::new(&candidate).args(["-XshowSettings:properties", "-version"]),
            "Checking server Java",
            ctx,
            Duration::from_secs(15),
        ) && result.success
            && compatible(&format!("{}\n{}", result.stdout, result.stderr), 21)
        {
            return Ok(candidate);
        }
        ctx.check()?;
    }
    fs::create_dir_all(paths.tools_dir())?;
    dependencies::ensure_package(&paths.tools_dir(), &dependencies::SERVER_JAVA, ctx)?;
    Ok(paths
        .tools_dir()
        .join(dependencies::SERVER_JAVA.folder)
        .join(dependencies::SERVER_JAVA.executable))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_wrong_major_and_architecture() {
        assert!(compatible("java.version = 21.0.12\nos.arch = amd64", 21));
        assert!(!compatible("java.version = 17.0.9\nos.arch = amd64", 21));
        assert!(!compatible("java.version = 21.0.12\nos.arch = x86", 21));
        assert_eq!(required_major("1.21.1"), Some(21));
        assert_eq!(required_major("1.20.4"), Some(17));
        assert_eq!(required_major("26.1"), None);
    }
    #[test]
    fn fallback_clears_incompatible_java_override() {
        let text = configure(
            "[General]\nOverrideJavaLocation=true\nJavaPath=C:/Java17/java.exe\n",
            None,
        );
        assert_eq!(ini_value(&text, "OverrideJavaLocation"), Some("false"));
        assert_eq!(ini_value(&text, "JavaPath"), Some(""));
    }
}
