use chrono::{DateTime, Utc};
use reqwest::blocking::Client;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

const UPDATE_SETTINGS_SCHEMA: u32 = 1;
const DEFAULT_GITHUB_REPOSITORY: &str = "zhumengkang/claude-session-manager";
const MAX_RELEASE_RESPONSE_BYTES: u64 = 1024 * 1024;
static UPDATE_SETTINGS_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
struct StoredUpdateSettings {
    schema: u32,
    github_repository: String,
    auto_check: bool,
    dismissed_version: Option<String>,
}

impl Default for StoredUpdateSettings {
    fn default() -> Self {
        Self {
            schema: UPDATE_SETTINGS_SCHEMA,
            github_repository: DEFAULT_GITHUB_REPOSITORY.to_string(),
            auto_check: true,
            dismissed_version: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct UpdateSettings {
    current_version: String,
    github_repository: String,
    auto_check: bool,
    dismissed_version: Option<String>,
}

impl UpdateSettings {
    fn from_stored(stored: StoredUpdateSettings) -> Self {
        Self {
            current_version: env!("CARGO_PKG_VERSION").to_string(),
            github_repository: stored.github_repository,
            auto_check: stored.auto_check,
            dismissed_version: stored.dismissed_version,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct UpdateCheckResult {
    current_version: String,
    latest_version: String,
    release_name: String,
    release_url: String,
    published_at: Option<String>,
    repository: String,
    update_available: bool,
    dismissed: bool,
    checked_at: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: Option<String>,
    html_url: String,
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

fn update_settings_path() -> PathBuf {
    dirs::config_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_default()
        .join("com.claude.sessionmanager")
        .join("update-settings.json")
}

fn normalize_repository(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(String::new());
    }
    if value.len() > 201
        || value.matches('/').count() != 1
        || value.contains(['\\', ':', '@', '?', '#'])
        || value.contains(char::is_whitespace)
    {
        return Err("GitHub 仓库必须使用 owner/repository 格式".into());
    }
    let (owner, repository) = value
        .split_once('/')
        .ok_or_else(|| "GitHub 仓库必须使用 owner/repository 格式".to_string())?;
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment.len() <= 100
            && segment != "."
            && segment != ".."
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    };
    if !valid_segment(owner) || !valid_segment(repository) {
        return Err("GitHub 仓库包含无效字符；请输入 owner/repository，不要粘贴 URL 或凭据".into());
    }
    Ok(format!("{owner}/{repository}"))
}

fn parse_release_version(tag: &str) -> Result<Version, String> {
    let tag = tag.trim();
    let version = tag
        .strip_prefix('v')
        .or_else(|| tag.strip_prefix('V'))
        .unwrap_or(tag);
    Version::parse(version).map_err(|_| {
        format!("最新 Release 标签 {tag:?} 不是语义化版本；请使用 v1.2.3 或 1.2.3 形式")
    })
}

fn validate_release_url(url: &str, repository: &str) -> Result<(), String> {
    let prefix = format!("https://github.com/{repository}/releases/tag/");
    let Some(actual_prefix) = url.get(..prefix.len()) else {
        return Err("GitHub 返回的发布页地址不属于当前仓库".into());
    };
    if !actual_prefix.eq_ignore_ascii_case(&prefix) {
        return Err("GitHub 返回的发布页地址不属于当前仓库".into());
    }
    let tag = &url[prefix.len()..];
    if tag.is_empty() || tag.starts_with('/') || tag.contains(['?', '#', '\r', '\n', '\0']) {
        return Err("GitHub 返回的发布页地址无效".into());
    }
    Ok(())
}

fn load_settings_from(path: &Path) -> Result<StoredUpdateSettings, String> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StoredUpdateSettings::default());
        }
        Err(error) => return Err(format!("读取更新设置失败: {error}")),
    };
    if bytes.len() > 64 * 1024 {
        return Err("更新设置文件过大，未覆盖原文件".into());
    }
    let mut stored: StoredUpdateSettings =
        serde_json::from_slice(&bytes).map_err(|error| format!("解析更新设置失败: {error}"))?;
    if stored.schema != UPDATE_SETTINGS_SCHEMA {
        return Err(format!("不支持的更新设置格式: {}", stored.schema));
    }
    let normalized = normalize_repository(&stored.github_repository)?;
    if normalized != stored.github_repository {
        return Err("更新设置中的 GitHub 仓库格式无效，未覆盖原文件".into());
    }
    if let Some(version) = &stored.dismissed_version {
        Version::parse(version).map_err(|_| "更新设置中的忽略版本无效".to_string())?;
    }
    if stored.github_repository.is_empty() {
        stored.github_repository = DEFAULT_GITHUB_REPOSITORY.to_string();
    }
    Ok(stored)
}

fn save_settings_to(path: &Path, stored: &StoredUpdateSettings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(stored)
        .map_err(|error| format!("序列化更新设置失败: {error}"))?;
    crate::write_file_atomically(path, &bytes).map_err(|error| format!("写入更新设置失败: {error}"))
}

fn public_settings(stored: StoredUpdateSettings) -> UpdateSettings {
    UpdateSettings::from_stored(stored)
}

#[tauri::command]
pub(crate) fn get_update_settings() -> Result<UpdateSettings, String> {
    let _guard = UPDATE_SETTINGS_LOCK
        .lock()
        .map_err(|_| "更新设置锁已失效".to_string())?;
    load_settings_from(&update_settings_path()).map(public_settings)
}

#[tauri::command]
pub(crate) fn save_update_settings(
    github_repository: String,
    auto_check: bool,
) -> Result<UpdateSettings, String> {
    let repository = normalize_repository(&github_repository)?;
    let repository = if repository.is_empty() {
        DEFAULT_GITHUB_REPOSITORY.to_string()
    } else {
        repository
    };
    let _guard = UPDATE_SETTINGS_LOCK
        .lock()
        .map_err(|_| "更新设置锁已失效".to_string())?;
    let path = update_settings_path();
    let mut stored = load_settings_from(&path)?;
    if stored.github_repository != repository {
        stored.dismissed_version = None;
    }
    stored.github_repository = repository;
    stored.auto_check = auto_check;
    save_settings_to(&path, &stored)?;
    Ok(public_settings(stored))
}

#[tauri::command]
pub(crate) fn dismiss_update(version: String) -> Result<UpdateSettings, String> {
    let version = parse_release_version(&version)?.to_string();
    let _guard = UPDATE_SETTINGS_LOCK
        .lock()
        .map_err(|_| "更新设置锁已失效".to_string())?;
    let path = update_settings_path();
    let mut stored = load_settings_from(&path)?;
    stored.dismissed_version = Some(version);
    save_settings_to(&path, &stored)?;
    Ok(public_settings(stored))
}

fn evaluate_release(
    stored: &StoredUpdateSettings,
    release: GitHubRelease,
    checked_at: DateTime<Utc>,
) -> Result<UpdateCheckResult, String> {
    if stored.github_repository.is_empty() {
        return Err("请先在“更新”中填写 GitHub 仓库（owner/repository）".into());
    }
    if release.draft || release.prerelease {
        return Err("最新 Release 是草稿或预发布版本，不作为稳定升级提醒".into());
    }
    let current = Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| format!("当前应用版本无效: {error}"))?;
    let latest = parse_release_version(&release.tag_name)?;
    validate_release_url(&release.html_url, &stored.github_repository)?;
    let latest_version = latest.to_string();
    let dismissed = stored.dismissed_version.as_deref() == Some(latest_version.as_str());
    let release_name = release
        .name
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("Claude Session Manager {latest_version}"));
    Ok(UpdateCheckResult {
        current_version: current.to_string(),
        latest_version: latest_version.clone(),
        release_name,
        release_url: release.html_url,
        published_at: release.published_at,
        repository: stored.github_repository.clone(),
        update_available: latest > current,
        dismissed,
        checked_at: checked_at.to_rfc3339(),
    })
}

