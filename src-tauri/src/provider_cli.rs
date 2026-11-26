//! Automatic Claude/Codex CLI discovery shared by macOS and Windows.
//!
//! Discovery is based on the current process environment and current user's
//! home directory. No user-specific absolute path is embedded in the app.

use crate::Provider;
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

fn override_variable(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "CSM_CLAUDE_CLI",
        Provider::Codex => "CSM_CODEX_CLI",
    }
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

fn candidate_names_for(name: &str) -> Vec<OsString> {
    #[cfg(target_os = "windows")]
    {
        let mut extensions = vec![
            OsString::from("exe"),
            OsString::from("cmd"),
            OsString::from("bat"),
            OsString::from("ps1"),
        ];
        if let Some(pathext) =
            std::env::var_os("PATHEXT").and_then(|value| value.into_string().ok())
        {
            extensions.extend(
                pathext
                    .split(';')
                    .map(str::trim)
                    .map(|extension| extension.trim_start_matches('.'))
                    .filter(|extension| !extension.is_empty())
                    .map(OsString::from),
            );
        }
        let mut seen = HashSet::new();
        extensions
            .into_iter()
            .filter(|extension| seen.insert(extension.to_string_lossy().to_ascii_lowercase()))
            .map(|extension| {
                let mut candidate = OsString::from(name);
                candidate.push(".");
                candidate.push(extension);
                candidate
            })
            .collect()
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![OsString::from(name)]
    }
}

fn find_in_directories<I>(provider: Provider, directories: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    find_named_in_directories(provider.as_str(), directories)
}

fn find_named_in_directories<I>(name: &str, directories: I) -> Option<PathBuf>
where
    I: IntoIterator<Item = PathBuf>,
{
    let names = candidate_names_for(name);
    let mut seen = HashSet::new();
    for directory in directories {
        if directory.as_os_str().is_empty() || !seen.insert(directory.clone()) {
            continue;
        }
        for name in &names {
            let candidate = directory.join(name);
            if !executable_file(&candidate) {
                continue;
            }
            if candidate.is_absolute() {
                return Some(candidate);
            }
            if let Ok(current) = std::env::current_dir() {
                return Some(current.join(candidate));
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn nvm_version_key(path: &Path) -> [u64; 3] {
    let version = path
        .parent()
        .and_then(|directory| directory.file_name())
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    let mut numbers = version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse::<u64>().unwrap_or(0));
    [
        numbers.next().unwrap_or(0),
        numbers.next().unwrap_or(0),
        numbers.next().unwrap_or(0),
    ]
}

#[cfg(target_os = "macos")]
fn append_nvm_bin_directories(home: &Path, directories: &mut Vec<PathBuf>) {
    let mut version_bins = std::fs::read_dir(home.join(".nvm/versions/node"))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("bin"))
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    version_bins.sort_by(|left, right| {
        nvm_version_key(right)
            .cmp(&nvm_version_key(left))
            .then_with(|| right.cmp(left))
    });
    directories.extend(version_bins);
}

fn fallback_directories_for(
    provider: Provider,
    home: Option<&Path>,
    roaming_data: Option<&Path>,
    local_data: Option<&Path>,
) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = home {
        for relative in [
            ".local/bin",
            "bin",
            ".npm-global/bin",
            ".volta/bin",
            ".bun/bin",
            ".cargo/bin",
            ".asdf/shims",
            ".nodenv/shims",
            ".local/share/mise/shims",
        ] {
            directories.push(home.join(relative));
        }
        if provider == Provider::Claude {
            // Older Claude Code native installers place the launcher here.
            directories.push(home.join(".claude/local"));
        }
        #[cfg(target_os = "macos")]
        {
            directories.push(home.join("Library/pnpm"));
            append_nvm_bin_directories(home, &mut directories);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local) = local_data {
            directories.push(local.join("Programs/nodejs"));
        }
        if let Some(roaming) = roaming_data {
            directories.push(roaming.join("npm"));
        }
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(program_files) =
                std::env::var_os(variable).filter(|value| !value.is_empty())
            {
                directories.push(PathBuf::from(program_files).join("nodejs"));
            }
        }
        if let Some(local) = local_data {
            directories.push(local.join("pnpm"));
            directories.push(local.join("Microsoft/WinGet/Links"));
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (roaming_data, local_data);
    }
    #[cfg(target_os = "macos")]
    {
        directories.push(PathBuf::from("/opt/homebrew/bin"));
        directories.push(PathBuf::from("/usr/local/bin"));
    }
    directories
}

fn fallback_directories(provider: Provider) -> Vec<PathBuf> {
    let home = dirs::home_dir();
    let roaming = dirs::data_dir();
    let local = dirs::data_local_dir();
    fallback_directories_for(
        provider,
        home.as_deref(),
        roaming.as_deref(),
        local.as_deref(),
    )
}

pub(crate) fn resolve(provider: Provider) -> Result<PathBuf, String> {
    let name = provider.as_str();
    let variable = override_variable(provider);
    if let Some(configured) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(configured);
        if !path.is_absolute() {
            return Err(format!("{variable} 必须是 {name} 可执行文件的绝对路径"));
        }
        return if executable_file(&path) {
            Ok(path)
        } else {
            Err(format!(
                "{variable} 指向的 {name} CLI 不存在、不是文件或不可执行: {}",
                path.display()
            ))
        };
    }

    let mut directories = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    directories.extend(fallback_directories(provider));
    find_in_directories(provider, directories).ok_or_else(|| {
        format!(
            "未自动找到 {name} CLI。已检查当前 PATH 和当前用户常见安装目录；请安装并登录 CLI，或将 {variable} 设置为可执行文件绝对路径后重新启动应用"
        )
    })
}