fn github_api_url(repository: &str) -> String {
    format!("https://api.github.com/repos/{repository}/releases/latest")
}

fn check_for_updates_blocking() -> Result<UpdateCheckResult, String> {
    let stored = {
        let _guard = UPDATE_SETTINGS_LOCK
            .lock()
            .map_err(|_| "更新设置锁已失效".to_string())?;
        load_settings_from(&update_settings_path())?
    };
    if stored.github_repository.is_empty() {
        return Err("请先在“更新”中填写 GitHub 仓库（owner/repository）".into());
    }
    let client = Client::builder()
        .user_agent(format!(
            "Claude-Session-Manager/{}",
            env!("CARGO_PKG_VERSION")
        ))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .redirect(reqwest::redirect::Policy::limited(3))
        .https_only(true)
        .build()
        .map_err(|error| format!("创建 GitHub 更新检查客户端失败: {error}"))?;
    let response = client
        .get(github_api_url(&stored.github_repository))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .map_err(|error| format!("GitHub 更新检查请求失败: {error}"))?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err("未找到公开仓库或稳定 Release；请检查 owner/repository，并确认已有非草稿、非预发布 Release".into());
    }
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS
    {
        return Err(format!(
            "GitHub 更新检查被限流或拒绝（HTTP {status}），请稍后重试"
        ));
    }
    if !status.is_success() {
        return Err(format!("GitHub 更新检查返回 HTTP {status}"));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RELEASE_RESPONSE_BYTES)
    {
        return Err("GitHub Release 响应过大，已停止读取".into());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_RELEASE_RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取 GitHub Release 响应失败: {error}"))?;
    if bytes.len() as u64 > MAX_RELEASE_RESPONSE_BYTES {
        return Err("GitHub Release 响应超过 1 MiB，已停止解析".into());
    }
    let release: GitHubRelease = serde_json::from_slice(&bytes)
        .map_err(|error| format!("解析 GitHub Release 响应失败: {error}"))?;
    evaluate_release(&stored, release, Utc::now())
}