fn prepend_node_to_path(node: &Path, current: Option<OsString>) -> Result<OsString, String> {
    let directory = node
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| "CSM_NODE_CLI 必须包含可执行文件名".to_string())?;
    let mut directories = vec![directory.to_path_buf()];
    if let Some(current) = current {
        directories.extend(std::env::split_paths(&current));
    }
    std::env::join_paths(directories).map_err(|error| format!("无法为 Node 构造恢复 PATH: {error}"))
}

fn node_fallback_directories() -> Vec<PathBuf> {
    let home = dirs::home_dir();
    let roaming = dirs::data_dir();
    let local = dirs::data_local_dir();
    let directories = fallback_directories_for(
        Provider::Codex,
        home.as_deref(),
        roaming.as_deref(),
        local.as_deref(),
    );
    #[cfg(target_os = "windows")]
    let mut directories = directories;
    #[cfg(target_os = "windows")]
    {
        for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(program_files) =
                std::env::var_os(variable).filter(|value| !value.is_empty())
            {
                directories.push(PathBuf::from(program_files).join("nodejs"));
            }
        }
        if let Some(roaming) = roaming {
            directories.push(roaming.join("nvm"));
        }
    }
    directories
}

/// Return a child PATH only when Node needs to be added. Native CLIs remain
/// usable without Node; npm launchers can use CSM_NODE_CLI for custom runtimes.
pub(crate) fn launch_path() -> Result<Option<OsString>, String> {
    let current = std::env::var_os("PATH");
    if let Some(configured) = std::env::var_os("CSM_NODE_CLI").filter(|value| !value.is_empty()) {
        let path = PathBuf::from(configured);
        if !path.is_absolute() {
            return Err("CSM_NODE_CLI 必须是 Node 可执行文件的绝对路径".into());
        }
        if !executable_file(&path) {
            return Err(format!(
                "CSM_NODE_CLI 指向的 Node 不存在、不是文件或不可执行: {}",
                path.display()
            ));
        }
        return prepend_node_to_path(&path, current).map(Some);
    }

    let path_directories = current
        .as_ref()
        .map(|path| std::env::split_paths(path).collect::<Vec<_>>())
        .unwrap_or_default();
    if find_named_in_directories("node", path_directories).is_some() {
        return Ok(None);
    }
    if let Some(node) = find_named_in_directories("node", node_fallback_directories()) {
        return prepend_node_to_path(&node, current).map(Some);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "csm-provider-cli-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self { root }
        }

        fn executable(&self, directory: &Path, provider: Provider) -> PathBuf {
            std::fs::create_dir_all(directory).unwrap();
            let path = directory.join(
                candidate_names_for(provider.as_str())
                    .into_iter()
                    .next()
                    .unwrap(),
            );
            std::fs::write(&path, b"test launcher\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            path
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn discovers_cli_in_an_arbitrary_user_directory() {
        let fixture = Fixture::new("arbitrary-directory");
        let custom = fixture.root.join("自定义 CLI location/bin");
        let expected = fixture.executable(&custom, Provider::Claude);
        let resolved = find_in_directories(Provider::Claude, vec![custom]).unwrap();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn path_order_selects_the_first_matching_provider() {
        let fixture = Fixture::new("path-order");
        let first = fixture.root.join("first");
        let second = fixture.root.join("second");
        let expected = fixture.executable(&first, Provider::Codex);
        fixture.executable(&second, Provider::Codex);
        let resolved = find_in_directories(Provider::Codex, vec![first, second]).unwrap();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn discovery_rejects_missing_files_and_does_not_swap_providers() {
        let fixture = Fixture::new("provider-identity");
        let custom = fixture.root.join("bin");
        fixture.executable(&custom, Provider::Claude);
        assert!(find_in_directories(Provider::Codex, vec![custom.clone()]).is_none());
        assert!(
            find_in_directories(Provider::Claude, vec![fixture.root.join("missing")]).is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn discovery_rejects_non_executable_files() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new("not-executable");
        let custom = fixture.root.join("bin");
        let path = fixture.executable(&custom, Provider::Claude);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(find_in_directories(Provider::Claude, vec![custom]).is_none());
    }

    #[test]
    fn node_runtime_directory_is_prepended_without_discarding_path() {
        let fixture = Fixture::new("node-path");
        let first = fixture.root.join("first");
        let second = fixture.root.join("second");
        let current = std::env::join_paths([&first, &second]).unwrap();
        let node_directory = fixture.root.join("Node Runtime 中文");
        let node = node_directory.join(if cfg!(target_os = "windows") {
            "node.exe"
        } else {
            "node"
        });
        let augmented = prepend_node_to_path(&node, Some(current)).unwrap();
        let directories = std::env::split_paths(&augmented).collect::<Vec<_>>();
        assert_eq!(directories, vec![node_directory, first, second]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn nvm_fallback_prefers_the_newest_numeric_version() {
        let fixture = Fixture::new("nvm-order");
        let home = fixture.root.join("home");
        let mut expected = Vec::new();
        for version in ["v22.11.0", "v20.10.0", "v9.11.0"] {
            let bin = home.join(".nvm/versions/node").join(version).join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            expected.push(bin);
        }
        let mut directories = Vec::new();
        append_nvm_bin_directories(&home, &mut directories);
        assert_eq!(directories, expected);
    }

    #[test]
    #[ignore = "opt-in real CLI discovery; resolves paths without executing providers"]
    fn real_provider_cli_auto_discovery_smoke() {
        if std::env::var("CSM_REAL_CLI_DISCOVERY").as_deref() != Ok("1") {
            return;
        }
        for provider in [Provider::Claude, Provider::Codex] {
            let path = resolve(provider).unwrap();
            assert!(path.is_absolute());
            assert!(executable_file(&path));
        }
        let runtime_path = launch_path().unwrap();
        if let Some(configured) = std::env::var_os("CSM_NODE_CLI").filter(|value| !value.is_empty())
        {
            let runtime_path = runtime_path
                .as_ref()
                .expect("explicit Node must prepare PATH");
            let first = std::env::split_paths(runtime_path).next().unwrap();
            assert_eq!(first, PathBuf::from(configured).parent().unwrap());
        }
        println!("REAL_PROVIDER_CLI_DISCOVERY_OK claude=1 codex=1 executed=0");
        println!(
            "REAL_PROVIDER_RUNTIME_PATH_OK prepared={} executed=0",
            if runtime_path.is_some() { 1 } else { 0 }
        );
    }

    #[test]
    #[ignore = "opt-in real CLI --version through automatic CLI and Node PATH; no prompts"]
    fn real_provider_cli_version_with_runtime_path_smoke() {
        if std::env::var("CSM_REAL_CLI_VERSION").as_deref() != Ok("1") {
            return;
        }
        for provider in [Provider::Claude, Provider::Codex] {
            let path = resolve(provider).unwrap();
            let runtime_path = launch_path().unwrap();
            #[cfg(target_os = "windows")]
            let output = {
                let quote = |value: &str| format!("'{}'", value.replace('\'', "''"));
                let mut script = String::new();
                if let Some(runtime_path) = &runtime_path {
                    script.push_str(&format!(
                        "$env:PATH = {}; ",
                        quote(&runtime_path.to_string_lossy())
                    ));
                }
                if path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"))
                {
                    script.push_str(
                        "Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass -Force -ErrorAction Stop; ",
                    );
                }
                script.push_str(&format!("& {} --version", quote(&path.to_string_lossy())));
                Command::new("powershell.exe")
                    .args(["-NoLogo", "-NoProfile", "-Command"])
                    .arg(script)
                    .output()
                    .unwrap()
            };
            #[cfg(not(target_os = "windows"))]
            let output = {
                let mut command = Command::new(&path);
                command.arg("--version");
                if let Some(runtime_path) = &runtime_path {
                    command.env("PATH", runtime_path);
                }
                command.output().unwrap()
            };
            assert!(
                output.status.success(),
                "{} --version failed with status {}",
                provider.as_str(),
                output.status
            );
        }
        println!("REAL_PROVIDER_CLI_VERSION_OK claude=1 codex=1 prompts=0");
    }

    #[test]
    fn fallback_locations_are_derived_from_the_supplied_home() {
        let fixture = Fixture::new("fallback-home");
        let home = fixture.root.join("different-user-home");
        let roaming = fixture.root.join("roaming");
        let local = fixture.root.join("local");
        let directories =
            fallback_directories_for(Provider::Claude, Some(&home), Some(&roaming), Some(&local));
        assert!(directories.contains(&home.join(".local/bin")));
        assert!(directories.contains(&home.join(".claude/local")));
        assert!(!directories
            .iter()
            .any(|path| path.starts_with("/Users/should-not-be-hardcoded")));
        #[cfg(target_os = "windows")]
        {
            let local_node = directories
                .iter()
                .position(|path| path == &local.join("Programs/nodejs"))
                .unwrap();
            let roaming_npm = directories
                .iter()
                .position(|path| path == &roaming.join("npm"))
                .unwrap();
            assert!(local_node < roaming_npm);
            assert!(directories.contains(&local.join("pnpm")));
        }
        #[cfg(target_os = "macos")]
        assert!(directories.contains(&home.join("Library/pnpm")));
    }
}