#[tauri::command]
pub(crate) async fn check_for_updates() -> Result<UpdateCheckResult, String> {
    tokio::task::spawn_blocking(check_for_updates_blocking)
        .await
        .map_err(|error| format!("GitHub 更新检查任务失败: {error}"))?
}

#[tauri::command]
pub(crate) fn open_update_release(release_url: String) -> Result<(), String> {
    let stored = {
        let _guard = UPDATE_SETTINGS_LOCK
            .lock()
            .map_err(|_| "更新设置锁已失效".to_string())?;
        load_settings_from(&update_settings_path())?
    };
    validate_release_url(&release_url, &stored.github_repository)?;
    open::that(&release_url).map_err(|error| format!("打开 GitHub 发布页失败: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_TEST_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    fn temporary_directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "csm-update-checker-{label}-{}-{}",
            std::process::id(),
            NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }

    fn release(tag: &str, url: &str) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.into(),
            name: Some("Test release".into()),
            html_url: url.into(),
            published_at: Some("2026-11-26T00:00:00Z".into()),
            draft: false,
            prerelease: false,
        }
    }

    fn configured() -> StoredUpdateSettings {
        StoredUpdateSettings {
            github_repository: "example-owner/session-manager".into(),
            ..StoredUpdateSettings::default()
        }
    }

    #[test]
    fn missing_settings_default_to_the_embedded_repository() {
        let directory = temporary_directory("default-repository");
        let path = directory.join("update-settings.json");
        let loaded = load_settings_from(&path).unwrap();
        assert_eq!(loaded.github_repository, DEFAULT_GITHUB_REPOSITORY);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repository_validation_accepts_only_owner_repository_form() {
        assert_eq!(
            normalize_repository(" example_owner/session.manager ").unwrap(),
            "example_owner/session.manager"
        );
        assert_eq!(normalize_repository("  ").unwrap(), "");
        for invalid in [
            "https://github.com/owner/repo",
            "owner/repo/extra",
            "owner\\repo",
            "owner/repo?token=secret",
            "owner /repo",
            "owner/",
            "/repo",
            "../repo",
            "owner/..",
        ] {
            assert!(normalize_repository(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn semantic_release_versions_drive_newer_same_and_older_results() {
        let stored = configured();
        let url = "https://github.com/example-owner/session-manager/releases/tag/v9.9.9";
        let newer = evaluate_release(&stored, release("v9.9.9", url), Utc::now()).unwrap();
        assert!(newer.update_available);
        assert_eq!(newer.latest_version, "9.9.9");
        assert_eq!(newer.current_version, env!("CARGO_PKG_VERSION"));
        let same_url = format!(
            "https://github.com/example-owner/session-manager/releases/tag/v{}",
            env!("CARGO_PKG_VERSION")
        );
        let same = evaluate_release(
            &stored,
            release(&format!("v{}", env!("CARGO_PKG_VERSION")), &same_url),
            Utc::now(),
        )
        .unwrap();
        assert!(!same.update_available);
        let older = evaluate_release(
            &stored,
            release(
                "v0.0.1",
                "https://github.com/example-owner/session-manager/releases/tag/v0.0.1",
            ),
            Utc::now(),
        )
        .unwrap();
        assert!(!older.update_available);
        assert!(parse_release_version("release-1.2.3").is_err());
    }

    #[test]
    fn release_evaluation_rejects_drafts_prereleases_and_foreign_urls() {
        let stored = configured();
        let url = "https://github.com/example-owner/session-manager/releases/tag/v9.9.9";
        let mut draft = release("v9.9.9", url);
        draft.draft = true;
        assert!(evaluate_release(&stored, draft, Utc::now()).is_err());
        let mut prerelease = release("v9.9.9", url);
        prerelease.prerelease = true;
        assert!(evaluate_release(&stored, prerelease, Utc::now()).is_err());
        for invalid in [
            "https://evil.example/example-owner/session-manager/releases/tag/v9.9.9",
            "https://github.com/another/repository/releases/tag/v9.9.9",
            "https://github.com.example.com/example-owner/session-manager/releases/tag/v9.9.9",
            "https://github.com/example-owner/session-manager/releases/tag/v9.9.9?x=1",
        ] {
            assert!(
                evaluate_release(&stored, release("v9.9.9", invalid), Utc::now()).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn dismissed_version_is_reported_without_changing_availability() {
        let mut stored = configured();
        stored.dismissed_version = Some("9.9.9".into());
        let result = evaluate_release(
            &stored,
            release(
                "v9.9.9",
                "https://github.com/example-owner/session-manager/releases/tag/v9.9.9",
            ),
            Utc::now(),
        )
        .unwrap();
        assert!(result.update_available);
        assert!(result.dismissed);
    }

    #[test]
    fn settings_round_trip_preserves_and_clears_dismissal_as_expected() {
        let directory = temporary_directory("settings");
        let path = directory.join("update-settings.json");
        let mut stored = configured();
        stored.auto_check = false;
        stored.dismissed_version = Some("9.9.9".into());
        save_settings_to(&path, &stored).unwrap();
        let loaded = load_settings_from(&path).unwrap();
        assert_eq!(loaded.github_repository, "example-owner/session-manager");
        assert!(!loaded.auto_check);
        assert_eq!(loaded.dismissed_version.as_deref(), Some("9.9.9"));
        let same_repository = public_settings(loaded.clone());
        assert_eq!(same_repository.current_version, env!("CARGO_PKG_VERSION"));
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupted_settings_are_not_overwritten() {
        let directory = temporary_directory("corrupt");
        let path = directory.join("update-settings.json");
        let original = b"{ not valid update settings\n";
        std::fs::write(&path, original).unwrap();
        assert!(load_settings_from(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn github_api_url_is_fixed_to_the_selected_public_repository() {
        assert_eq!(
            github_api_url("owner/repo"),
            "https://api.github.com/repos/owner/repo/releases/latest"
        );
    }
}
