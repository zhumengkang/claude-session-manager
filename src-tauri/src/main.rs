#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod provider_cli;
mod provider_process;
mod update_checker;
mod windows_platform;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use blake3::Hasher as ContentHasher;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    collections::HashSet,
    io::{BufRead, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

#[cfg(any(target_os = "windows", test))]
use std::sync::{Condvar, LazyLock};

#[cfg(target_os = "macos")]
use std::process::Command;

// ─── Data types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionInfo {
    session_id: String,
    #[serde(default = "default_provider")]
    provider: String,
    #[serde(default = "default_session_kind")]
    session_kind: String,
    #[serde(default)]
    source: String,
    project_slug: String,
    project_path: String,
    title: String,
    message_count: usize,
    #[serde(default = "default_message_count_loaded")]
    message_count_loaded: bool,
    updated_at: String,
    updated_at_ts: i64,
    created_at: String,
    created_at_ts: i64,
    file_path: String,
    favorited: bool,
}

fn default_provider() -> String {
    "claude".into()
}

fn default_session_kind() -> String {
    "main".into()
}

fn default_message_count_loaded() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Provider {
    Claude,
    Codex,
}

impl Provider {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("claude") {
            "claude" => Ok(Self::Claude),
            "codex" => Ok(Self::Codex),
            other => Err(format!("不支持的会话类型: {other}")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn favorite_key(self, session_id: &str) -> String {
        match self {
            // Keep the original Claude favorites.json entries compatible.
            Self::Claude => session_id.to_string(),
            Self::Codex => format!("codex:{session_id}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMsg {
    role: String,
    text: String,
    timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessagesPage {
    messages: Vec<ChatMsg>,
    /// Exact only when `total_exact` is true; otherwise a known lower bound.
    total: usize,
    /// Kept for compatibility; identical to `next_offset`.
    offset: usize,
    next_offset: usize,
    total_exact: bool,
    has_more: bool,
    #[serde(default)]
    snapshot_len: u64,
    #[serde(default)]
    snapshot_token: String,
}

#[derive(Debug, Default)]
struct MessageSnapshotCursor {
    len: Option<u64>,
    token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessageSnapshot {
    identity: String,
    change_stamp: String,
    len: u64,
    content_hash: ContentHash,
    probes: SnapshotProbes,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SnapshotProbes {
    head_len: u64,
    head_hash: ContentHash,
    tail_start: u64,
    tail_len: u64,
    tail_hash: ContentHash,
}

// ─── Incremental JSONL cache ──────────────────────────────────────────────────

const CACHE_FORMAT_VERSION: u32 = 4;

#[derive(Clone, Default, Serialize, Deserialize)]
struct ParsedState {
    ai_title: String,
    custom_title: String,
    project_path: String,
    user_count: usize,
    created_ts: i64,
    updated_ts: i64,
}

#[derive(Clone, Serialize, Deserialize)]
struct CachedSession {
    mtime: u64,
    file_len: u64,
    parsed_offset: u64,
    #[serde(default)]
    content_hash: Option<[u8; 32]>,
    #[serde(default)]
    change_stamp: Option<String>,
    #[serde(default)]
    growth_probes: Option<SnapshotProbes>,
    state: ParsedState,
}

#[derive(Serialize, Deserialize)]
struct StoredCache {
    version: u32,
    entries: HashMap<String, CachedSession>,
}

struct Cache {
    entries: HashMap<String, CachedSession>,
    dirty: bool,
}

impl Cache {
    fn new() -> Self {
        Cache {
            entries: HashMap::new(),
            dirty: false,
        }
    }

    fn load() -> Self {
        let Ok(bytes) = std::fs::read(cache_file_path()) else {
            return Self::new();
        };
        let Ok(stored) = serde_json::from_slice::<StoredCache>(&bytes) else {
            return Self::new();
        };
        if !(1..=CACHE_FORMAT_VERSION).contains(&stored.version) {
            return Self::new();
        }
        Self {
            entries: stored.entries,
            dirty: false,
        }
    }

    fn persist_if_needed(&mut self) {
        if self.dirty {
            self.persist_to_path(cache_file_path());
        }
    }

    fn persist_to_path(&mut self, path: PathBuf) {
        let stored = StoredCache {
            version: CACHE_FORMAT_VERSION,
            entries: self.entries.clone(),
        };
        let Ok(json) = serde_json::to_vec(&stored) else {
            return;
        };
        if write_file_atomically(&path, &json).is_ok() {
            self.dirty = false;
        }
    }
}

fn write_file_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static TEMPORARY_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent"))?;
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing filename"))?;
    let (temporary, mut file) = (0..128)
        .find_map(|_| {
            let sequence = TEMPORARY_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let mut temporary_name = file_name.to_os_string();
            temporary_name.push(format!(".{}.{}.tmp", std::process::id(), sequence));
            let temporary = parent.join(temporary_name);
            let mut options = std::fs::OpenOptions::new();
            options.create_new(true).read(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&temporary) {
                Ok(file) => Some(Ok((temporary, file))),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .transpose()?
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "no unique temporary file",
            )
        })?;
    let mut temporary = UniqueTemporaryFile {
        path: temporary,
        armed: true,
    };
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary.path, path)?;
    temporary.armed = false;
    Ok(())
}

fn cache_file_path() -> PathBuf {
    if let Ok(directory) = std::env::var("CSM_CACHE_DIR") {
        return PathBuf::from(directory).join("session-metadata-v1.json");
    }
    dirs::cache_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"))
        .join("com.claude.sessionmanager/session-metadata-v1.json")
}

static CACHE: std::sync::LazyLock<Mutex<Cache>> =
    std::sync::LazyLock::new(|| Mutex::new(Cache::load()));

fn metadata_mtime(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
fn file_mtime(path: &Path) -> u64 {
    path.metadata().map(|m| metadata_mtime(&m)).unwrap_or(0)
}

/// Returns a stable file identity and a metadata-change stamp. Unlike mtime,
/// the change stamp is also updated when a same-length rewrite restores the
/// previous modification time. Changed stamps avoid unnecessary reads; tied
/// stamps still require full-content validation before cached state is reused.
#[cfg(unix)]
fn file_identity_and_change(file: &std::fs::File) -> std::io::Result<(String, String)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    Ok((
        format!("{:x}:{:x}", metadata.dev(), metadata.ino()),
        format!("{}:{}", metadata.ctime(), metadata.ctime_nsec()),
    ))
}

#[cfg(windows)]
fn file_identity_and_change(file: &std::fs::File) -> std::io::Result<(String, String)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO, FILE_ID_INFO,
    };

    let handle = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    let mut basic: FILE_BASIC_INFO = unsafe { std::mem::zeroed() };
    let basic_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            &mut basic as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_BASIC_INFO>() as u32,
        )
    };
    if basic_ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut id: FILE_ID_INFO = unsafe { std::mem::zeroed() };
    let id_ok = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            &mut id as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if id_ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let file_id = id
        .FileId
        .Identifier
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok((
        format!("{:x}:{file_id}", id.VolumeSerialNumber),
        basic.ChangeTime.to_string(),
    ))
}

#[cfg(not(any(unix, windows)))]
fn file_identity_and_change(file: &std::fs::File) -> std::io::Result<(String, String)> {
    let metadata = file.metadata()?;
    let modified = metadata
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok((String::new(), modified.to_string()))
}

fn cached_content_hash_matches(
    file: &mut std::fs::File,
    cached: &CachedSession,
    file_len: u64,
) -> std::io::Result<bool> {
    let Some(expected) = cached.content_hash else {
        return Ok(false);
    };
    if cached.file_len != file_len {
        return Ok(false);
    }
    // A restored mtime and a tied platform change stamp cannot prove that the
    // middle of an unchanged-length file is intact. Revalidate the complete
    // cached byte range before returning any previously parsed state.
    hash_content_prefix(file, file_len).map(|(actual, _)| actual == expected)
}

// ─── Paths & favorites ────────────────────────────────────────────────────────

fn provider_config_dir(
    override_dir: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
    default_directory: &str,
) -> PathBuf {
    if let Some(path) = override_dir.filter(|path| !path.is_empty()) {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            path
        } else if let Ok(current) = std::env::current_dir() {
            // Storage enumeration runs before the launcher changes to the project
            // directory. Resolve a relative override now so both phases use one root.
            current.join(path)
        } else {
            path
        };
    }
    home.map(|path| path.join(default_directory))
        .unwrap_or_else(|| PathBuf::from(default_directory))
}

fn configured_provider_dir(provider: Provider) -> Option<(&'static str, PathBuf)> {
    let (variable, directory) = match provider {
        Provider::Claude => ("CLAUDE_CONFIG_DIR", claude_dir()),
        Provider::Codex => ("CODEX_HOME", codex_dir()),
    };
    std::env::var_os(variable)
        .filter(|value| !value.is_empty())
        .map(|_| (variable, directory))
}

fn unicode_launch_path<'a>(path: &'a Path, label: &str) -> Result<&'a str, String> {
    path.to_str()
        .ok_or_else(|| format!("{label} 必须是有效 Unicode 路径，未执行恢复"))
}

#[cfg(any(target_os = "macos", test))]
fn provider_config_launch_prefix(provider: Provider) -> Result<String, String> {
    let Some((variable, directory)) = configured_provider_dir(provider) else {
        return Ok(String::new());
    };
    let path = unicode_launch_path(&directory, variable)?;
    Ok(format!("{variable}={} ", shell_quote(path)))
}

#[cfg(any(target_os = "macos", test))]
fn provider_runtime_launch_prefix() -> Result<String, String> {
    let Some(path) = provider_cli::launch_path()? else {
        return Ok(String::new());
    };
    let path = unicode_launch_path(Path::new(&path), "Node PATH")?;
    Ok(format!("PATH={} ", shell_quote(path)))
}

fn claude_dir() -> PathBuf {
    provider_config_dir(
        std::env::var_os("CLAUDE_CONFIG_DIR"),
        dirs::home_dir(),
        ".claude",
    )
}

fn codex_dir() -> PathBuf {
    provider_config_dir(std::env::var_os("CODEX_HOME"), dirs::home_dir(), ".codex")
}

fn favorites_path() -> PathBuf {
    claude_dir().join("favorites.json")
}

fn read_favorites() -> Result<HashSet<String>, String> {
    let path = favorites_path();
    match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str::<Vec<String>>(&contents)
            .map(|values| values.into_iter().collect())
            .map_err(|error| format!("解析收藏文件 {} 失败: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(HashSet::new()),
        Err(error) => Err(format!("读取收藏文件 {} 失败: {error}", path.display())),
    }
}

fn load_favorites() -> HashSet<String> {
    // Listing sessions can still be useful when only the favorites metadata is
    // damaged. Mutations use `read_favorites` directly so this read-only
    // fallback can never turn into a destructive rewrite of the original file.
    read_favorites().unwrap_or_else(|error| {
        debug_log(&format!("favorites_load fallback=empty error={error}"));
        HashSet::new()
    })
}

struct FavoritesLock(std::fs::File);

impl Drop for FavoritesLock {
    fn drop(&mut self) {
        let _ = std::fs::File::unlock(&self.0);
    }
}

fn lock_favorites() -> Result<FavoritesLock, String> {
    let path = favorites_path();
    let parent = path
        .parent()
        .ok_or_else(|| "收藏文件路径缺少父目录".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("创建收藏目录 {} 失败: {error}", parent.display()))?;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    // Keep this inode permanently. Removing a lock file after each mutation would
    // allow two processes to lock different inodes for the same favorites path.
    let file = options
        .open(parent.join("favorites.lock"))
        .map_err(|error| format!("打开收藏锁文件失败: {error}"))?;
    file.lock()
        .map_err(|error| format!("锁定收藏文件失败: {error}"))?;
    Ok(FavoritesLock(file))
}

struct UniqueTemporaryFile {
    path: PathBuf,
    armed: bool,
}

impl Drop for UniqueTemporaryFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn save_favorites_unlocked(favs: &HashSet<String>) -> Result<(), String> {
    let mut values: Vec<&String> = favs.iter().collect();
    values.sort();
    let json = serde_json::to_string_pretty(&values)
        .map_err(|error| format!("序列化收藏失败: {error}"))?;
    let path = favorites_path();
    write_file_atomically(&path, json.as_bytes())
        .map_err(|error| format!("保存收藏文件 {} 失败: {error}", path.display()))
}

// ─── Load custom names from ~/.claude/sessions/*.json ─────────────────────────
// /rename command writes name to these files, not to the JSONL.
fn load_session_names() -> HashMap<String, String> {
    let mut map: HashMap<String, (i64, String)> = HashMap::new();
    let sessions_dir = claude_dir().join("sessions");
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(s) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                    if let (Some(sid), Some(name)) = (
                        v.get("sessionId").and_then(|x| x.as_str()),
                        v.get("name")
                            .and_then(|x| x.as_str())
                            .filter(|s| !s.is_empty()),
                    ) {
                        // Use updatedAt timestamp to prefer newest rename
                        let updated = v.get("updatedAt").and_then(|x| x.as_i64()).unwrap_or(0);
                        let e = map
                            .entry(sid.to_string())
                            .or_insert((i64::MIN, String::new()));
                        if updated >= e.0 {
                            *e = (updated, name.to_string());
                        }
                    }
                }
            }
        }
    }
    map.into_iter().map(|(k, (_, v))| (k, v)).collect()
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn extract_text(val: &serde_json::Value) -> String {
    if let Some(s) = val.as_str() {
        return s.to_string();
    }
    if let Some(arr) = val.as_array() {
        return arr
            .iter()
            .filter(|c| c.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|c| c.get("text")?.as_str())
            .collect::<Vec<_>>()
            .join("\n");
    }
    String::new()
}

fn strip_claude_meta_noise(text: &str) -> String {
    let mut cleaned = text.to_string();
    for tag in [
        "command-name",
        "local-command-caveat",
        "task-notification",
        "system-reminder",
        "bash-input",
        "local-command-stdout",
    ] {
        let opening = format!("<{tag}>");
        let closing = format!("</{tag}>");
        while let Some(start) = cleaned.find(&opening) {
            let content_start = start + opening.len();
            if let Some(relative_end) = cleaned[content_start..].find(&closing) {
                let end = content_start + relative_end + closing.len();
                cleaned.replace_range(start..end, "\n");
            } else {
                // An unterminated generated metadata block consumes only the
                // remainder; real user text before it must still survive.
                cleaned.truncate(start);
                break;
            }
        }
    }

    let trimmed = cleaned.trim_start();
    if trimmed.starts_with("Caveat:") {
        let remainder = trimmed
            .find("\r\n\r\n")
            .map(|index| &trimmed[index + 4..])
            .or_else(|| trimmed.find("\n\n").map(|index| &trimmed[index + 2..]))
            .or_else(|| trimmed.find('\n').map(|index| &trimmed[index + 1..]));
        cleaned = remainder.unwrap_or("").to_string();
    }
    cleaned.trim().to_string()
}

fn parse_ts(iso: &str) -> i64 {
    DateTime::parse_from_rfc3339(iso)
        .map(|dt| dt.with_timezone(&Utc).timestamp())
        .unwrap_or(0)
}

fn fmt_ts_display(iso: &str) -> String {
    if let Ok(dt) = DateTime::parse_from_rfc3339(iso) {
        dt.with_timezone(&chrono::Local)
            .format("%m/%d %H:%M")
            .to_string()
    } else {
        String::new()
    }
}

fn fmt_ts(ts: i64) -> String {
    if ts <= 0 {
        return String::new();
    }
    let now = Utc::now().timestamp();
    let diff = (now - ts).max(0) as u64;
    match diff {
        s if s < 60 => "刚才".into(),
        s if s < 3600 => format!("{}分钟前", s / 60),
        s if s < 86400 => format!("{}小时前", s / 3600),
        s if s < 604800 => format!("{}天前", s / 86400),
        _ => {
            if let Some(dt) = DateTime::from_timestamp(ts, 0) {
                dt.with_timezone(&chrono::Local)
                    .format("%Y/%m/%d")
                    .to_string()
            } else {
                String::new()
            }
        }
    }
}

fn is_valid_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, &c) in b.iter().enumerate() {
        if matches!(i, 8 | 13 | 18 | 23) {
            if c != b'-' {
                return false;
            }
        } else if !c.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

// ─── Session parsing ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SessionLine<'a> {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    cwd: Option<String>,
    #[serde(rename = "aiTitle")]
    ai_title: Option<String>,
    #[serde(rename = "customTitle")]
    custom_title: Option<String>,
    #[serde(rename = "isSidechain", default)]
    is_sidechain: bool,
    #[serde(rename = "isMeta", default)]
    is_meta: bool,
    /// Keep large tool/message payloads as raw JSON until a real user text is needed.
    #[serde(borrow)]
    message: Option<&'a serde_json::value::RawValue>,
}

impl ParsedState {
    fn consume_line(&mut self, line: &[u8]) {
        let e: SessionLine = match serde_json::from_slice(line) {
            Ok(v) => v,
            Err(_) => return,
        };
        let kind = e.kind.as_deref().unwrap_or("");

        if let Some(ts_str) = e.timestamp.as_deref() {
            let ts = parse_ts(ts_str);
            if ts > 0 {
                if self.created_ts == 0 {
                    self.created_ts = ts;
                }
                if ts > self.updated_ts {
                    self.updated_ts = ts;
                }
            }
        }

        // custom-title wins over ai-title; retain the latest non-empty value.
        if kind == "ai-title" {
            if let Some(title) = e.ai_title.filter(|title| !title.is_empty()) {
                self.ai_title = title;
            }
        } else if kind == "custom-title" {
            if let Some(title) = e.custom_title.filter(|title| !title.is_empty()) {
                self.custom_title = title;
            }
        }

        if kind == "user" && !e.is_meta && !e.is_sidechain {
            if let Some(raw_message) = e.message {
                let msg: serde_json::Value =
                    serde_json::from_str(raw_message.get()).unwrap_or(serde_json::Value::Null);
                let text = strip_claude_meta_noise(&extract_text(
                    msg.get("content").unwrap_or(&serde_json::Value::Null),
                ));
                if !text.is_empty() {
                    if self.project_path.is_empty() {
                        self.project_path = e.cwd.clone().unwrap_or_default();
                    }
                    self.user_count += 1;
                }
            }
        }

        if self.project_path.is_empty() {
            if let Some(cwd) = e.cwd.filter(|cwd| !cwd.is_empty()) {
                self.project_path = cwd;
            }
        }
    }

    fn consume_codex_count_line(&mut self, line: &[u8]) {
        // Rollout files contain many large reasoning/tool records. Only candidate
        // user messages need JSON parsing; the final parser still applies all
        // metadata and content filters, so this byte prefilter only avoids work.
        if !json_field_is(line, "type", "response_item") || !json_field_is(line, "role", "user") {
            return;
        }
        if let Ok(event) = serde_json::from_slice::<serde_json::Value>(line) {
            if parse_codex_chat_msg(&event).is_some_and(|message| message.role == "user") {
                self.user_count += 1;
            }
        }
    }

    fn session_info(
        &self,
        session_id: &str,
        slug: &str,
        file_path: &str,
        mtime: u64,
        favs: &HashSet<String>,
    ) -> SessionInfo {
        let mut title = if !self.custom_title.is_empty() {
            self.custom_title.clone()
        } else {
            self.ai_title.clone()
        };

        if title.is_empty() {
            let n = session_id
                .char_indices()
                .nth(8)
                .map(|(i, _)| i)
                .unwrap_or(session_id.len());
            let prefix = if self.user_count == 0 {
                "[无用户消息]"
            } else {
                "[未命名]"
            };
            title = format!("{} {}", prefix, &session_id[..n]);
        }

        // The timestamp is nanoseconds here; convert the mtime fallback to seconds.
        let updated_ts = if self.updated_ts > 0 {
            self.updated_ts
        } else {
            (mtime / 1_000_000_000) as i64
        };

        SessionInfo {
            session_id: session_id.to_string(),
            provider: Provider::Claude.as_str().into(),
            session_kind: "main".into(),
            source: String::new(),
            project_slug: slug.to_string(),
            project_path: self.project_path.clone(),
            title,
            message_count: self.user_count,
            message_count_loaded: true,
            updated_at: fmt_ts(updated_ts),
            updated_at_ts: updated_ts,
            created_at: fmt_ts(self.created_ts),
            created_at_ts: self.created_ts,
            file_path: file_path.to_string(),
            favorited: favs.contains(&Provider::Claude.favorite_key(session_id)),
        }
    }
}

type ContentHash = [u8; 32];

#[cfg(test)]
thread_local! {
    static CACHE_TEST_BYTES_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_cache_test_read(bytes: usize) {
    CACHE_TEST_BYTES_READ.with(|total| total.set(total.get().saturating_add(bytes as u64)));
}

#[cfg(test)]
fn take_cache_test_bytes_read() -> u64 {
    CACHE_TEST_BYTES_READ.with(|total| total.replace(0))
}

fn hash_content_prefix(
    file: &mut std::fs::File,
    len: u64,
) -> std::io::Result<(ContentHash, ContentHasher)> {
    file.seek(SeekFrom::Start(0))?;
    let mut remaining = len;
    let mut hasher = ContentHasher::new();
    let mut buffer = [0_u8; 256 * 1024];
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        let read = file.read(&mut buffer[..wanted])?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "session file shrank while hashing its previous snapshot",
            ));
        }
        hasher.update(&buffer[..read]);
        #[cfg(test)]
        record_cache_test_read(read);
        remaining -= read as u64;
    }
    let digest = *hasher.finalize().as_bytes();
    Ok((digest, hasher))
}

#[cfg(test)]
fn read_content_hash(file: &mut std::fs::File, len: u64) -> std::io::Result<ContentHash> {
    hash_content_prefix(file, len).map(|(digest, _)| digest)
}

fn json_field_is(line: &[u8], field: &str, expected: &str) -> bool {
    let field = format!("\"{field}\"").into_bytes();
    let expected = format!("\"{expected}\"").into_bytes();
    let mut search_start = 0usize;
    while search_start + field.len() <= line.len() {
        let Some(relative) = line[search_start..]
            .windows(field.len())
            .position(|window| window == field)
        else {
            return false;
        };
        let mut cursor = search_start + relative + field.len();
        while line
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            cursor += 1;
        }
        if line.get(cursor) != Some(&b':') {
            search_start += relative + field.len();
            continue;
        }
        cursor += 1;
        while line
            .get(cursor)
            .is_some_and(|byte| byte.is_ascii_whitespace())
        {
            cursor += 1;
        }
        if line[cursor..].starts_with(&expected) {
            return true;
        }
        search_start += relative + field.len();
    }
    false
}

/// Parse complete lines from `start`. `hasher` already covers bytes through
/// `hashed_offset`; when a partial old tail is reparsed, only its new suffix is
/// hashed again. Returns offsets and the hash of the complete observed file.
fn parse_complete_lines(
    file: &mut std::fs::File,
    start: u64,
    hashed_offset: u64,
    mut hasher: ContentHasher,
    state: &mut ParsedState,
    provider: Provider,
) -> std::io::Result<(u64, u64, ContentHash)> {
    file.seek(SeekFrom::Start(start))?;
    let mut reader = std::io::BufReader::new(file);
    let mut parsed_offset = start;
    let mut observed_end = start;

    loop {
        let mut line = Vec::new();
        let bytes = reader.read_until(b'\n', &mut line)?;
        if bytes == 0 {
            break;
        }
        #[cfg(test)]
        record_cache_test_read(bytes);
        let line_start = observed_end;
        observed_end += bytes as u64;
        let already_hashed = hashed_offset
            .saturating_sub(line_start)
            .min(line.len() as u64) as usize;
        hasher.update(&line[already_hashed..]);
        if line.last() == Some(&b'\n') {
            match provider {
                Provider::Claude => state.consume_line(&line),
                Provider::Codex => state.consume_codex_count_line(&line),
            }
            parsed_offset = observed_end;
        } else {
            // A valid JSON record does not require a trailing newline. Only an
            // incomplete/invalid tail waits for a later append before advancing.
            if serde_json::from_slice::<&serde_json::value::RawValue>(&line).is_err() {
                break;
            }
            match provider {
                Provider::Claude => state.consume_line(&line),
                Provider::Codex => state.consume_codex_count_line(&line),
            }
            parsed_offset = observed_end;
        }
    }

    Ok((parsed_offset, observed_end, *hasher.finalize().as_bytes()))
}

fn parsed_state_with_cache(
    path: &PathBuf,
    cache_key: String,
    provider: Provider,
    cache: &mut Cache,
) -> Option<(ParsedState, u64)> {
    let mut file = std::fs::File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let mtime = metadata_mtime(&metadata);
    let file_len = metadata.len();
    let (identity, change) = file_identity_and_change(&file).ok()?;
    let change_stamp = format!("{identity}:{change}");
    let cached = cache.entries.get(&cache_key).cloned();

    // Reusing unchanged state requires a current full-content hash. In
    // particular, legacy entries must be reparsed once rather than blessed with
    // a new change stamp while their old parsed state is retained.
    if let Some(c) = &cached {
        if c.mtime == mtime
            && c.file_len == file_len
            && c.change_stamp.as_deref() == Some(change_stamp.as_str())
            && cached_content_hash_matches(&mut file, c, file_len).ok()?
        {
            return Some((c.state.clone(), mtime));
        }
    }

    // Pure growth is incremental only after hashing the complete old prefix.
    // Edge probes alone cannot distinguish a normal append from a middle rewrite
    // followed by an append; accepting the latter would mix two cache versions.
    let mut append_state = None;
    if let Some(c) = &cached {
        if file_len > c.file_len {
            if let Some(expected_hash) = c.content_hash {
                if let Ok((current_hash, hasher)) = hash_content_prefix(&mut file, c.file_len) {
                    if current_hash == expected_hash {
                        append_state = Some((c.parsed_offset, c.state.clone(), c.file_len, hasher));
                    }
                }
            }
        }
    }

    let (start, mut state, hashed_offset, hasher) =
        append_state.unwrap_or((0, ParsedState::default(), 0, ContentHasher::new()));
    let (parsed_offset, observed_end, content_hash) = parse_complete_lines(
        &mut file,
        start,
        hashed_offset,
        hasher,
        &mut state,
        provider,
    )
    .ok()?;

    cache.entries.insert(
        cache_key,
        CachedSession {
            mtime,
            file_len: observed_end,
            parsed_offset,
            content_hash: Some(content_hash),
            change_stamp: Some(change_stamp),
            growth_probes: Some(create_snapshot_probes(&mut file, observed_end).ok()?),
            state: state.clone(),
        },
    );
    cache.dirty = true;
    Some((state, mtime))
}

fn parse_session_file_with_cache(
    path: &PathBuf,
    slug: &str,
    favs: &HashSet<String>,
    cache: &mut Cache,
) -> Option<SessionInfo> {
    let session_id = path.file_stem()?.to_string_lossy().to_string();
    if !is_valid_uuid(&session_id) {
        return None;
    }
    let fp_str = path.to_str()?.to_string();
    let (state, mtime) = parsed_state_with_cache(path, fp_str.clone(), Provider::Claude, cache)?;
    Some(state.session_info(&session_id, slug, &fp_str, mtime, favs))
}

fn parse_session_file(path: &PathBuf, slug: &str, favs: &HashSet<String>) -> Option<SessionInfo> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    parse_session_file_with_cache(path, slug, favs, &mut cache)
}

// ─── Commands ─────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn default_debug_log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library/Logs/Claude Session Manager/launch.log"))
}

#[cfg(target_os = "windows")]
fn default_debug_log_path() -> Option<PathBuf> {
    dirs::data_local_dir().map(|directory| directory.join("Claude Session Manager/Logs/launch.log"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn default_debug_log_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|directory| directory.join("com.claude.sessionmanager/launch.log"))
}

fn debug_log(message: &str) {
    let path = match std::env::var_os("CSM_DEBUG_LOG") {
        Some(path) => PathBuf::from(path),
        None if cfg!(test) => return,
        None => {
            let Some(path) = default_debug_log_path() else {
                return;
            };
            path
        }
    };
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{} {}", Utc::now().to_rfc3339(), message);
    }
}

#[derive(Debug)]
struct ClaudeProjectRoot {
    slug: String,
    read_path: PathBuf,
    canonical_path: PathBuf,
}

/// Only direct children of `projects` can introduce an external root. Nested
/// symlinks are never followed, so an explicit project link cannot silently
/// expand into an arbitrary filesystem tree.
fn discover_claude_project_roots(projects: &Path) -> Result<Vec<ClaudeProjectRoot>, String> {
    let entries = match std::fs::read_dir(projects) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "读取 Claude 项目目录 {} 失败: {error}",
                projects.display()
            ));
        }
    };
    let mut roots = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("枚举 Claude 项目目录失败: {error}"))?;
        let slug_dir = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "读取 Claude 项目条目类型 {} 失败: {error}",
                    slug_dir.display()
                ));
            }
        };
        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }
        let canonical_path = match std::fs::canonicalize(&slug_dir) {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "定位 Claude 项目目录 {} 失败: {error}",
                    slug_dir.display()
                ));
            }
        };
        let is_directory = match std::fs::metadata(&canonical_path) {
            Ok(metadata) => metadata.is_dir(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "读取 Claude 项目目录元数据 {} 失败: {error}",
                    slug_dir.display()
                ));
            }
        };
        if !is_directory {
            continue;
        }
        let Some(slug) = slug_dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        else {
            continue;
        };
        roots.push(ClaudeProjectRoot {
            slug,
            read_path: slug_dir,
            canonical_path,
        });
    }
    roots.sort_by(|left, right| left.slug.cmp(&right.slug));
    Ok(roots)
}

fn get_claude_sessions(favs: &HashSet<String>) -> Result<Vec<SessionInfo>, String> {
    let names = load_session_names(); // /rename overrides
    let projects = claude_dir().join("projects");
    let roots = discover_claude_project_roots(&projects)?;
    let mut sessions = Vec::new();
    let mut seen_paths = HashSet::new();
    for root in roots {
        let files = match std::fs::read_dir(&root.read_path) {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "读取 Claude 会话目录 {} 失败: {error}",
                    root.read_path.display()
                ));
            }
        };
        for file in files {
            let file = file.map_err(|error| {
                format!(
                    "枚举 Claude 会话目录 {} 失败: {error}",
                    root.read_path.display()
                )
            })?;
            let fp = file.path();
            if fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let canonical = match std::fs::canonicalize(&fp) {
                Ok(path) => path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "定位 Claude 会话文件 {} 失败: {error}",
                        fp.display()
                    ));
                }
            };
            let is_file = match std::fs::metadata(&fp) {
                Ok(metadata) => metadata.is_file(),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "读取 Claude 会话文件元数据 {} 失败: {error}",
                        fp.display()
                    ));
                }
            };
            // A nested link or a direct file link may not escape the explicit
            // project root that introduced this entry.
            if !is_file || !canonical.starts_with(&root.canonical_path) {
                continue;
            }
            seen_paths.insert(fp.to_string_lossy().to_string());
            if let Some(mut s) = parse_session_file(&fp, &root.slug, favs) {
                if let Some(custom) = names.get(&s.session_id) {
                    s.title = custom.clone();
                }
                sessions.push(s);
            }
        }
    }
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at_ts));
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let previous_len = cache.entries.len();
    // Reaching this point proves enumeration completed. An I/O error returns
    // above without treating a partially seen directory as an empty provider.
    cache
        .entries
        .retain(|path, _| path.starts_with("codex:") || seen_paths.contains(path.as_str()));
    if cache.entries.len() != previous_len {
        cache.dirty = true;
    }
    cache.persist_if_needed();
    drop(cache);
    Ok(sessions)
}

fn load_codex_names_from_reader<R: BufRead>(reader: R) -> HashMap<String, String> {
    let mut latest: HashMap<String, (Option<DateTime<Utc>>, String)> = HashMap::new();
    for line in reader.lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let (Some(id), Some(name)) = (
            value.get("id").and_then(|v| v.as_str()),
            value
                .get("thread_name")
                .and_then(|v| v.as_str())
                .filter(|name| !name.trim().is_empty()),
        ) else {
            continue;
        };
        let updated = value
            .get("updated_at")
            .and_then(|v| v.as_str())
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc));
        let replace = match latest.get(id) {
            None => true,
            Some((current, _)) => match (&updated, current) {
                (Some(candidate), Some(current)) => candidate >= current,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                // The index is append-only: the later record wins an exact tie.
                (None, None) => true,
            },
        };
        if replace {
            latest.insert(id.to_string(), (updated, name.trim().to_string()));
        }
    }
    latest
        .into_iter()
        .map(|(id, (_, name))| (id, name))
        .collect()
}

fn load_codex_names() -> HashMap<String, String> {
    let path = codex_dir().join("session_index.jsonl");
    std::fs::File::open(path)
        .map(std::io::BufReader::new)
        .map(load_codex_names_from_reader)
        .unwrap_or_default()
}

fn codex_cache_key(path: &Path) -> String {
    let normalized = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    format!("codex:{}", normalized.to_string_lossy())
}

fn cached_codex_message_count(path: &Path, cache_key: &str, cache: &Cache) -> (usize, bool) {
    let Some(cached) = cache.entries.get(cache_key) else {
        return (0, false);
    };
    let Ok(mut file) = std::fs::File::open(path) else {
        return (cached.state.user_count, false);
    };
    let Ok(metadata) = file.metadata() else {
        return (cached.state.user_count, false);
    };
    let Ok((identity, change)) = file_identity_and_change(&file) else {
        return (cached.state.user_count, false);
    };
    let change_stamp = format!("{identity}:{change}");
    let unchanged = cached.mtime == metadata_mtime(&metadata)
        && cached.file_len == metadata.len()
        && cached.change_stamp.as_deref() == Some(change_stamp.as_str())
        && cached_content_hash_matches(&mut file, cached, metadata.len()).unwrap_or(false);
    (cached.state.user_count, unchanged)
}

fn read_codex_thread_rows(db_path: &Path) -> Result<Vec<serde_json::Value>, String> {
    use rusqlite::{Connection, OpenFlags};

    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("无法打开 Codex 会话数据库: {e}"))?;
    connection
        .busy_timeout(std::time::Duration::from_millis(1000))
        .map_err(|e| format!("无法配置 Codex 数据库读取: {e}"))?;
    let mut statement = connection
        .prepare(
            "SELECT id, rollout_path, created_at, updated_at, source, cwd, title, \
             first_user_message, name, preview FROM threads WHERE archived = 0 \
             ORDER BY updated_at DESC;",
        )
        .map_err(|e| format!("读取 Codex 会话数据库失败: {e}"))?;
    let rows = statement
        .query_map([], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, Option<String>>(0)?,
                "rollout_path": row.get::<_, Option<String>>(1)?,
                "created_at": row.get::<_, Option<i64>>(2)?,
                "updated_at": row.get::<_, Option<i64>>(3)?,
                "source": row.get::<_, Option<String>>(4)?,
                "cwd": row.get::<_, Option<String>>(5)?,
                "title": row.get::<_, Option<String>>(6)?,
                "first_user_message": row.get::<_, Option<String>>(7)?,
                "name": row.get::<_, Option<String>>(8)?,
                "preview": row.get::<_, Option<String>>(9)?,
            }))
        })
        .map_err(|e| format!("查询 Codex 会话数据库失败: {e}"))?;
    rows.map(|row| row.map_err(|e| format!("解码 Codex 会话记录失败: {e}")))
        .collect()
}

fn get_codex_sessions(favs: &HashSet<String>) -> Result<Vec<SessionInfo>, String> {
    get_codex_sessions_from_db(&codex_dir().join("state_5.sqlite"), favs)
}

fn get_codex_sessions_from_db(
    db_path: &Path,
    favs: &HashSet<String>,
) -> Result<Vec<SessionInfo>, String> {
    match std::fs::metadata(db_path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(format!("Codex 会话数据库不是文件: {}", db_path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(format!(
                "无法读取 Codex 会话数据库元数据 {}: {error}",
                db_path.display()
            ));
        }
    }
    let rows = read_codex_thread_rows(db_path)?;
    let custom_names = load_codex_names();
    let mut sessions = Vec::new();
    let mut seen_cache_keys = HashSet::new();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    for row in rows {
        let Some(session_id) = row.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if !is_valid_uuid(session_id) {
            continue;
        }
        let raw_string_field = |key: &str| {
            row.get(key)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        let string_field = |key: &str| raw_string_field(key).trim().to_string();
        let explicit_name = string_field("name");
        let mut title = if !explicit_name.is_empty() {
            explicit_name
        } else if let Some(name) = custom_names.get(session_id) {
            name.clone()
        } else {
            String::new()
        };
        for candidate in ["title", "preview", "first_user_message"] {
            if title.is_empty() {
                title = string_field(candidate);
            }
        }
        if title.is_empty() {
            title = format!("[未命名] {}", &session_id[..8]);
        }
        let project_path = raw_string_field("cwd");
        let project_slug = std::path::Path::new(&project_path)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "codex".into());
        let created_at_ts = row.get("created_at").and_then(|v| v.as_i64()).unwrap_or(0);
        let mut updated_at_ts = row.get("updated_at").and_then(|v| v.as_i64()).unwrap_or(0);
        if updated_at_ts == 0 {
            updated_at_ts = created_at_ts;
        }
        let source = string_field("source");
        let session_kind = if source.contains("subagent") {
            "subagent"
        } else {
            "main"
        };
        let file_path = raw_string_field("rollout_path");
        let cache_key = codex_cache_key(Path::new(&file_path));
        let (message_count, message_count_loaded) = if file_path.is_empty() {
            (0, true)
        } else {
            seen_cache_keys.insert(cache_key.clone());
            cached_codex_message_count(Path::new(&file_path), &cache_key, &cache)
        };
        sessions.push(SessionInfo {
            session_id: session_id.to_string(),
            provider: Provider::Codex.as_str().into(),
            session_kind: session_kind.into(),
            source,
            project_slug,
            project_path,
            title,
            message_count,
            message_count_loaded,
            updated_at: fmt_ts(updated_at_ts),
            updated_at_ts,
            created_at: fmt_ts(created_at_ts),
            created_at_ts,
            file_path,
            favorited: favs.contains(&Provider::Codex.favorite_key(session_id)),
        });
    }
    let previous_len = cache.entries.len();
    cache
        .entries
        .retain(|key, _| !key.starts_with("codex:") || seen_cache_keys.contains(key));
    if cache.entries.len() != previous_len {
        cache.dirty = true;
    }
    cache.persist_if_needed();
    drop(cache);
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated_at_ts));
    Ok(sessions)
}

#[tauri::command]
async fn get_sessions(provider: Option<String>) -> Result<Vec<SessionInfo>, String> {
    let provider = Provider::parse(provider.as_deref())?;
    tokio::task::spawn_blocking(move || {
        let favs = load_favorites();
        let sessions = match provider {
            Provider::Claude => get_claude_sessions(&favs),
            Provider::Codex => get_codex_sessions(&favs),
        }?;
        debug_log(&format!(
            "get_sessions provider={} total={} favorites={}",
            provider.as_str(),
            sessions.len(),
            sessions.iter().filter(|s| s.favorited).count()
        ));
        Ok(sessions)
    })
    .await
    .map_err(|e| format!("会话加载任务失败: {e}"))?
}

#[tauri::command]
async fn get_codex_message_count(file_path: String) -> Result<usize, String> {
    tokio::task::spawn_blocking(move || {
        let rollout = std::fs::canonicalize(&file_path)
            .map_err(|e| format!("无法定位 Codex rollout 文件: {e}"))?;
        let root = std::fs::canonicalize(codex_dir())
            .map_err(|e| format!("无法定位 Codex 数据目录: {e}"))?;
        if rollout.extension().and_then(|v| v.to_str()) != Some("jsonl")
            || !rollout.starts_with(&root)
        {
            return Err("消息计数路径不在 Codex 数据目录内".into());
        }
        let cache_key = codex_cache_key(&rollout);
        // Do not hold the global cache lock while reading a large rollout. Merge
        // the single updated entry after parsing so list refreshes stay responsive.
        let cached = {
            let cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            cache.entries.get(&cache_key).cloned()
        };
        let mut local_cache = Cache::new();
        if let Some(cached) = cached {
            local_cache.entries.insert(cache_key.clone(), cached);
        }
        let (state, _) = parsed_state_with_cache(
            &rollout,
            cache_key.clone(),
            Provider::Codex,
            &mut local_cache,
        )
        .ok_or_else(|| "无法读取 Codex 消息计数".to_string())?;
        if let Some(updated) = local_cache.entries.get(&cache_key).cloned() {
            let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
            cache.entries.insert(cache_key, updated);
            cache.dirty = true;
            cache.persist_if_needed();
        }
        Ok(state.user_count)
    })
    .await
    .map_err(|e| format!("Codex 消息计数任务失败: {e}"))?
}

#[tauri::command]
async fn get_favorite_sessions(provider: Option<String>) -> Result<Vec<SessionInfo>, String> {
    let provider = Provider::parse(provider.as_deref())?;
    tokio::task::spawn_blocking(move || {
        let favs = load_favorites();
        // The button lives inside the active provider's session library. Keep
        // its batch scoped to that provider so opening Claude favorites cannot
        // silently start Codex sessions and block their later explicit launch.
        let mut sessions = match provider {
            Provider::Claude => get_claude_sessions(&favs)?,
            Provider::Codex => get_codex_sessions(&favs)?,
        };
        sessions.retain(|session| session.favorited);
        Ok(sessions)
    })
    .await
    .map_err(|e| format!("收藏会话加载任务失败: {e}"))?
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct ClaudeStorageStats {
    recoverable: usize,
    history_ids: usize,
    history_only: usize,
    subagent_records: usize,
}

fn claude_storage_stats_from_roots(
    projects: &Path,
    history_path: &Path,
) -> Result<ClaudeStorageStats, String> {
    let mut top_ids = HashSet::new();
    let mut nested_jsonl = 0usize;
    for root in discover_claude_project_roots(projects)? {
        let mut stack = vec![root.canonical_path.clone()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(format!(
                        "读取 Claude 存储目录 {} 失败: {error}",
                        dir.display()
                    ));
                }
            };
            for entry in entries {
                let entry = entry.map_err(|error| {
                    format!("枚举 Claude 存储目录 {} 失败: {error}", dir.display())
                })?;
                // DirEntry::file_type does not follow symlinks. Nested symlinked
                // directories are skipped to prevent cycles and duplicate trees.
                let file_type = entry.file_type().map_err(|error| {
                    format!(
                        "读取 Claude 存储条目类型 {} 失败: {error}",
                        entry.path().display()
                    )
                })?;
                let path = entry.path();
                if file_type.is_dir() {
                    stack.push(path);
                    continue;
                }
                if !file_type.is_file()
                    || path.extension().and_then(|v| v.to_str()) != Some("jsonl")
                    || path.to_str().is_none()
                {
                    continue;
                }
                let is_top_level = path.parent() == Some(root.canonical_path.as_path());
                if !is_top_level {
                    nested_jsonl += 1;
                    continue;
                }
                if let Some(id) = path.file_stem().and_then(|v| v.to_str()) {
                    if is_valid_uuid(id) {
                        top_ids.insert(id.to_string());
                    }
                }
            }
        }
    }
    let mut history = HashSet::new();
    match std::fs::File::open(history_path) {
        Ok(file) => {
            for line in std::io::BufReader::new(file).lines() {
                let line = line.map_err(|error| {
                    format!(
                        "读取 Claude 历史文件 {} 失败: {error}",
                        history_path.display()
                    )
                })?;
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(id) = value.get("sessionId").and_then(|v| v.as_str()) {
                        history.insert(id.to_string());
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "无法打开 Claude 历史文件 {}: {error}",
                history_path.display()
            ));
        }
    }
    let history_only = history.difference(&top_ids).count();
    Ok(ClaudeStorageStats {
        recoverable: top_ids.len(),
        history_ids: history.len(),
        history_only,
        // Duplicate top-level UUID files are not subagents. Only genuinely
        // nested JSONL records use the subagent/miscellaneous counter.
        subagent_records: nested_jsonl,
    })
}

#[tauri::command]
async fn get_claude_storage_stats() -> Result<ClaudeStorageStats, String> {
    tokio::task::spawn_blocking(move || {
        claude_storage_stats_from_roots(
            &claude_dir().join("projects"),
            &claude_dir().join("history.jsonl"),
        )
    })
    .await
    .map_err(|error| format!("Claude 存储统计任务失败: {error}"))?
}

fn is_codex_meta_text(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("<environment_context>")
        || text.starts_with("# AGENTS.md instructions")
        || text.starts_with("# AGENTS.md\n")
        || text.starts_with("<INSTRUCTIONS>")
        || text.starts_with("<user_instructions>")
        || text.starts_with("<permissions instructions>")
        || text.starts_with("<system-reminder>")
        || text.starts_with("<local-command-caveat>")
}

fn parse_codex_chat_msg(e: &serde_json::Value) -> Option<ChatMsg> {
    if e.get("type").and_then(|v| v.as_str()) != Some("response_item") {
        return None;
    }
    let payload = e.get("payload")?;
    let payload_type = payload.get("type").and_then(|v| v.as_str())?;
    let timestamp = e
        .get("timestamp")
        .and_then(|v| v.as_str())
        .map(fmt_ts_display)
        .unwrap_or_default();
    if matches!(payload_type, "function_call" | "custom_tool_call") {
        let name = payload.get("name").and_then(|v| v.as_str())?;
        return Some(ChatMsg {
            role: "assistant".into(),
            text: format!("[工具调用: {name}]"),
            timestamp,
        });
    }
    if payload_type != "message" {
        return None;
    }
    let role = match payload.get("role").and_then(|v| v.as_str())? {
        "user" => "user",
        "assistant" => "assistant",
        _ => return None,
    };
    let content = payload.get("content")?;
    let text = if let Some(text) = content.as_str() {
        text.to_string()
    } else if let Some(items) = content.as_array() {
        items
            .iter()
            .filter(|item| {
                matches!(
                    item.get("type").and_then(|v| v.as_str()),
                    Some("input_text" | "output_text" | "text")
                )
            })
            .filter_map(|item| item.get("text").and_then(|v| v.as_str()))
            .filter(|text| !is_codex_meta_text(text))
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        String::new()
    };
    let text = text.trim().to_string();
    if text.is_empty() || is_codex_meta_text(&text) {
        return None;
    }
    Some(ChatMsg {
        role: role.into(),
        text,
        timestamp,
    })
}

fn parse_claude_chat_msg(e: &serde_json::Value) -> Option<ChatMsg> {
    let kind = e.get("type").and_then(|v| v.as_str())?;
    if kind != "user" && kind != "assistant" {
        return None;
    }
    if e.get("isMeta").and_then(|v| v.as_bool()).unwrap_or(false)
        || e.get("isSidechain")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        return None;
    }
    let msg = e.get("message")?;
    let content = msg.get("content").unwrap_or(&serde_json::Value::Null);
    let timestamp = e
        .get("timestamp")
        .and_then(|v| v.as_str())
        .map(fmt_ts_display)
        .unwrap_or_default();
    let text = strip_claude_meta_noise(&extract_text(content));
    if !text.is_empty() {
        return Some(ChatMsg {
            role: kind.into(),
            text,
            timestamp,
        });
    }
    if kind == "assistant" {
        if let Some(items) = content.as_array() {
            let tools: Vec<&str> = items
                .iter()
                .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("tool_use"))
                .filter_map(|item| item.get("name").and_then(|v| v.as_str()))
                .collect();
            if !tools.is_empty() {
                return Some(ChatMsg {
                    role: "assistant".into(),
                    text: format!("[工具调用: {}]", tools.join(", ")),
                    timestamp,
                });
            }
        }
    }
    None
}

fn parse_chat_record(line: &[u8], provider: Provider) -> Option<ChatMsg> {
    let event: serde_json::Value = serde_json::from_slice(line).ok()?;
    match provider {
        Provider::Claude => parse_claude_chat_msg(&event),
        Provider::Codex => parse_codex_chat_msg(&event),
    }
}

fn hash_file_range(file: &mut std::fs::File, start: u64, len: u64) -> std::io::Result<ContentHash> {
    file.seek(SeekFrom::Start(start))?;
    let mut remaining = len;
    let mut hasher = ContentHasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    while remaining > 0 {
        let wanted = remaining.min(buffer.len() as u64) as usize;
        file.read_exact(&mut buffer[..wanted])?;
        #[cfg(test)]
        record_cache_test_read(wanted);
        hasher.update(&buffer[..wanted]);
        remaining -= wanted as u64;
    }
    Ok(*hasher.finalize().as_bytes())
}

fn create_snapshot_probes(file: &mut std::fs::File, len: u64) -> std::io::Result<SnapshotProbes> {
    const PROBE_LIMIT: u64 = 64 * 1024;
    let head_len = len.min(PROBE_LIMIT);
    let tail_len = head_len;
    let tail_start = len - tail_len;
    Ok(SnapshotProbes {
        head_len,
        head_hash: hash_file_range(file, 0, head_len)?,
        tail_start,
        tail_len,
        tail_hash: hash_file_range(file, tail_start, tail_len)?,
    })
}

fn snapshot_probes_match(
    file: &mut std::fs::File,
    probes: &SnapshotProbes,
) -> std::io::Result<bool> {
    if hash_file_range(file, 0, probes.head_len)? != probes.head_hash {
        return Ok(false);
    }
    Ok(hash_file_range(file, probes.tail_start, probes.tail_len)? == probes.tail_hash)
}

fn create_message_snapshot(file: &mut std::fs::File, len: u64) -> std::io::Result<MessageSnapshot> {
    let (identity, change_stamp) = file_identity_and_change(file)?;
    let probes = create_snapshot_probes(file, len)?;
    let content_hash = hash_file_range(file, 0, len)?;
    Ok(MessageSnapshot {
        identity,
        change_stamp,
        len,
        content_hash,
        probes,
    })
}

fn encode_message_snapshot(snapshot: &MessageSnapshot) -> std::io::Result<String> {
    let json = serde_json::to_vec(snapshot).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cannot encode message snapshot: {error}"),
        )
    })?;
    Ok(URL_SAFE_NO_PAD.encode(json))
}

fn decode_message_snapshot(encoded: &str) -> std::io::Result<MessageSnapshot> {
    if encoded.is_empty() || encoded.len() > 4096 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid message snapshot token size",
        ));
    }
    let json = URL_SAFE_NO_PAD.decode(encoded).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid message snapshot token: {error}"),
        )
    })?;
    let snapshot: MessageSnapshot = serde_json::from_slice(&json).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("invalid message snapshot contents: {error}"),
        )
    })?;
    let expected_probe_len = snapshot.len.min(64 * 1024);
    if snapshot.probes.head_len != expected_probe_len
        || snapshot.probes.tail_len != expected_probe_len
        || snapshot.probes.tail_start != snapshot.len - expected_probe_len
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "message snapshot probe bounds do not match its source length",
        ));
    }
    Ok(snapshot)
}

/// Validates a fixed snapshot by hashing its complete prefix. A normal append
/// keeps the original pages available; any middle rewrite is rejected even when
/// it falls outside the legacy edge probes and is followed by more records.
fn validate_message_snapshot(
    file: &mut std::fs::File,
    snapshot: &MessageSnapshot,
    current_len: u64,
) -> std::io::Result<String> {
    let (identity, change_stamp) = file_identity_and_change(file)?;
    if identity != snapshot.identity {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "session file was replaced between message pages",
        ));
    }
    if current_len < snapshot.len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "JSONL file was truncated between message pages",
        ));
    }
    if current_len == snapshot.len && change_stamp != snapshot.change_stamp {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "session file changed between message pages; reload the conversation",
        ));
    }
    // Keep the cheap probes as an early rejection, then validate every byte.
    // ChangeTime can tie on Windows and edge probes cannot see middle rewrites.
    if !snapshot_probes_match(file, &snapshot.probes)?
        || hash_file_range(file, 0, snapshot.len)? != snapshot.content_hash
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "session file prefix changed between message pages; reload the conversation",
        ));
    }
    Ok(change_stamp)
}

/// Reads newline-delimited JSON backwards in bounded chunks. The callback is
/// invoked newest-record first and can stop the scan by returning false.
/// Returns true only when every record through the beginning was examined.
#[cfg(test)]
fn scan_jsonl_reverse<F>(file: &mut std::fs::File, consume: F) -> std::io::Result<bool>
where
    F: FnMut(&[u8]) -> bool,
{
    let snapshot_len = file.metadata()?.len();
    scan_jsonl_reverse_from(file, snapshot_len, consume)
}

fn consume_reverse_line<F>(
    prefix: &mut Vec<u8>,
    pending: &mut Vec<Vec<u8>>,
    consume: &mut F,
) -> bool
where
    F: FnMut(&[u8]) -> bool,
{
    if prefix.is_empty() && pending.is_empty() {
        return true;
    }
    let keep_reading = if pending.is_empty() {
        consume(prefix)
    } else {
        let capacity = prefix.len() + pending.iter().map(Vec::len).sum::<usize>();
        let mut line = Vec::with_capacity(capacity);
        line.append(prefix);
        // Chunks are discovered newest-to-oldest; reverse them exactly once.
        for mut part in pending.drain(..).rev() {
            line.append(&mut part);
        }
        consume(&line)
    };
    pending.clear();
    keep_reading
}

fn scan_jsonl_reverse_from<F>(
    file: &mut std::fs::File,
    snapshot_len: u64,
    mut consume: F,
) -> std::io::Result<bool>
where
    F: FnMut(&[u8]) -> bool,
{
    const CHUNK_SIZE: u64 = 256 * 1024;
    if snapshot_len > file.metadata()?.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "JSONL file was truncated during export",
        ));
    }
    let mut end = snapshot_len;
    // A record split across reads owns each chunk separately. Never prepend the
    // accumulated suffix to every older chunk: that would copy one long record
    // quadratically even though disk reads themselves use bounded chunks.
    let mut pending: Vec<Vec<u8>> = Vec::new();
    while end > 0 {
        let start = end.saturating_sub(CHUNK_SIZE);
        let mut buf = vec![0_u8; (end - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut buf)?;
        #[cfg(test)]
        record_cache_test_read(buf.len());

        if let Some(last_newline) = buf.iter().rposition(|byte| *byte == b'\n') {
            let mut line_prefix = buf.split_off(last_newline + 1);
            if !consume_reverse_line(&mut line_prefix, &mut pending, &mut consume) {
                return Ok(false);
            }
            let mut segment_end = last_newline;
            while let Some(newline) = buf[..segment_end].iter().rposition(|byte| *byte == b'\n') {
                let line = &buf[newline + 1..segment_end];
                if !line.is_empty() && !consume(line) {
                    return Ok(false);
                }
                segment_end = newline;
            }
            buf.truncate(segment_end);
            if start == 0 {
                if !consume_reverse_line(&mut buf, &mut pending, &mut consume) {
                    return Ok(false);
                }
                return Ok(true);
            }
            if !buf.is_empty() {
                pending.push(buf);
            }
        } else if start == 0 {
            if !consume_reverse_line(&mut buf, &mut pending, &mut consume) {
                return Ok(false);
            }
            return Ok(true);
        } else {
            pending.push(buf);
        }
        end = start;
    }
    Ok(true)
}

struct ReversePageCollector {
    provider: Provider,
    offset: usize,
    limit: usize,
    skipped: usize,
    messages: Vec<ChatMsg>,
    has_more: bool,
}

impl ReversePageCollector {
    fn consume(&mut self, line: &[u8]) -> bool {
        let Some(message) = parse_chat_record(line, self.provider) else {
            return true;
        };
        if self.skipped < self.offset {
            self.skipped += 1;
            return true;
        }
        if self.messages.len() < self.limit {
            self.messages.push(message);
            return true;
        }
        self.has_more = true;
        false
    }
}

#[cfg(test)]
fn empty_messages_page() -> MessagesPage {
    MessagesPage {
        messages: vec![],
        total: 0,
        offset: 0,
        next_offset: 0,
        total_exact: true,
        has_more: false,
        snapshot_len: 0,
        snapshot_token: String::new(),
    }
}

fn read_messages_page_snapshot(
    file: &mut std::fs::File,
    snapshot_len: u64,
    offset: usize,
    limit: usize,
    provider: Provider,
) -> std::io::Result<MessagesPage> {
    let mut collector = ReversePageCollector {
        provider,
        offset,
        limit: if limit == 0 { 200 } else { limit.min(1000) },
        skipped: 0,
        messages: Vec::new(),
        has_more: false,
    };
    let total_exact = scan_jsonl_reverse_from(file, snapshot_len, |line| collector.consume(line))?;
    collector.messages.reverse();
    let next_offset = collector.skipped + collector.messages.len();
    let total = next_offset + usize::from(collector.has_more);
    Ok(MessagesPage {
        messages: collector.messages,
        total,
        offset: next_offset,
        next_offset,
        total_exact,
        has_more: collector.has_more,
        snapshot_len,
        snapshot_token: String::new(),
    })
}

fn try_read_messages_page(
    path: &Path,
    snapshot_len: Option<u64>,
    snapshot_token: Option<String>,
    offset: usize,
    limit: usize,
    provider: Provider,
) -> std::io::Result<MessagesPage> {
    let mut file = std::fs::File::open(path)?;
    let current_len = file.metadata()?.len();
    let (snapshot, validated_change) = match snapshot_token {
        Some(encoded) => {
            let snapshot = decode_message_snapshot(&encoded)?;
            if snapshot_len.is_some_and(|length| length != snapshot.len) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "message snapshot length does not match its token",
                ));
            }
            let change = validate_message_snapshot(&mut file, &snapshot, current_len)?;
            (snapshot, change)
        }
        None => {
            if snapshot_len.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "a fixed message page requires its snapshot token",
                ));
            }
            let snapshot = create_message_snapshot(&mut file, current_len)?;
            let change = snapshot.change_stamp.clone();
            (snapshot, change)
        }
    };
    let encoded = encode_message_snapshot(&snapshot)?;
    let mut page = read_messages_page_snapshot(&mut file, snapshot.len, offset, limit, provider)?;

    // Never return a token for a page that raced an in-place writer. Appends are
    // also retried conservatively, keeping the success path simple to reason about.
    let (identity_after, change_after) = file_identity_and_change(&file)?;
    if identity_after != snapshot.identity
        || change_after != validated_change
        || file.metadata()?.len() != current_len
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "session file changed while reading a message page; retry",
        ));
    }
    page.snapshot_token = encoded;
    Ok(page)
}

#[cfg(test)]
fn read_messages_page(
    path: &Path,
    offset: usize,
    limit: usize,
    provider: Provider,
) -> MessagesPage {
    try_read_messages_page(path, None, None, offset, limit, provider)
        .unwrap_or_else(|_| empty_messages_page())
}

#[derive(Debug, Serialize)]
struct ExportResult {
    file_path: String,
    message_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    cleanup_warning: Option<String>,
}

struct ExportWorkspace {
    path: PathBuf,
}

fn remove_export_workspace(path: &Path) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("导出工作区出现意外目录或链接: {}", entry.path().display()),
            ));
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

impl ExportWorkspace {
    fn new(output_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(output_dir)
            .map_err(|e| format!("无法创建导出目录 {}: {e}", output_dir.display()))?;
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_err(|e| format!("无法生成导出工作区标识: {e}"))?
            .as_nanos();
        for attempt in 0..100 {
            let path = output_dir.join(format!(
                ".csm-export-{}-{unique}-{attempt}",
                std::process::id()
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    let workspace = Self { path };
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        if let Err(error) = std::fs::set_permissions(
                            &workspace.path,
                            std::fs::Permissions::from_mode(0o700),
                        ) {
                            let operation_error = format!("无法保护导出工作区: {error}");
                            return Err(match workspace.cleanup() {
                                Ok(()) => operation_error,
                                Err(cleanup_error) => {
                                    format!("{operation_error}；{cleanup_error}")
                                }
                            });
                        }
                    }
                    return Ok(workspace);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("无法创建导出工作区: {error}")),
            }
        }
        Err("无法分配唯一导出工作区".into())
    }

    fn cleanup(mut self) -> Result<(), String> {
        let mut last_error = None;
        for attempt in 0..6 {
            match remove_export_workspace(&self.path) {
                Ok(()) => {
                    self.path = PathBuf::new();
                    return Ok(());
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.path = PathBuf::new();
                    return Ok(());
                }
                Err(error) => last_error = Some(error),
            }
            if attempt < 5 {
                std::thread::sleep(std::time::Duration::from_millis(100 * (attempt + 1)));
            }
        }
        Err(format!(
            "临时导出目录清理失败，请手动删除 {}: {}",
            self.path.display(),
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "未知错误".into())
        ))
    }
}

impl Drop for ExportWorkspace {
    fn drop(&mut self) {
        if !self.path.as_os_str().is_empty() {
            let _ = remove_export_workspace(&self.path);
        }
    }
}

fn remove_export_file_with_retry(path: &Path) -> Result<(), String> {
    let mut last_error = None;
    for attempt in 0..6 {
        match std::fs::remove_file(path) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        if attempt < 5 {
            std::thread::sleep(std::time::Duration::from_millis(100 * (attempt + 1)));
        }
    }
    Err(format!(
        "未完成的导出文件清理失败，请手动删除 {}: {}",
        path.display(),
        last_error
            .map(|error| error.to_string())
            .unwrap_or_else(|| "未知错误".into())
    ))
}

/// Exclusively reserves the visible destination. Failed exports explicitly
/// remove their partial file; successful exports disarm the fallback `Drop`.
struct ExportDestination {
    path: PathBuf,
    file: Option<std::fs::File>,
    disarmed: bool,
}

impl ExportDestination {
    fn new(output_dir: &Path, base: &str, session_id: &str) -> Result<Self, String> {
        for suffix in 0..1000 {
            let suffix = if suffix == 0 {
                String::new()
            } else {
                format!("-{suffix}")
            };
            let path = output_dir.join(format!("{base}-{}{suffix}.md", &session_id[..8]));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        disarmed: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!("创建导出目标 {} 失败: {error}", path.display()));
                }
            }
        }
        Err("无法生成不重复的导出文件名".into())
    }

    fn file_mut(&mut self) -> &mut std::fs::File {
        self.file
            .as_mut()
            .expect("active export destination retains its file")
    }

    fn commit(mut self) -> PathBuf {
        self.file.take();
        self.disarmed = true;
        self.path.clone()
    }

    fn cleanup(mut self) -> Result<(), String> {
        self.file.take();
        let result = remove_export_file_with_retry(&self.path);
        if result.is_ok() {
            self.disarmed = true;
        }
        result
    }
}

impl Drop for ExportDestination {
    fn drop(&mut self) {
        self.file.take();
        if !self.disarmed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn combine_export_failure(
    operation_error: String,
    destination_cleanup: Option<String>,
    workspace_cleanup: Option<String>,
) -> String {
    [
        Some(operation_error),
        destination_cleanup,
        workspace_cleanup,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("；")
}

fn write_export_page(
    workspace: &ExportWorkspace,
    page_paths: &mut Vec<PathBuf>,
    messages: &[ChatMsg],
    provider: Provider,
) -> Result<(), String> {
    if messages.is_empty() {
        return Ok(());
    }
    let page_path = workspace
        .path
        .join(format!("page-{:08}.md", page_paths.len()));
    let mut writer = std::io::BufWriter::new(
        std::fs::File::create(&page_path).map_err(|e| format!("创建导出片段失败: {e}"))?,
    );
    for message in messages {
        let role = if message.role == "user" {
            "You"
        } else {
            match provider {
                Provider::Claude => "Claude",
                Provider::Codex => "Codex",
            }
        };
        writeln!(writer, "## {role}\n").map_err(|e| format!("写入导出片段失败: {e}"))?;
        writeln!(writer, "{}\n", message.text).map_err(|e| format!("写入导出片段失败: {e}"))?;
    }
    writer
        .flush()
        .map_err(|e| format!("保存导出片段失败: {e}"))?;
    page_paths.push(page_path);
    Ok(())
}

#[cfg(test)]
static EXPORT_TEST_AFTER_SCAN: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

#[cfg(test)]
fn run_export_test_after_scan() {
    let callback = EXPORT_TEST_AFTER_SCAN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    if let Some(callback) = callback {
        callback();
    }
}

fn export_markdown_from_file(
    provider: Provider,
    session_id: &str,
    title: &str,
    project_path: &str,
    updated_at: &str,
    source_path: &Path,
    output_dir: &Path,
) -> Result<ExportResult, String> {
    if !is_valid_uuid(session_id) {
        return Err("导出会话 ID 无效".into());
    }
    let mut source = std::fs::File::open(source_path)
        .map_err(|e| format!("无法打开导出会话 {}: {e}", source_path.display()))?;
    let snapshot_len = source
        .metadata()
        .map_err(|e| format!("无法读取导出会话长度: {e}"))?
        .len();
    let snapshot = create_message_snapshot(&mut source, snapshot_len)
        .map_err(|e| format!("无法固定导出会话快照: {e}"))?;
    let workspace = ExportWorkspace::new(output_dir)?;
    let mut destination = None;
    let operation = (|| -> Result<(PathBuf, usize), String> {
        let mut page_paths = Vec::new();
        let mut page_messages = Vec::with_capacity(1000);
        let mut message_count = 0usize;
        let mut export_error = None;
        scan_jsonl_reverse_from(&mut source, snapshot_len, |line| {
            let Some(message) = parse_chat_record(line, provider) else {
                return true;
            };
            page_messages.push(message);
            message_count += 1;
            if page_messages.len() == 1000 {
                page_messages.reverse();
                if let Err(error) =
                    write_export_page(&workspace, &mut page_paths, &page_messages, provider)
                {
                    export_error = Some(error);
                    return false;
                }
                page_messages.clear();
            }
            true
        })
        .map_err(|e| format!("读取导出内容失败: {e}"))?;
        if let Some(error) = export_error {
            return Err(error);
        }
        #[cfg(test)]
        run_export_test_after_scan();
        page_messages.reverse();
        write_export_page(&workspace, &mut page_paths, &page_messages, provider)?;

        let mut base = title
            .chars()
            .map(|character| {
                if character.is_control() || "/\\:*?\"<>|".contains(character) {
                    '_'
                } else {
                    character
                }
            })
            .take(40)
            .collect::<String>();
        base = base.trim_matches([' ', '.']).to_string();
        if base.is_empty() {
            base = "session".into();
        }
        destination = Some(ExportDestination::new(output_dir, &base, session_id)?);

        let provider_name = match provider {
            Provider::Claude => "Claude",
            Provider::Codex => "Codex",
        };
        {
            let output = destination
                .as_mut()
                .expect("export destination was just reserved");
            let mut writer = std::io::BufWriter::new(output.file_mut());
            writeln!(writer, "# {title}\n").map_err(|e| format!("写入导出标题失败: {e}"))?;
            writeln!(writer, "> Session: {session_id}  ")
                .and_then(|_| writeln!(writer, "> Provider: {provider_name}  "))
                .and_then(|_| writeln!(writer, "> Project: {project_path}  "))
                .and_then(|_| writeln!(writer, "> Last active: {updated_at}\n"))
                .map_err(|e| format!("写入导出元数据失败: {e}"))?;
            for page_path in page_paths.iter().rev() {
                let mut page = std::fs::File::open(page_path)
                    .map_err(|e| format!("重新读取导出片段失败: {e}"))?;
                std::io::copy(&mut page, &mut writer)
                    .map_err(|e| format!("合并导出片段失败: {e}"))?;
            }
            writer
                .flush()
                .map_err(|e| format!("保存完整导出文件失败: {e}"))?;
        }
        let current_len = source
            .metadata()
            .map_err(|e| format!("提交导出前无法复核源文件: {e}"))?
            .len();
        validate_message_snapshot(&mut source, &snapshot, current_len)
            .map_err(|e| format!("导出期间源会话发生变化，未提交不完整文件: {e}"))?;
        let output_path = destination
            .take()
            .expect("committed export destination exists")
            .commit();
        Ok((output_path, message_count))
    })();

    let destination_cleanup = match (&operation, destination) {
        (Err(_), Some(destination)) => destination.cleanup().err(),
        _ => None,
    };
    let workspace_cleanup = workspace.cleanup().err();
    match operation {
        Ok((output_path, message_count)) => {
            debug_log(&format!(
                "export_session_markdown provider={} messages={message_count}",
                provider.as_str()
            ));
            Ok(ExportResult {
                file_path: output_path.to_string_lossy().into_owned(),
                message_count,
                cleanup_warning: workspace_cleanup,
            })
        }
        Err(error) => Err(combine_export_failure(
            error,
            destination_cleanup,
            workspace_cleanup,
        )),
    }
}

fn read_validated_messages_page(
    provider: Provider,
    session_id: &str,
    _project_path: &str,
    file_path: &str,
    snapshot: MessageSnapshotCursor,
    offset: usize,
    limit: usize,
) -> Result<MessagesPage, String> {
    let file_path = validate_session_file(provider, session_id, file_path)?;
    try_read_messages_page(
        &file_path,
        snapshot.len,
        snapshot.token,
        offset,
        limit,
        provider,
    )
    .map_err(|e| format!("读取会话消息失败: {e}"))
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Flat arguments are the versioned frontend IPC contract.
async fn get_messages(
    session_id: String,
    project_path: String,
    file_path: String,
    snapshot_len: Option<u64>,
    snapshot_token: Option<String>,
    offset: usize,
    limit: usize,
    provider: Option<String>,
) -> Result<MessagesPage, String> {
    let provider = Provider::parse(provider.as_deref())?;
    tokio::task::spawn_blocking(move || {
        read_validated_messages_page(
            provider,
            &session_id,
            &project_path,
            &file_path,
            MessageSnapshotCursor {
                len: snapshot_len,
                token: snapshot_token,
            },
            offset,
            limit,
        )
    })
    .await
    .map_err(|e| format!("消息读取任务失败: {e}"))?
}

#[tauri::command]
async fn export_session_markdown(
    session_id: String,
    project_path: String,
    file_path: String,
    provider: Option<String>,
    title: String,
    updated_at: Option<String>,
) -> Result<ExportResult, String> {
    let provider = Provider::parse(provider.as_deref())?;
    tokio::task::spawn_blocking(move || {
        let file_path = validate_session_file(provider, &session_id, &file_path)?;
        let output_dir = dirs::document_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_default()
            .join("Claude Session Manager Exports");
        export_markdown_from_file(
            provider,
            &session_id,
            &title,
            &project_path,
            updated_at.as_deref().unwrap_or(""),
            &file_path,
            &output_dir,
        )
    })
    .await
    .map_err(|e| format!("导出任务失败: {e}"))?
}

#[tauri::command]
fn toggle_favorite(session_id: String, provider: Option<String>) -> Result<bool, String> {
    let provider = Provider::parse(provider.as_deref())?;
    let key = provider.favorite_key(&session_id);
    let _lock = lock_favorites()?;
    let mut favs = read_favorites()?;
    let now_fav = if favs.contains(&key) {
        favs.remove(&key);
        false
    } else {
        favs.insert(key);
        true
    };
    save_favorites_unlocked(&favs)?;
    Ok(now_fav)
}

#[cfg(target_os = "macos")]
fn running_session_token(
    provider: Provider,
    session_id: &str,
    file_path: &str,
    _source: &str,
) -> Result<Option<String>, String> {
    Ok(match provider {
        Provider::Claude => find_running_session_tty(session_id),
        Provider::Codex => find_codex_session_tty(session_id, file_path),
    })
}

#[cfg(target_os = "windows")]
fn running_session_token(
    provider: Provider,
    session_id: &str,
    file_path: &str,
    source: &str,
) -> Result<Option<String>, String> {
    windows_platform::running_session_token(provider, session_id, file_path, source)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn running_session_token(
    _provider: Provider,
    _session_id: &str,
    _file_path: &str,
    _source: &str,
) -> Result<Option<String>, String> {
    Ok(None)
}

#[cfg(target_os = "macos")]
fn focus_running_session(token: &str) -> Result<bool, String> {
    focus_terminal_tty(token)
}

#[cfg(target_os = "windows")]
fn focus_running_session(token: &str) -> Result<bool, String> {
    windows_platform::focus_running_session(token)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn focus_running_session(_token: &str) -> Result<bool, String> {
    Ok(false)
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq, Eq)]
struct TerminalSingleMetadata {
    window_id: i64,
    tty: String,
}

#[cfg(any(target_os = "macos", test))]
fn parse_terminal_single_result(result: &str) -> Option<TerminalSingleMetadata> {
    let mut fields = result.split_whitespace();
    if fields.next()? != "TERMINAL_SINGLE_OK" || fields.next()? != "WINDOW_ID" {
        return None;
    }
    let window_id = fields.next()?.parse().ok()?;
    if fields.next()? != "TAB_TTY" {
        return None;
    }
    let tty = fields.next()?;
    if !tty.starts_with("/dev/tty") || fields.next()? != "TRUST_CONFIRMED" {
        return None;
    }
    if !matches!(fields.next()?, "true" | "false") || fields.next().is_some() {
        return None;
    }
    Some(TerminalSingleMetadata {
        window_id,
        tty: tty.to_string(),
    })
}

#[cfg(target_os = "macos")]
struct MacTerminalLaunch {
    provider: Provider,
    session_id: String,
    command: String,
}

#[cfg(any(target_os = "macos", test))]
fn unconfirmed_launch_error(requested: usize, confirmed: usize) -> String {
    format!(
        "Terminal 请求已发送，但只核实到 {confirmed}/{requested} 个本次新 TTY 上具有精确 Provider、恢复参数和 session ID 的进程。CLI 可能不存在、恢复可能已立即退出，或 Trust 尚未确认；未报告打开成功。已打开内容会保留，未执行自动清理"
    )
}

#[cfg(any(target_os = "macos", test))]
const MACOS_PROVIDER_STABLE_FOR: std::time::Duration = std::time::Duration::from_millis(1000);

/// A process can be visible in one snapshot immediately before exiting. Require
/// the complete launch set to remain present for a short interval, and restart
/// that interval whenever any observation is incomplete.
#[cfg(any(target_os = "macos", test))]
fn macos_stable_launch_confirmed(
    stable_since: &mut Option<std::time::Instant>,
    confirmed: usize,
    requested: usize,
    now: std::time::Instant,
) -> bool {
    if requested == 0 || confirmed != requested {
        *stable_since = None;
        return false;
    }
    let started = stable_since.get_or_insert(now);
    now.duration_since(*started) >= MACOS_PROVIDER_STABLE_FOR
}

#[cfg(target_os = "macos")]
fn wait_for_provider_launches(
    launches: &[MacTerminalLaunch],
    expected_ttys: &[String],
) -> Result<(), String> {
    if launches.is_empty() || launches.len() != expected_ttys.len() {
        return Err("Terminal 启动元数据与所选会话数量不一致".into());
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    let mut stable_since = None;
    loop {
        let system = sysinfo::System::new_all();
        let process_ttys = terminal_process_ttys();
        let confirmed = launches
            .iter()
            .zip(expected_ttys)
            .filter(|(launch, expected_tty)| {
                system.processes().values().any(|process| {
                    provider_process::process_matches_resume(
                        process,
                        launch.provider,
                        &launch.session_id,
                    ) && process_tty(process, &process_ttys).as_ref() == Some(*expected_tty)
                })
            })
            .count();
        let now = std::time::Instant::now();
        if macos_stable_launch_confirmed(&mut stable_since, confirmed, launches.len(), now) {
            debug_log(&format!(
                "macos_provider_launch verified={confirmed} requested={} stable_ms={}",
                launches.len(),
                MACOS_PROVIDER_STABLE_FOR.as_millis()
            ));
            return Ok(());
        }
        if now >= deadline {
            return Err(unconfirmed_launch_error(launches.len(), confirmed));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(target_os = "macos")]
fn open_stopped_session(
    provider: Provider,
    session_id: &str,
    project_path: &str,
    file_path: &str,
    _title: Option<String>,
) -> Result<String, String> {
    let command = session_launch_command_for(provider, session_id, project_path, file_path)?;
    require_accessibility_permission()?;
    let script = build_terminal_single_script(&command);
    let result = run_applescript(&script)?;
    let metadata = parse_terminal_single_result(&result)
        .ok_or_else(|| format!("Terminal 未确认单会话窗口与 TTY: {result}"))?;
    debug_log(&format!(
        "macos_single_terminal window={} tty={}",
        metadata.window_id, metadata.tty
    ));
    let launch = MacTerminalLaunch {
        provider,
        session_id: session_id.to_string(),
        command,
    };
    wait_for_provider_launches(&[launch], &[metadata.tty])?;
    Ok("opened".into())
}

#[cfg(any(target_os = "windows", test))]
fn windows_single_session_title(
    provider: Provider,
    session_id: &str,
    title: Option<String>,
) -> String {
    title
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| format!("{} {}", provider.as_str(), &session_id[..8]))
}

#[cfg(target_os = "windows")]
fn open_stopped_session(
    provider: Provider,
    session_id: &str,
    project_path: &str,
    file_path: &str,
    title: Option<String>,
) -> Result<String, String> {
    let launch = validate_session_launch(provider, session_id, project_path, file_path)?;
    let title = windows_single_session_title(provider, session_id, title);
    let spec = windows_platform::WindowsLaunchSpec::new(launch, &title)?;
    windows_platform::open_launches(&[spec])?;
    Ok("opened".into())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn open_stopped_session(
    _provider: Provider,
    _session_id: &str,
    _project_path: &str,
    _file_path: &str,
    _title: Option<String>,
) -> Result<String, String> {
    Err("当前构建只支持 macOS 和 Windows".into())
}

type SessionLaunchKey = (String, String);

#[cfg(any(target_os = "windows", test))]
static BUSY_SESSION_LAUNCHES: LazyLock<Mutex<HashSet<SessionLaunchKey>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
#[cfg(any(target_os = "windows", test))]
static SESSION_LAUNCH_AVAILABLE: Condvar = Condvar::new();

fn session_launch_key(provider: Provider, session_id: &str) -> SessionLaunchKey {
    (
        provider.as_str().to_string(),
        session_id.to_ascii_lowercase(),
    )
}

/// Owns a sorted set of busy keys, not a set of borrowed mutex guards. This
/// supports a multi-session batch while allowing unrelated providers/sessions
/// to proceed concurrently. The running-process check happens after acquisition.
#[cfg(any(target_os = "windows", test))]
struct SessionLaunchGuards {
    keys: Vec<SessionLaunchKey>,
}

#[cfg(any(target_os = "windows", test))]
impl SessionLaunchGuards {
    fn acquire(mut keys: Vec<SessionLaunchKey>) -> Self {
        keys.sort();
        keys.dedup();
        let mut busy = BUSY_SESSION_LAUNCHES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        while keys.iter().any(|key| busy.contains(key)) {
            busy = SESSION_LAUNCH_AVAILABLE
                .wait(busy)
                .unwrap_or_else(|error| error.into_inner());
        }
        busy.extend(keys.iter().cloned());
        Self { keys }
    }
}

#[cfg(any(target_os = "windows", test))]
impl Drop for SessionLaunchGuards {
    fn drop(&mut self) {
        let mut busy = BUSY_SESSION_LAUNCHES
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for key in &self.keys {
            busy.remove(key);
        }
        drop(busy);
        SESSION_LAUNCH_AVAILABLE.notify_all();
    }
}

fn open_session_blocking(
    session_id: String,
    project_path: String,
    file_path: String,
    provider: Option<String>,
    source: Option<String>,
    title: Option<String>,
) -> Result<String, String> {
    let provider = Provider::parse(provider.as_deref())?;
    #[cfg(target_os = "windows")]
    let _launch_guard =
        SessionLaunchGuards::acquire(vec![session_launch_key(provider, &session_id)]);
    let source = source.unwrap_or_default();
    if let Some(token) = running_session_token(provider, &session_id, &file_path, &source)? {
        if focus_running_session(&token)? {
            return Ok("focused".into());
        }
        return Err(format!(
            "会话 {session_id} 正在运行，但未找到可聚焦的 Terminal 标签页；未重复启动"
        ));
    }
    open_stopped_session(provider, &session_id, &project_path, &file_path, title)
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn open_session(
    session_id: String,
    project_path: String,
    file_path: String,
    provider: Option<String>,
    source: Option<String>,
    title: Option<String>,
) -> Result<String, String> {
    // In-process OSAScript must retain Tauri's synchronous main-thread command
    // path. Moving `do script` to a blocking worker can leave the command
    // unexecuted at the shell prompt or time out a later Apple Event.
    open_session_blocking(session_id, project_path, file_path, provider, source, title)
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
async fn open_session(
    session_id: String,
    project_path: String,
    file_path: String,
    provider: Option<String>,
    source: Option<String>,
    title: Option<String>,
) -> Result<String, String> {
    // Process enumeration and the stable-launch interval can take many seconds.
    // Keep the WebView2/main thread responsive on Windows.
    tokio::task::spawn_blocking(move || {
        open_session_blocking(session_id, project_path, file_path, provider, source, title)
    })
    .await
    .map_err(|error| format!("会话启动任务失败: {error}"))?
}

#[cfg(any(target_os = "macos", test))]
fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('\'', "\\'");
    format!("$'{}'", escaped)
}

#[cfg(any(target_os = "macos", test))]
fn applescript_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "macos")]
fn terminal_process_ttys() -> HashMap<u32, String> {
    let output = Command::new("ps").args(["-axo", "pid=,tty="]).output();
    let Ok(output) = output else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse::<u32>().ok()?;
            let tty = fields.next()?;
            if tty == "??" || tty == "?" {
                return None;
            }
            Some((pid, format!("/dev/{tty}")))
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn process_tty(process: &sysinfo::Process, ttys: &HashMap<u32, String>) -> Option<String> {
    ttys.get(&process.pid().as_u32()).cloned()
}

#[cfg(target_os = "macos")]
fn find_resumed_session_tty_in(
    system: &sysinfo::System,
    ttys: &HashMap<u32, String>,
    provider: Provider,
    session_id: &str,
) -> Option<String> {
    system.processes().values().find_map(|process| {
        if provider_process::process_matches_resume(process, provider, session_id) {
            process_tty(process, ttys)
        } else {
            None
        }
    })
}

#[cfg(target_os = "macos")]
fn find_running_session_tty_in(
    sessions_dir: &Path,
    system: &sysinfo::System,
    ttys: &HashMap<u32, String>,
    session_id: &str,
) -> Option<String> {
    // Missing or unreadable session records must not disable the exact live
    // provider argv fallback below.
    let entries = std::fs::read_dir(sessions_dir).ok().into_iter().flatten();
    let mut candidates = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                return None;
            }
            let value: serde_json::Value = std::fs::read_to_string(&path).ok()?.parse().ok()?;
            if value.get("sessionId")?.as_str()? != session_id {
                return None;
            }
            let pid = value
                .get("pid")
                .and_then(|v| v.as_u64())
                .or_else(|| path.file_stem()?.to_string_lossy().parse().ok())?;
            let pid = u32::try_from(pid).ok()?;
            let updated = value.get("updatedAt").and_then(|v| v.as_i64()).unwrap_or(0);
            let started_at = value.get("startedAt").and_then(|v| v.as_i64());
            Some((updated, pid, started_at))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    for (_, pid, started_at) in candidates {
        let Some(process) = system.process(sysinfo::Pid::from_u32(pid)) else {
            continue;
        };
        if provider_process::recorded_claude_process_matches(process, session_id, started_at) {
            if let Some(tty) = process_tty(process, ttys) {
                return Some(tty);
            }
        }
    }
    find_resumed_session_tty_in(system, ttys, Provider::Claude, session_id)
}

#[cfg(target_os = "macos")]
fn find_running_session_tty(session_id: &str) -> Option<String> {
    let sessions_dir = claude_dir().join("sessions");
    let system = sysinfo::System::new_all();
    let ttys = terminal_process_ttys();
    find_running_session_tty_in(&sessions_dir, &system, &ttys, session_id)
}

#[cfg(target_os = "macos")]
fn find_codex_running_session_tty(session_id: &str) -> Option<String> {
    let system = sysinfo::System::new_all();
    let ttys = terminal_process_ttys();
    find_resumed_session_tty_in(&system, &ttys, Provider::Codex, session_id)
}

#[cfg(target_os = "macos")]
fn find_codex_session_tty(session_id: &str, file_path: &str) -> Option<String> {
    if let Some(tty) = find_codex_running_session_tty(session_id) {
        return Some(tty);
    }
    // Codex sessions not originally launched with `resume <id>` do not carry
    // their session ID in argv. An open rollout FD identifies the session file;
    // structured provider identity and the current TTY are still required.
    let output = Command::new("lsof")
        .args(["-F", "p", file_path])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let system = sysinfo::System::new_all();
    let ttys = terminal_process_ttys();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some(pid) = line.strip_prefix('p') else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let Some(process) = system.process(sysinfo::Pid::from_u32(pid)) else {
            continue;
        };
        if provider_process::process_is_provider(process, Provider::Codex) {
            if let Some(tty) = process_tty(process, &ttys) {
                return Some(tty);
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn focus_terminal_tty(tty: &str) -> Result<bool, String> {
    let script = format!(
        r##"tell application "Terminal"
set matchingWindows to {{}}
repeat with w in windows
  try
    if tty of selected tab of w is "{tty}" then set end of matchingWindows to w
  end try
end repeat
if (count of matchingWindows) is not 1 then return "TERMINAL_TAB_NOT_FOUND"
set targetWindow to item 1 of matchingWindows
set targetWindowId to id of targetWindow
activate
set selected of selected tab of targetWindow to true
set index of targetWindow to 1
set frontmost of targetWindow to true
repeat 25 times
  delay 0.2
  if (id of front window) is targetWindowId then
    if selected of selected tab of targetWindow then return "TERMINAL_TAB_FOCUSED"
  end if
end repeat
return "TERMINAL_TAB_NOT_FRONTMOST id=" & (id of targetWindow) & " index=" & (index of targetWindow) & " frontmost=" & (frontmost of targetWindow) & " selected=" & (selected of selected tab of targetWindow)
end tell"##
    );
    let result = run_applescript(&script)?;
    debug_log(&format!("focus_terminal_tty tty={tty} result={result}"));
    Ok(result.starts_with("TERMINAL_TAB_FOCUSED"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ValidatedLaunch {
    provider: Provider,
    session_id: String,
    cwd: PathBuf,
    file_path: PathBuf,
}

fn validated_launch_cwd(project_path: &str) -> Result<PathBuf, String> {
    if project_path.is_empty() {
        return dirs::home_dir().ok_or_else(|| "无法确定默认启动目录".into());
    }
    let path = PathBuf::from(project_path);
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("项目目录不可用，未回退到 home: {project_path} ({error})"))?;
    if !metadata.is_dir() {
        return Err(format!("项目路径不是目录，未回退到 home: {project_path}"));
    }
    // Keep the user's Windows verbatim path spelling intact for PowerShell.
    Ok(path)
}

fn validate_session_file(
    provider: Provider,
    session_id: &str,
    file_path: &str,
) -> Result<PathBuf, String> {
    if !is_valid_uuid(session_id) {
        return Err(format!("无效的会话 ID: {session_id}"));
    }
    if file_path.contains(['\n', '\r']) {
        return Err("会话路径不能包含换行符".into());
    }

    let file_path = match provider {
        Provider::Claude => {
            let transcript = std::fs::canonicalize(file_path)
                .map_err(|e| format!("无法定位会话文件 {file_path}: {e}"))?;
            if transcript.extension().and_then(|e| e.to_str()) != Some("jsonl")
                || transcript.file_stem().and_then(|s| s.to_str()) != Some(session_id)
            {
                return Err("会话文件与 session ID 不匹配".into());
            }
            let projects_root = claude_dir().join("projects");
            let project_roots = discover_claude_project_roots(&projects_root)?;
            if !project_roots
                .iter()
                .any(|root| transcript.starts_with(&root.canonical_path))
            {
                return Err("会话文件不在当前 Claude projects 或显式项目链接目标内".into());
            }
            if std::env::var("CLAUDE_CONFIG_DIR").is_ok_and(|path| path.contains(['\n', '\r'])) {
                return Err("CLAUDE_CONFIG_DIR 不能包含换行符".into());
            }
            transcript
        }
        Provider::Codex => {
            let rollout = std::fs::canonicalize(file_path)
                .map_err(|e| format!("无法定位 Codex rollout 文件 {file_path}: {e}"))?;
            let codex_root = std::fs::canonicalize(codex_dir())
                .map_err(|e| format!("无法定位 Codex 数据目录: {e}"))?;
            if rollout.extension().and_then(|v| v.to_str()) != Some("jsonl")
                || !rollout
                    .file_stem()
                    .and_then(|v| v.to_str())
                    .is_some_and(|stem| {
                        stem == session_id || stem.ends_with(&format!("-{session_id}"))
                    })
                || !rollout.starts_with(&codex_root)
            {
                return Err("Codex rollout 文件与 session ID 或数据目录不匹配".into());
            }
            if std::env::var("CODEX_HOME").is_ok_and(|path| path.contains(['\n', '\r'])) {
                return Err("CODEX_HOME 不能包含换行符".into());
            }
            rollout
        }
    };

    Ok(file_path)
}

fn validate_session_launch(
    provider: Provider,
    session_id: &str,
    project_path: &str,
    file_path: &str,
) -> Result<ValidatedLaunch, String> {
    if project_path.contains(['\n', '\r']) {
        return Err("会话路径不能包含换行符".into());
    }
    let file_path = validate_session_file(provider, session_id, file_path)?;
    let cwd = validated_launch_cwd(project_path)?;
    Ok(ValidatedLaunch {
        provider,
        session_id: session_id.to_string(),
        cwd,
        file_path,
    })
}

#[cfg(any(target_os = "macos", test))]
fn session_launch_command(
    session_id: &str,
    project_path: &str,
    file_path: &str,
) -> Result<String, String> {
    let launch = validate_session_launch(Provider::Claude, session_id, project_path, file_path)?;
    let cli = provider_cli::resolve(Provider::Claude)?;
    let cli = unicode_launch_path(&cli, "Claude CLI")?;
    let cwd = unicode_launch_path(&launch.cwd, "Claude 项目目录")?;
    let runtime_prefix = provider_runtime_launch_prefix()?;
    let config_prefix = provider_config_launch_prefix(Provider::Claude)?;
    Ok(format!(
        "cd {} && env -u CLAUDE_CODE_CHILD_SESSION -u CLAUDECODE {}{}{} --dangerously-skip-permissions -r {}",
        shell_quote(cwd),
        runtime_prefix,
        config_prefix,
        shell_quote(cli),
        launch.session_id
    ))
}

#[cfg(any(target_os = "macos", test))]
fn session_launch_command_for(
    provider: Provider,
    session_id: &str,
    project_path: &str,
    file_path: &str,
) -> Result<String, String> {
    match provider {
        Provider::Claude => session_launch_command(session_id, project_path, file_path),
        Provider::Codex => codex_session_launch_command(session_id, project_path, file_path),
    }
}

#[cfg(any(target_os = "macos", test))]
fn codex_session_launch_command(
    session_id: &str,
    project_path: &str,
    file_path: &str,
) -> Result<String, String> {
    let launch = validate_session_launch(Provider::Codex, session_id, project_path, file_path)?;
    let cli = provider_cli::resolve(Provider::Codex)?;
    let cli = unicode_launch_path(&cli, "Codex CLI")?;
    let cwd = unicode_launch_path(&launch.cwd, "Codex 项目目录")?;
    let runtime_prefix = provider_runtime_launch_prefix()?;
    let config_prefix = provider_config_launch_prefix(Provider::Codex)?;
    Ok(format!(
        "cd {} && env -u CLAUDE_CODE_CHILD_SESSION -u CLAUDECODE {}{}{} --dangerously-bypass-approvals-and-sandbox resume {}",
        shell_quote(cwd),
        runtime_prefix,
        config_prefix,
        shell_quote(cli),
        launch.session_id
    ))
}

#[cfg(target_os = "macos")]
fn accessibility_permission_granted(prompt: bool) -> bool {
    use core::ffi::c_void;
    use objc2::rc::autoreleasepool;
    use objc2_foundation::{NSDictionary, NSNumber, NSString};

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        #[link_name = "AXIsProcessTrustedWithOptions"]
        fn ax_is_process_trusted_with_options(options: *const c_void) -> u8;
    }

    autoreleasepool(|_| {
        let key = NSString::from_str("AXTrustedCheckOptionPrompt");
        let value = NSNumber::numberWithBool(prompt);
        let options = NSDictionary::from_slices(&[&*key], &[&*value]);
        let options_ptr = std::ptr::from_ref(&*options).cast::<c_void>();
        // SAFETY: `options` is a live toll-free-bridged CFDictionary containing
        // the documented AXTrustedCheckOptionPrompt key and a CFBoolean value.
        unsafe { ax_is_process_trusted_with_options(options_ptr) != 0 }
    })
}

#[cfg(target_os = "macos")]
fn require_accessibility_permission() -> Result<(), String> {
    if accessibility_permission_granted(true) {
        return Ok(());
    }
    debug_log("accessibility_preflight status=denied terminal_objects_created=0");
    Err("macOS 尚未允许 Claude Session Manager 使用辅助功能，无法创建 Terminal 原生标签页。请在系统提示中打开“系统设置 → 隐私与安全性 → 辅助访问”，允许 Claude Session Manager 后重新点击。此次未创建任何 Terminal 窗口。".into())
}

#[cfg(target_os = "macos")]
fn run_applescript(script: &str) -> Result<String, String> {
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::AnyThread;
    use objc2_foundation::NSString;
    use objc2_osa_kit::{OSAScript, OSAScriptErrorMessageKey};

    // OSAScript executes inside the app process. Spawning /usr/bin/osascript
    // instead makes macOS attribute UI menu actions to osascript, which requires
    // a separate global Accessibility grant and fails with error -1719.
    autoreleasepool(|_| {
        let source = NSString::from_str(script);
        let script = unsafe { OSAScript::initWithSource(OSAScript::alloc(), &source) };
        let mut error = None;
        let result = unsafe { script.executeAndReturnError(Some(&mut error)) };
        if let Some(error) = error {
            let message = error
                .objectForKey(unsafe { OSAScriptErrorMessageKey })
                .map(|value| {
                    let value: Retained<NSString> = unsafe { Retained::cast_unchecked(value) };
                    value.to_string()
                })
                .unwrap_or_else(|| "未知 AppleScript 错误".to_string());
            debug_log(&format!(
                "run_applescript in_process status=error stdout= stderr={message}"
            ));
            return Err(format!(
                "Terminal 自动化失败: {message}。请确认已允许本应用控制系统 Terminal 和辅助功能。"
            ));
        }
        let stdout = result
            .and_then(|value| value.stringValue())
            .map(|value| value.to_string())
            .unwrap_or_default()
            .trim()
            .to_string();
        debug_log(&format!(
            "run_applescript in_process status=0 stdout={stdout} stderr="
        ));
        Ok(stdout)
    })
}

#[cfg(target_os = "macos")]
fn build_terminal_single_script(command: &str) -> String {
    let command = applescript_quote(command);
    let template = r##"set sessionCommand to __COMMAND__

on confirmTrustPrompt(windowId)
  repeat 24 times
    try
      tell application "Terminal"
        if not (exists window id windowId) then return false
        set targetWindow to window id windowId
        set tabText to (contents of selected tab of targetWindow) as text
        if tabText contains "Quick safety check:" and tabText contains "Yes, I trust this folder" then
          set selected of selected tab of targetWindow to true
          set frontmost of targetWindow to true
          set index of targetWindow to 1
          activate
          delay 0.2
          tell application "System Events"
            set frontmost of process "Terminal" to true
            tell process "Terminal"
              key code 125
              delay 0.1
              key code 36
            end tell
          end tell
          return true
        end if
      end tell
    end try
    delay 0.25
  end repeat
  return false
end confirmTrustPrompt

tell application "Terminal"
  activate
  set launchedTab to do script sessionCommand
  delay 0.75
  set launchedTty to tty of launchedTab
  set matchingWindowIds to {}
  repeat with candidateWindow in windows
    repeat with candidateTab in tabs of candidateWindow
      if (tty of candidateTab) is launchedTty then
        set end of matchingWindowIds to id of candidateWindow
      end if
    end repeat
  end repeat
  if (count of matchingWindowIds) is not 1 then error "无法唯一确认新 Terminal 窗口与 TTY"
  set newWindowId to item 1 of matchingWindowIds
end tell
set trustConfirmed to my confirmTrustPrompt(newWindowId)
return "TERMINAL_SINGLE_OK WINDOW_ID " & newWindowId & " TAB_TTY " & launchedTty & " TRUST_CONFIRMED " & trustConfirmed"##;
    template.replace("__COMMAND__", &command)
}

#[cfg(any(target_os = "macos", test))]
fn build_terminal_batch_script(commands: &[String]) -> String {
    let command_list = commands
        .iter()
        .map(|command| applescript_quote(command))
        .collect::<Vec<_>>()
        .join(", ");
    let template = r##"set sessionCommands to {__COMMANDS__}
set sessionCount to count of sessionCommands
set batchStage to "VALIDATE"
set sessionWindowId to 0
set createdTabWindowIds to {}
set expectedTtys to {}

try
  if sessionCount is 0 then error "没有可启动的会话"

  set batchStage to "CREATE_WINDOW"
  tell application "Terminal"
    activate
    set initialTab to do script ""
    delay 0.5
    set initialTty to tty of initialTab
    set sessionWindowId to 0

    -- The returned tab identifies the first new Terminal object without using
    -- the previously focused window. Native tabs appear as distinct window IDs
    -- in Terminal's scripting model, even though they share one visible window.
    repeat with candidateWindow in windows
      repeat with candidateTab in tabs of candidateWindow
        if (tty of candidateTab) is initialTty then
          set sessionWindowId to id of candidateWindow
          exit repeat
        end if
      end repeat
      if sessionWindowId is not 0 then exit repeat
    end repeat
    if sessionWindowId is 0 then error "无法确认新标签页所属的 Terminal 对象"

    if (count of tabs of window id sessionWindowId) is not 1 then error "新 Terminal 对象的初始标签页数量异常"
    set end of createdTabWindowIds to sessionWindowId
    set end of expectedTtys to initialTty
  end tell

  repeat with tabNumber from 2 to sessionCount
    set batchStage to "PREPARE_TAB_" & tabNumber
    tell application "Terminal"
      set anchorWindowId to item -1 of createdTabWindowIds
      if not (exists window id anchorWindowId) then error "本次新建的 Terminal 标签页对象已不存在"
      set anchorWindow to window id anchorWindowId
      set selected of tab 1 of anchorWindow to true
      set frontmost of anchorWindow to true
      set index of anchorWindow to 1
      activate
    end tell

    set batchStage to "FOCUS_TAB_" & tabNumber
    tell application "System Events"
      set frontmost of process "Terminal" to true
    end tell
    set targetWindowReady to false
    repeat 20 times
      tell application "Terminal"
        if exists window id anchorWindowId then
          set anchorWindow to window id anchorWindowId
          if (id of front window) is anchorWindowId then
            if selected of tab 1 of anchorWindow then set targetWindowReady to true
          end if
        end if
      end tell
      if targetWindowReady then exit repeat
      delay 0.1
    end repeat
    if not targetWindowReady then error "目标窗口未确认为 Terminal 当前窗口，未执行系统新标签快捷键"

    set batchStage to "CREATE_TAB_" & tabNumber
    tell application "Terminal" to set beforeActionIds to id of every window
    tell application "System Events"
      tell process "Terminal"
        keystroke "t" using command down
      end tell
    end tell

    -- Window IDs are used only to identify newly created native-tab objects.
    -- They are never used here for closing, cleanup, or modifying old windows.
    set newTabWindowId to 0
    repeat 100 times
      tell application "Terminal"
        set afterActionIds to id of every window
        set newTabWindowIds to {}
        repeat with widRef in afterActionIds
          set wid to contents of widRef
          if wid is not in beforeActionIds then set end of newTabWindowIds to wid
        end repeat
        if (count of newTabWindowIds) is greater than 1 then error "检测到多个新 Terminal 对象，无法确认本次标签页"
        if (count of newTabWindowIds) is 1 then set newTabWindowId to item 1 of newTabWindowIds
      end tell
      if newTabWindowId is not 0 then exit repeat
      delay 0.1
    end repeat
    if newTabWindowId is 0 then error "Terminal 在 10 秒内未返回新标签页对象；已打开内容会保留，不会自动清理"

    tell application "Terminal"
      set newTabWindow to window id newTabWindowId
      if (count of tabs of newTabWindow) is not 1 then error "新标签页脚本对象包含异常数量的 tabs"
      set newTab to tab 1 of newTabWindow
      set end of createdTabWindowIds to newTabWindowId
      set end of expectedTtys to tty of newTab
    end tell
  end repeat

  set batchStage to "VERIFY_TAB_COUNT"
  if (count of createdTabWindowIds) is not sessionCount then error "标签页对象预创建数量与所选会话数量不一致"

  repeat with tabNumber from 1 to sessionCount
    set batchStage to "LAUNCH_TAB_" & tabNumber
    tell application "Terminal"
      set targetWindowId to item tabNumber of createdTabWindowIds
      if not (exists window id targetWindowId) then error "启动会话前目标标签页对象已不存在"
      set targetTab to tab 1 of window id targetWindowId
      set launchedTab to do script (item tabNumber of sessionCommands) in targetTab
      if (tty of launchedTab) is not (item tabNumber of expectedTtys) then error "命令返回的标签页与目标标签页不一致"
      delay 0.35
    end tell
  end repeat

  set batchStage to "FINAL_VERIFY"
  tell application "Terminal"
    repeat with widRef in createdTabWindowIds
      set wid to contents of widRef
      if not (exists window id wid) then error "启动后标签页对象已不存在"
      if (count of tabs of window id wid) is not 1 then error "启动后标签页脚本对象异常"
    end repeat
  end tell
  set finalTabCount to count of createdTabWindowIds
  set savedDelimiters to AppleScript's text item delimiters
  set AppleScript's text item delimiters to ","
  set traceIds to createdTabWindowIds as text
  set traceTtys to expectedTtys as text
  set AppleScript's text item delimiters to savedDelimiters
  return "TERMINAL_BATCH_OK " & finalTabCount & " TAB_IDS " & traceIds & " TAB_TTYS " & traceTtys
on error errMsg number errNum
  set savedDelimiters to AppleScript's text item delimiters
  set AppleScript's text item delimiters to ","
  set traceIds to createdTabWindowIds as text
  set traceTtys to expectedTtys as text
  set AppleScript's text item delimiters to savedDelimiters
  error "CSM_BATCH_ERROR stage=" & batchStage & " detail=" & errMsg & " code=" & errNum & " window=" & sessionWindowId & " tabIds=" & traceIds & " tabTtys=" & traceTtys
end try"##;
    template.replace("__COMMANDS__", &command_list)
}

#[cfg(any(target_os = "macos", test))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct TerminalBatchMetadata {
    window_ids: Vec<i64>,
    ttys: Vec<String>,
}

#[cfg(any(target_os = "macos", test))]
fn parse_terminal_batch_result(result: &str, opened: usize) -> Option<TerminalBatchMetadata> {
    let mut fields = result.split_whitespace();
    if fields.next()? != "TERMINAL_BATCH_OK" || fields.next()?.parse::<usize>().ok()? != opened {
        return None;
    }
    if fields.next()? != "TAB_IDS" {
        return None;
    }
    let ids = fields.next()?;
    if fields.next()? != "TAB_TTYS" {
        return None;
    }
    let ttys = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    let window_ids = ids
        .split(',')
        .map(str::parse::<i64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let ttys = ttys.split(',').map(str::to_string).collect::<Vec<_>>();
    if window_ids.len() != opened
        || ttys.len() != opened
        || window_ids.iter().collect::<HashSet<_>>().len() != opened
        || ttys.iter().collect::<HashSet<_>>().len() != opened
        || !ttys.iter().all(|tty| tty.starts_with("/dev/tty"))
    {
        return None;
    }
    Some(TerminalBatchMetadata { window_ids, ttys })
}

#[cfg(test)]
fn valid_terminal_batch_result(result: &str, opened: usize) -> bool {
    parse_terminal_batch_result(result, opened).is_some()
}

#[derive(Debug, Serialize)]
struct TerminalBatchResult {
    opened: usize,
    already_running: usize,
}

#[cfg(target_os = "macos")]
type PlatformLaunch = MacTerminalLaunch;
#[cfg(target_os = "windows")]
type PlatformLaunch = windows_platform::WindowsLaunchSpec;
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
type PlatformLaunch = ();

#[cfg(target_os = "macos")]
fn prepare_platform_launch(
    provider: Provider,
    session_id: &str,
    project_path: &str,
    file_path: &str,
    _title: &str,
) -> Result<PlatformLaunch, String> {
    let command = session_launch_command_for(provider, session_id, project_path, file_path)?;
    Ok(MacTerminalLaunch {
        provider,
        session_id: session_id.to_string(),
        command,
    })
}

#[cfg(target_os = "windows")]
fn prepare_platform_launch(
    provider: Provider,
    session_id: &str,
    project_path: &str,
    file_path: &str,
    title: &str,
) -> Result<PlatformLaunch, String> {
    let launch = validate_session_launch(provider, session_id, project_path, file_path)?;
    windows_platform::WindowsLaunchSpec::new(launch, title)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn prepare_platform_launch(
    _provider: Provider,
    _session_id: &str,
    _project_path: &str,
    _file_path: &str,
    _title: &str,
) -> Result<PlatformLaunch, String> {
    Err("当前构建只支持 macOS 和 Windows".into())
}

#[cfg(target_os = "macos")]
fn launch_terminal_batch(launches: &[PlatformLaunch]) -> Result<(), String> {
    require_accessibility_permission()?;
    let commands = launches
        .iter()
        .map(|launch| launch.command.clone())
        .collect::<Vec<_>>();
    let script = build_terminal_batch_script(&commands);
    let result = run_applescript(&script)?;
    let metadata = parse_terminal_batch_result(&result, launches.len()).ok_or_else(|| {
        format!(
            "Terminal 未确认全部 {} 个标签页的对象与 TTY: {result}。已打开内容会保留，未执行自动清理。",
            launches.len()
        )
    })?;
    debug_log(&format!(
        "macos_batch_terminal windows={:?} ttys={:?}",
        metadata.window_ids, metadata.ttys
    ));
    wait_for_provider_launches(launches, &metadata.ttys)
}

#[cfg(target_os = "windows")]
fn launch_terminal_batch(launches: &[PlatformLaunch]) -> Result<(), String> {
    windows_platform::open_launches(launches)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn launch_terminal_batch(_launches: &[PlatformLaunch]) -> Result<(), String> {
    Err("当前构建只支持 macOS 和 Windows".into())
}

fn open_sessions_terminal_blocking(
    sessions: Vec<SessionInfo>,
) -> Result<TerminalBatchResult, String> {
    let mut seen = HashSet::new();
    let mut unique_sessions = Vec::new();
    for session in sessions {
        let provider = Provider::parse(Some(session.provider.as_str()))?;
        let key = session_launch_key(provider, &session.session_id);
        if seen.insert(key) {
            unique_sessions.push((provider, session));
        }
    }

    #[cfg(target_os = "windows")]
    let _launch_guards = SessionLaunchGuards::acquire(seen.iter().cloned().collect());
    let mut launches = Vec::new();
    let mut already_running = 0usize;

    // Validate every launch before creating anything. Running sessions are only
    // skipped: this path never focuses, modifies or cleans up existing windows.
    // On Windows the per-key guards also serialize this check with single-open
    // and overlapping batches, including aliases with different backing paths.
    for (provider, session) in unique_sessions {
        if running_session_token(
            provider,
            &session.session_id,
            &session.file_path,
            &session.source,
        )?
        .is_some()
        {
            already_running += 1;
            continue;
        }
        launches.push(prepare_platform_launch(
            provider,
            &session.session_id,
            &session.project_path,
            &session.file_path,
            &session.title,
        )?);
    }

    if launches.is_empty() {
        return Ok(TerminalBatchResult {
            opened: 0,
            already_running,
        });
    }
    let opened = launches.len();
    debug_log(&format!(
        "open_sessions_terminal open={opened} already_running={already_running}"
    ));
    launch_terminal_batch(&launches)?;
    Ok(TerminalBatchResult {
        opened,
        already_running,
    })
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn open_sessions_terminal(sessions: Vec<SessionInfo>) -> Result<TerminalBatchResult, String> {
    // Keep Terminal's in-process Apple Events on Tauri's main-thread command
    // path; see open_session for the verified background-thread failure mode.
    open_sessions_terminal_blocking(sessions)
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
async fn open_sessions_terminal(sessions: Vec<SessionInfo>) -> Result<TerminalBatchResult, String> {
    tokio::task::spawn_blocking(move || open_sessions_terminal_blocking(sessions))
        .await
        .map_err(|error| format!("批量会话启动任务失败: {error}"))?
}

#[tauri::command]
fn should_open_favorites() -> bool {
    // Login may open the manager, but must never trigger batch Terminal recovery.
    false
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn reveal_in_finder(file_path: String) -> Result<(), String> {
    Command::new("open")
        .args(["-R", &file_path])
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
const MACOS_AUTOSTART_LABEL: &str = "com.claude.sessionmanager";

#[cfg(any(target_os = "macos", test))]
fn macos_autostart_plist_contents(executable: &Path) -> String {
    let exe = executable
        .to_string_lossy()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");
    format!(
"<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\"><dict>
  <key>Label</key><string>com.claude.sessionmanager</string>
  <key>ProgramArguments</key><array><string>{exe}</string></array>
  <key>RunAtLoad</key><true/>
  <key>LaunchOnlyOnce</key><true/>
</dict></plist>")
}

#[cfg(any(target_os = "macos", test))]
fn restore_autostart_file(path: &Path, previous: &Option<Vec<u8>>) -> Result<&'static str, String> {
    match previous {
        Some(contents) => std::fs::write(path, contents)
            .map(|_| "已恢复此前的自启配置文件")
            .map_err(|error| format!("恢复此前自启配置失败: {error}")),
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok("已清理未生效的自启配置文件"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok("已清理未生效的自启配置文件")
            }
            Err(error) => Err(format!("清理未生效自启配置失败: {error}")),
        },
    }
}

#[cfg(any(target_os = "macos", test))]
fn rollback_failed_autostart_setup(
    path: &Path,
    previous: &Option<Vec<u8>>,
    was_loaded: bool,
    unload: &mut dyn FnMut() -> bool,
    load: &mut dyn FnMut() -> bool,
    loaded: &mut dyn FnMut() -> bool,
) -> Result<String, String> {
    // Unload while the new plist still exists. Removing it first can leave an
    // in-memory registration behind because `launchctl unload <missing>` fails.
    let unloaded = unload();
    let restored = restore_autostart_file(path, previous);
    let restored_message = match restored {
        Ok(message) => message.to_string(),
        Err(error) => return Err(format!("{error}；新注册卸载成功: {unloaded}")),
    };
    if was_loaded {
        if previous.is_none() {
            return Err(format!(
                "{restored_message}；此前服务已加载但没有可恢复的配置文件；新注册卸载成功: {unloaded}"
            ));
        }
        let load_started = load();
        let verified = load_started && loaded();
        if verified {
            return Ok(format!("{restored_message}，并已恢复此前已加载的自启注册"));
        }
        return Err(format!(
            "{restored_message}；此前注册恢复失败（新注册卸载成功: {unloaded}，重新加载成功: {load_started}，加载复核成功: false）"
        ));
    }
    if loaded() {
        Err(format!(
            "{restored_message}；新注册仍被 launchctl 报告为已加载（卸载命令成功: {unloaded}）"
        ))
    } else {
        Ok(restored_message)
    }
}

#[cfg(any(target_os = "macos", test))]
fn launchctl_output_matches_current_executable(output: &[u8], executable: &Path) -> bool {
    let output = String::from_utf8_lossy(output);
    output.contains(MACOS_AUTOSTART_LABEL) && output.contains(executable.to_string_lossy().as_ref())
}

#[cfg(target_os = "macos")]
fn macos_autostart_service_output() -> Option<std::process::Output> {
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|output| {
            if !output.status.success() {
                return None;
            }
            let uid = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit())).then_some(uid)
        })?;
    Command::new("launchctl")
        .arg("print")
        .arg(format!("gui/{uid}/{MACOS_AUTOSTART_LABEL}"))
        .output()
        .ok()
        .filter(|output| output.status.success())
}

#[cfg(target_os = "macos")]
fn macos_autostart_service_loaded() -> bool {
    macos_autostart_service_output().is_some()
}

#[cfg(target_os = "macos")]
fn macos_autostart_loaded(executable: &Path) -> bool {
    macos_autostart_service_output().is_some_and(|output| {
        launchctl_output_matches_current_executable(&output.stdout, executable)
    })
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn setup_autostart() -> Result<String, String> {
    let executable =
        std::env::current_exe().map_err(|error| format!("获取可执行路径失败: {error}"))?;
    let plist = macos_autostart_plist_contents(&executable);
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join("Library/LaunchAgents/com.claude.sessionmanager.plist");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| format!("创建自启目录失败: {error}"))?;
    }
    let previous = match std::fs::read(&path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("读取既有自启配置失败: {error}")),
    };
    let was_loaded = macos_autostart_service_loaded();
    if let Err(error) = std::fs::write(&path, &plist) {
        let rollback = match restore_autostart_file(&path, &previous) {
            Ok(message) => message.to_string(),
            Err(rollback_error) => rollback_error,
        };
        return Err(format!("写入自启配置失败: {error}；{rollback}"));
    }
    let _ = Command::new("launchctl")
        .args(["unload", &path.to_string_lossy()])
        .status();
    let load = Command::new("launchctl")
        .args(["load", &path.to_string_lossy()])
        .status();
    let mut registered = false;
    if load.as_ref().is_ok_and(|status| status.success()) {
        for _ in 0..5 {
            if macos_autostart_loaded(&executable) {
                registered = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    if registered {
        return Ok("已设置开机自启".into());
    }

    let rollback = {
        let path_text = path.to_string_lossy();
        let mut unload = || {
            Command::new("launchctl")
                .args(["unload", path_text.as_ref()])
                .status()
                .is_ok_and(|status| status.success())
        };
        let mut load_previous = || {
            Command::new("launchctl")
                .args(["load", path_text.as_ref()])
                .status()
                .is_ok_and(|status| status.success())
        };
        let mut loaded = macos_autostart_service_loaded;
        rollback_failed_autostart_setup(
            &path,
            &previous,
            was_loaded,
            &mut unload,
            &mut load_previous,
            &mut loaded,
        )
    };
    let load_detail = match load {
        Ok(status) => format!("状态码 {:?}", status.code()),
        Err(error) => error.to_string(),
    };
    let rollback_detail = match rollback {
        Ok(message) => message.to_string(),
        Err(error) => error,
    };
    Err(format!(
        "launchctl 注册或复核失败（{load_detail}）；{rollback_detail}"
    ))
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn get_autostart_status() -> bool {
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join("Library/LaunchAgents/com.claude.sessionmanager.plist");
    std::fs::read_to_string(&path)
        .ok()
        .is_some_and(|contents| contents == macos_autostart_plist_contents(&executable))
        && macos_autostart_loaded(&executable)
}

#[cfg(target_os = "macos")]
#[tauri::command]
fn remove_autostart() -> Result<String, String> {
    let path = dirs::home_dir()
        .unwrap_or_default()
        .join("Library/LaunchAgents/com.claude.sessionmanager.plist");
    if !path.exists() {
        return Ok("未找到自启配置".into());
    }
    let unloaded = Command::new("launchctl")
        .args(["unload", &path.to_string_lossy()])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    std::fs::remove_file(&path)
        .map_err(|error| format!("删除自启配置失败: {error} (launchctl unload: {unloaded})"))?;
    Ok("已移除开机自启".into())
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn reveal_in_finder(file_path: String) -> Result<(), String> {
    windows_platform::reveal_in_file_manager(&file_path)
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[tauri::command]
fn reveal_in_finder(_file_path: String) -> Result<(), String> {
    Err("当前构建只支持 macOS 和 Windows".into())
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn setup_autostart() -> Result<String, String> {
    windows_platform::setup_autostart().map(|_| "已设置当前用户登录自启（只启动管理器）".into())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[tauri::command]
fn setup_autostart() -> Result<String, String> {
    Err("当前构建只支持 macOS 和 Windows".into())
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn get_autostart_status() -> bool {
    windows_platform::autostart_enabled()
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[tauri::command]
fn get_autostart_status() -> bool {
    false
}

#[cfg(target_os = "windows")]
#[tauri::command]
fn remove_autostart() -> Result<String, String> {
    windows_platform::remove_autostart().map(|_| "已移除当前用户登录自启".into())
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[tauri::command]
fn remove_autostart() -> Result<String, String> {
    Err("当前构建只支持 macOS 和 Windows".into())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![
            get_sessions,
            get_codex_message_count,
            get_favorite_sessions,
            get_claude_storage_stats,
            get_messages,
            export_session_markdown,
            toggle_favorite,
            open_session,
            open_sessions_terminal,
            should_open_favorites,
            reveal_in_finder,
            get_autostart_status,
            setup_autostart,
            remove_autostart,
            update_checker::get_update_settings,
            update_checker::save_update_settings,
            update_checker::check_for_updates,
            update_checker::dismiss_update,
            update_checker::open_update_release,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const SESSION_ID: &str = "00000000-0000-4000-8000-000000000001";
    static NEXT_FIXTURE: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn windows_single_session_title_preserves_visible_titles_and_falls_back_when_empty() {
        assert_eq!(
            windows_single_session_title(
                Provider::Claude,
                SESSION_ID,
                Some("CSM-WIN-unique Claude Gamma".into())
            ),
            "CSM-WIN-unique Claude Gamma"
        );
        assert_eq!(
            windows_single_session_title(Provider::Claude, SESSION_ID, Some(String::new())),
            "claude 00000000"
        );
        assert_eq!(
            windows_single_session_title(Provider::Codex, SESSION_ID, None),
            "codex 00000000"
        );
    }

    struct TestSession {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TestSession {
        fn new(name: &str) -> Self {
            let sequence = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "csm-parser-test-{}-{}-{}",
                std::process::id(),
                name,
                sequence
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("{}.jsonl", SESSION_ID));
            Self { dir, path }
        }

        fn write(&self, contents: &[u8]) {
            std::fs::write(&self.path, contents).unwrap();
        }

        fn append(&self, contents: &[u8]) {
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&self.path)
                .unwrap();
            file.write_all(contents).unwrap();
            file.sync_all().unwrap();
        }

        fn parse(&self, cache: &mut Cache, favs: &HashSet<String>) -> SessionInfo {
            parse_session_file_with_cache(&self.path, "test-project", favs, cache).unwrap()
        }
    }

    impl Drop for TestSession {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set_path(key: &'static str, value: &Path) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn initial_parse_and_append_only_reads_new_records() {
        let fixture = TestSession::new("append");
        let mut cache = Cache::new();
        let favs = HashSet::new();
        fixture.write(concat!(
            "{\"type\":\"user\",\"timestamp\":\"2026-11-23T01:00:00Z\",\"cwd\":\"/tmp/project\",\"message\":{\"content\":\"first\"}}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Initial\",\"timestamp\":\"2026-11-23T01:01:00Z\"}\n"
        ).as_bytes());

        let first = fixture.parse(&mut cache, &favs);
        assert_eq!(first.title, "Initial");
        assert_eq!(first.message_count, 1);
        assert_eq!(first.project_path, "/tmp/project");
        assert_eq!(first.created_at_ts, parse_ts("2026-11-23T01:00:00Z"));

        fixture.append(concat!(
            "{\"type\":\"custom-title\",\"customTitle\":\"Renamed\",\"timestamp\":\"2026-11-23T01:02:00Z\"}\n",
            "{\"type\":\"user\",\"timestamp\":\"2026-11-23T01:03:00Z\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"second\"}]}}\n"
        ).as_bytes());

        // Force the old mtime cache condition: length alone must trigger the append.
        let key = fixture.path.to_string_lossy().to_string();
        cache.entries.get_mut(&key).unwrap().mtime = file_mtime(&fixture.path);
        let second = fixture.parse(&mut cache, &favs);
        assert_eq!(second.title, "Renamed");
        assert_eq!(second.message_count, 2);
        assert_eq!(second.updated_at_ts, parse_ts("2026-11-23T01:03:00Z"));
        assert_eq!(second.project_path, "/tmp/project");
        assert_eq!(
            cache.entries.get(&key).unwrap().parsed_offset,
            fixture.path.metadata().unwrap().len()
        );
    }

    #[test]
    fn live_cache_appends_fully_validate_old_prefixes() {
        let fixture = TestSession::new("bounded-append-reads");
        let mut cache = Cache::new();
        let favs = HashSet::new();
        let large_record = serde_json::json!({"type":"progress", "data":"x".repeat(1024 * 1024)})
            .to_string()
            + "\n";
        fixture.write(large_record.as_bytes());
        fixture.parse(&mut cache, &favs);

        for index in 0..3 {
            let previous_len = fixture.path.metadata().unwrap().len();
            fixture.append(claude_message(&format!("bounded-{index}")).as_bytes());
            take_cache_test_bytes_read();
            let parsed = fixture.parse(&mut cache, &favs);
            let bytes_read = take_cache_test_bytes_read();
            assert_eq!(parsed.message_count, index + 1);
            assert!(
                bytes_read >= previous_len,
                "append {index} read {bytes_read} bytes without validating the {previous_len}-byte old prefix"
            );
        }
    }

    #[test]
    #[ignore = "opt-in real 8 GiB synthetic disk benchmark; set CSM_SYNTHETIC_8GIB_SMOKE=1"]
    fn synthetic_eight_gib_cache_and_pagination_smoke() {
        if std::env::var("CSM_SYNTHETIC_8GIB_SMOKE").as_deref() != Ok("1") {
            return;
        }
        const RECORDS: usize = 512;
        const CHUNKS_PER_RECORD: usize = 16;
        const CHUNK_LEN: usize = 1024 * 1024;
        const EXPECTED_PAYLOAD: u64 = 8 * 1024 * 1024 * 1024;

        let fixture = TestSession::new("synthetic-eight-gib");
        let write_started = std::time::Instant::now();
        let chunk = vec![b'x'; CHUNK_LEN];
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&fixture.path)
            .unwrap();
        let mut writer = std::io::BufWriter::with_capacity(CHUNK_LEN, file);
        for _ in 0..RECORDS {
            writer
                .write_all(b"{\"type\":\"progress\",\"data\":\"")
                .unwrap();
            for _ in 0..CHUNKS_PER_RECORD {
                writer.write_all(&chunk).unwrap();
            }
            writer.write_all(b"\"}\n").unwrap();
        }
        writer.flush().unwrap();
        writer.get_ref().sync_all().unwrap();
        drop(writer);
        let file_len = fixture.path.metadata().unwrap().len();
        assert!((EXPECTED_PAYLOAD..EXPECTED_PAYLOAD + 64 * 1024).contains(&file_len));
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let allocated = fixture.path.metadata().unwrap().blocks() * 512;
            assert!(
                allocated >= EXPECTED_PAYLOAD,
                "benchmark file must not be sparse"
            );
            println!("SYNTHETIC_8GIB_STORAGE allocated_bytes={allocated}");
        }
        println!(
            "SYNTHETIC_8GIB_WRITE bytes={file_len} elapsed_ms={}",
            write_started.elapsed().as_millis()
        );

        let mut cache = Cache::new();
        let favs = HashSet::new();
        take_cache_test_bytes_read();
        let cold_started = std::time::Instant::now();
        let cold = fixture.parse(&mut cache, &favs);
        let cold_ms = cold_started.elapsed().as_millis();
        let cold_bytes = take_cache_test_bytes_read();
        assert_eq!(cold.message_count, 0);
        assert!(cold_bytes >= EXPECTED_PAYLOAD);
        println!("SYNTHETIC_8GIB_COLD read_bytes={cold_bytes} elapsed_ms={cold_ms}");

        take_cache_test_bytes_read();
        let warm_started = std::time::Instant::now();
        fixture.parse(&mut cache, &favs);
        let warm_ms = warm_started.elapsed().as_millis();
        let warm_bytes = take_cache_test_bytes_read();
        assert!(
            warm_bytes >= EXPECTED_PAYLOAD,
            "unchanged cache reuse must fully revalidate content, read={warm_bytes}"
        );
        println!("SYNTHETIC_8GIB_WARM read_bytes={warm_bytes} elapsed_ms={warm_ms}");

        let mut append_reads = Vec::new();
        let mut append_prefixes = Vec::new();
        for index in 0..3 {
            append_prefixes.push(fixture.path.metadata().unwrap().len());
            fixture.append(claude_message(&format!("8gib-{index}")).as_bytes());
            take_cache_test_bytes_read();
            let append_started = std::time::Instant::now();
            let parsed = fixture.parse(&mut cache, &favs);
            let elapsed_ms = append_started.elapsed().as_millis();
            let bytes_read = take_cache_test_bytes_read();
            assert_eq!(parsed.message_count, index + 1);
            append_reads.push(bytes_read);
            println!(
                "SYNTHETIC_8GIB_APPEND index={index} read_bytes={bytes_read} elapsed_ms={elapsed_ms}"
            );
        }

        take_cache_test_bytes_read();
        let page_started = std::time::Instant::now();
        let first =
            try_read_messages_page(&fixture.path, None, None, 0, 1, Provider::Claude).unwrap();
        let first_ms = page_started.elapsed().as_millis();
        let first_bytes = take_cache_test_bytes_read();
        assert_eq!(first.messages.len(), 1);
        assert_eq!(first.messages[0].text, "8gib-2");
        assert!(!first.snapshot_token.is_empty());
        assert!(
            first_bytes >= file_len,
            "the first page hashes the complete fixed snapshot"
        );
        println!("SYNTHETIC_8GIB_PAGE index=0 read_bytes={first_bytes} elapsed_ms={first_ms}");

        take_cache_test_bytes_read();
        let page_started = std::time::Instant::now();
        let second = try_read_messages_page(
            &fixture.path,
            Some(first.snapshot_len),
            Some(first.snapshot_token.clone()),
            1,
            1,
            Provider::Claude,
        )
        .unwrap();
        let second_ms = page_started.elapsed().as_millis();
        let second_bytes = take_cache_test_bytes_read();
        assert_eq!(second.messages.len(), 1);
        assert_eq!(second.messages[0].text, "8gib-1");
        assert_eq!(second.snapshot_token, first.snapshot_token);
        assert!(
            second_bytes >= file_len,
            "older pages revalidate the complete fixed prefix"
        );
        println!("SYNTHETIC_8GIB_PAGE index=1 read_bytes={second_bytes} elapsed_ms={second_ms}");

        assert!(
            append_reads
                .iter()
                .zip(&append_prefixes)
                .all(|(bytes, prefix)| bytes >= prefix),
            "append reads did not fully validate old prefixes: reads={append_reads:?} prefixes={append_prefixes:?}"
        );
        println!(
            "SYNTHETIC_8GIB_SMOKE_OK bytes={file_len} cold_ms={cold_ms} warm_ms={warm_ms} append_reads={append_reads:?} page_ms={first_ms},{second_ms}"
        );
    }

    #[test]
    fn partial_trailing_line_is_completed_on_a_later_append() {
        let fixture = TestSession::new("partial");
        let mut cache = Cache::new();
        let favs = HashSet::new();
        let complete = concat!(
            "{\"type\":\"user\",\"timestamp\":\"2026-11-23T01:00:00Z\",\"message\":{\"content\":\"first\"}}\n",
            "{\"type\":\"ai-title\",\"aiTitle\":\"Original\"}\n"
        ).as_bytes();
        let title_line = "{\"type\":\"custom-title\",\"customTitle\":\"增量标题\"}".as_bytes();
        let split = 30;
        let mut initial = complete.to_vec();
        initial.extend_from_slice(&title_line[..split]);
        fixture.write(&initial);

        let first = fixture.parse(&mut cache, &favs);
        assert_eq!(first.title, "Original");
        assert_eq!(first.message_count, 1);
        let key = fixture.path.to_string_lossy().to_string();
        assert_eq!(
            cache.entries.get(&key).unwrap().parsed_offset,
            complete.len() as u64
        );

        let mut appended = title_line[split..].to_vec();
        appended.extend_from_slice(
            b"\n{\"type\":\"user\",\"timestamp\":\"2026-11-23T01:04:00Z\",\"message\":{\"content\":\"second\"}}\n",
        );
        fixture.append(&appended);
        let second = fixture.parse(&mut cache, &favs);
        assert_eq!(second.title, "增量标题");
        assert_eq!(second.message_count, 2);
        assert_eq!(
            cache.entries.get(&key).unwrap().parsed_offset,
            fixture.path.metadata().unwrap().len()
        );
        let cached_hash = cache.entries.get(&key).unwrap().content_hash.unwrap();
        let mut file = std::fs::File::open(&fixture.path).unwrap();
        let full_hash =
            read_content_hash(&mut file, fixture.path.metadata().unwrap().len()).unwrap();
        assert_eq!(
            cached_hash, full_hash,
            "old partial bytes are hashed only once"
        );
    }

    #[test]
    fn valid_unterminated_final_record_is_parsed_immediately() {
        let fixture = TestSession::new("unterminated-valid-record");
        let mut cache = Cache::new();
        let favs = HashSet::new();
        fixture.write(
            b"{\"type\":\"custom-title\",\"customTitle\":\"No newline required\",\"cwd\":\"/tmp/project\"}",
        );
        let first = fixture.parse(&mut cache, &favs);
        assert_eq!(first.title, "No newline required");
        assert_eq!(first.project_path, "/tmp/project");
        let key = fixture.path.to_string_lossy().to_string();
        assert_eq!(
            cache.entries.get(&key).unwrap().parsed_offset,
            fixture.path.metadata().unwrap().len()
        );

        fixture.append(
            b"\n{\"type\":\"user\",\"timestamp\":\"2026-11-23T01:00:00Z\",\"message\":{\"content\":\"next\"}}",
        );
        let second = fixture.parse(&mut cache, &favs);
        assert_eq!(second.message_count, 1);
        assert_eq!(second.title, "No newline required");
    }

    #[test]
    fn truncation_and_rewrite_reset_accumulated_state() {
        let fixture = TestSession::new("truncate");
        let mut cache = Cache::new();
        let favs = HashSet::new();
        fixture.write(concat!(
            "{\"type\":\"custom-title\",\"customTitle\":\"Old title\",\"cwd\":\"/old/project\"}\n",
            "{\"type\":\"user\",\"timestamp\":\"2026-11-23T01:00:00Z\",\"message\":{\"content\":\"first\"}}\n",
            "{\"type\":\"user\",\"timestamp\":\"2026-11-23T01:01:00Z\",\"message\":{\"content\":\"second\"}}\n"
        ).as_bytes());
        let first = fixture.parse(&mut cache, &favs);
        assert_eq!(first.message_count, 2);

        fixture.write(b"{\"type\":\"custom-title\",\"customTitle\":\"New\",\"cwd\":\"/new\"}\n");
        let rewritten = fixture.parse(&mut cache, &favs);
        assert_eq!(rewritten.title, "New");
        assert_eq!(rewritten.message_count, 0);
        assert_eq!(rewritten.project_path, "/new");
        assert_eq!(rewritten.created_at_ts, 0);
    }

    #[test]
    fn same_length_rewrite_is_not_served_from_cache() {
        let fixture = TestSession::new("rewrite");
        let mut cache = Cache::new();
        let favs = HashSet::new();
        fixture.write(
            b"{\"type\":\"ai-title\",\"aiTitle\":\"old\",\"timestamp\":\"2026-11-23T01:00:00Z\"}\n",
        );
        assert_eq!(fixture.parse(&mut cache, &favs).title, "old");

        let original_mtime = file_mtime(&fixture.path);
        let modified = fixture.path.metadata().unwrap().modified().unwrap();
        fixture.write(
            b"{\"type\":\"ai-title\",\"aiTitle\":\"new\",\"timestamp\":\"2026-11-23T01:00:00Z\"}\n",
        );
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&fixture.path)
            .unwrap();
        file.set_modified(modified).unwrap();
        assert_eq!(file_mtime(&fixture.path), original_mtime);
        assert_eq!(fixture.parse(&mut cache, &favs).title, "new");
    }

    #[test]
    fn unchanged_length_middle_rewrite_with_tied_stamps_is_reparsed() {
        let fixture = TestSession::new("unchanged-middle-rewrite");
        let mut cache = Cache::new();
        let favs = HashSet::new();
        let prefix =
            serde_json::json!({"type":"progress", "data":"x".repeat(70000)}).to_string() + "\n";
        let suffix =
            serde_json::json!({"type":"progress", "data":"z".repeat(80000)}).to_string() + "\n";
        let original = prefix.clone()
            + "{\"type\":\"ai-title\",\"aiTitle\":\"old title\",\"cwd\":\"/old/path\"}\n"
            + &claude_message("first")
            + &suffix;
        let rewritten = prefix
            + "{\"type\":\"ai-title\",\"aiTitle\":\"new title\",\"cwd\":\"/new/path\"}\n"
            + &claude_message("first")
            + &suffix;
        assert_eq!(original.len(), rewritten.len());
        fixture.write(original.as_bytes());
        assert_eq!(fixture.parse(&mut cache, &favs).title, "old title");

        let modified = fixture.path.metadata().unwrap().modified().unwrap();
        fixture.write(rewritten.as_bytes());
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&fixture.path)
            .unwrap();
        file.set_modified(modified).unwrap();
        drop(file);
        let key = fixture.path.to_string_lossy().to_string();
        let current = std::fs::File::open(&fixture.path).unwrap();
        let (identity, change) = file_identity_and_change(&current).unwrap();
        let cached = cache.entries.get_mut(&key).unwrap();
        cached.mtime = file_mtime(&fixture.path);
        cached.change_stamp = Some(format!("{identity}:{change}"));

        let reparsed = fixture.parse(&mut cache, &favs);
        assert_eq!(reparsed.title, "new title");
        assert_eq!(reparsed.project_path, "/new/path");
    }

    #[test]
    fn codex_unchanged_length_middle_rewrite_with_tied_stamps_is_not_cached() {
        let fixture = TestSession::new("codex-unchanged-middle-rewrite");
        let mut cache = Cache::new();
        let record = |role: &str| {
            serde_json::json!({
                "timestamp": "2026-11-23T01:00:00Z",
                "type": "response_item",
                "payload": {
                    "type": "message", "role": role,
                    "content": [{"type": "input_text", "text": "same"}]
                }
            })
            .to_string()
                + "\n"
        };
        let prefix =
            serde_json::json!({"type":"progress", "data":"x".repeat(70000)}).to_string() + "\n";
        let suffix =
            serde_json::json!({"type":"progress", "data":"z".repeat(80000)}).to_string() + "\n";
        let original = prefix.clone() + &record("user") + &suffix;
        let rewritten = prefix + &record("tool") + &suffix;
        assert_eq!(original.len(), rewritten.len());
        fixture.write(original.as_bytes());
        let key = codex_cache_key(&fixture.path);
        let (state, _) =
            parsed_state_with_cache(&fixture.path, key.clone(), Provider::Codex, &mut cache)
                .unwrap();
        assert_eq!(state.user_count, 1);

        let modified = fixture.path.metadata().unwrap().modified().unwrap();
        fixture.write(rewritten.as_bytes());
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&fixture.path)
            .unwrap();
        file.set_modified(modified).unwrap();
        drop(file);
        let current = std::fs::File::open(&fixture.path).unwrap();
        let (identity, change) = file_identity_and_change(&current).unwrap();
        let cached = cache.entries.get_mut(&key).unwrap();
        cached.mtime = file_mtime(&fixture.path);
        cached.change_stamp = Some(format!("{identity}:{change}"));

        let (_, unchanged) = cached_codex_message_count(&fixture.path, &key, &cache);
        assert!(
            !unchanged,
            "a tied stamp must not bypass full-content validation"
        );
        let (state, _) =
            parsed_state_with_cache(&fixture.path, key, Provider::Codex, &mut cache).unwrap();
        assert_eq!(state.user_count, 0);
    }

    #[test]
    fn rewrite_beyond_four_kib_then_append_rebuilds_cached_state() {
        let fixture = TestSession::new("rewrite-beyond-prefix");
        let mut cache = Cache::new();
        let favs = HashSet::new();
        let prefix =
            serde_json::json!({"type":"progress", "data":"x".repeat(5000)}).to_string() + "\n";
        let original = prefix.clone()
            + "{\"type\":\"ai-title\",\"aiTitle\":\"old title\",\"cwd\":\"/old/path\"}\n"
            + "{\"type\":\"user\",\"message\":{\"content\":\"first\"}}\n";
        fixture.write(original.as_bytes());
        assert_eq!(fixture.parse(&mut cache, &favs).title, "old title");

        let rewritten = prefix
            + "{\"type\":\"ai-title\",\"aiTitle\":\"new title\",\"cwd\":\"/new/path\"}\n"
            + "{\"type\":\"user\",\"message\":{\"content\":\"first\"}}\n"
            + "{\"type\":\"user\",\"message\":{\"content\":\"second\"}}\n";
        assert!(rewritten.len() > original.len());
        fixture.write(rewritten.as_bytes());
        let reparsed = fixture.parse(&mut cache, &favs);
        assert_eq!(reparsed.title, "new title");
        assert_eq!(reparsed.project_path, "/new/path");
        assert_eq!(reparsed.message_count, 2);
    }

    #[test]
    fn middle_rewrite_then_append_rebuilds_cached_state() {
        let fixture = TestSession::new("middle-rewrite-append-cache");
        let mut cache = Cache::new();
        let favs = HashSet::new();
        let (original, rewritten) = middle_rewrite_pair();
        fixture.write(original.as_bytes());
        assert_eq!(fixture.parse(&mut cache, &favs).title, "old title");

        fixture.write(rewritten.as_bytes());
        let reparsed = fixture.parse(&mut cache, &favs);
        assert_eq!(reparsed.title, "new title");
        assert_eq!(reparsed.project_path, "/new/path");
        assert_eq!(reparsed.message_count, 2);
    }

    #[test]
    fn rolling_cache_hash_matches_full_content_after_append() {
        let fixture = TestSession::new("rolling-content-hash");
        let mut cache = Cache::new();
        let favs = HashSet::new();
        fixture.write(b"{\"type\":\"user\",\"message\":{\"content\":\"first\"}}\n");
        fixture.parse(&mut cache, &favs);
        fixture.append(b"{\"type\":\"user\",\"message\":{\"content\":\"second\"}}");
        let second = fixture.parse(&mut cache, &favs);
        assert_eq!(second.message_count, 2);
        let key = fixture.path.to_string_lossy().to_string();
        let cached_hash = cache.entries.get(&key).unwrap().content_hash;
        let mut file = std::fs::File::open(&fixture.path).unwrap();
        let full_hash =
            read_content_hash(&mut file, fixture.path.metadata().unwrap().len()).unwrap();
        assert_eq!(cached_hash, Some(full_hash));
    }

    #[test]
    fn version_one_cache_is_fully_revalidated_before_reuse() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = TestSession::new("legacy-cache-migration");
        fixture.write(
            concat!(
                "{\"type\":\"custom-title\",\"customTitle\":\"Migrated title\"}\n",
                "{\"type\":\"user\",\"message\":{\"content\":\"first\"}}\n"
            )
            .as_bytes(),
        );
        let cache_dir = fixture.dir.join("isolated-cache");
        std::fs::create_dir(&cache_dir).unwrap();
        let _environment = EnvGuard::set_path("CSM_CACHE_DIR", &cache_dir);
        let key = fixture.path.to_string_lossy().to_string();
        let legacy_state = ParsedState {
            custom_title: "Stale legacy title".into(),
            ..ParsedState::default()
        };
        let legacy = CachedSession {
            mtime: file_mtime(&fixture.path),
            file_len: fixture.path.metadata().unwrap().len(),
            parsed_offset: fixture.path.metadata().unwrap().len(),
            content_hash: None,
            change_stamp: None,
            growth_probes: None,
            state: legacy_state,
        };
        let mut stored = serde_json::to_value(StoredCache {
            version: 1,
            entries: HashMap::from([(key.clone(), legacy)]),
        })
        .unwrap();
        let legacy_entry = stored
            .get_mut("entries")
            .and_then(|entries| entries.get_mut(&key))
            .and_then(|entry| entry.as_object_mut())
            .unwrap();
        legacy_entry.remove("content_hash");
        legacy_entry.insert("prefix_hash".into(), serde_json::json!(123_u64));
        std::fs::write(cache_file_path(), serde_json::to_vec(&stored).unwrap()).unwrap();

        let mut cache = Cache::load();
        let first = fixture.parse(&mut cache, &HashSet::new());
        assert_eq!(first.title, "Migrated title");
        assert_eq!(
            first.message_count, 1,
            "legacy state is reparsed instead of being blessed by a new change stamp"
        );
        assert!(cache.entries.get(&key).unwrap().content_hash.is_some());

        fixture.append(b"{\"type\":\"user\",\"message\":{\"content\":\"second\"}}");
        let second = fixture.parse(&mut cache, &HashSet::new());
        assert_eq!(
            second.message_count, 2,
            "a legacy append forces a fresh parse"
        );
        assert_eq!(second.title, "Migrated title");
        assert!(cache.entries.get(&key).unwrap().content_hash.is_some());
        cache.persist_if_needed();
        let bytes = std::fs::read(cache_file_path()).unwrap();
        let upgraded = serde_json::from_slice::<StoredCache>(&bytes).unwrap();
        assert_eq!(upgraded.version, CACHE_FORMAT_VERSION);
        assert!(upgraded.entries.get(&key).unwrap().content_hash.is_some());
    }

    #[test]
    fn concurrent_cache_writers_use_unique_atomic_files() {
        let fixture = TestSession::new("concurrent-cache-writers");
        fixture.write(b"{\"type\":\"custom-title\",\"customTitle\":\"template\"}\n");
        let mut initial = Cache::new();
        fixture.parse(&mut initial, &HashSet::new());
        let key = fixture.path.to_string_lossy().to_string();
        let template = initial.entries.get(&key).unwrap().clone();
        let path = fixture.dir.join("cache/session-metadata-v1.json");
        let threads = (0..16)
            .map(|index| {
                let path = path.clone();
                let key = key.clone();
                let mut entry = template.clone();
                entry.state.custom_title = format!("writer-{index}");
                std::thread::spawn(move || {
                    let mut cache = Cache {
                        entries: HashMap::from([(key, entry)]),
                        dirty: true,
                    };
                    cache.persist_to_path(path);
                    !cache.dirty
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            assert!(
                thread.join().unwrap(),
                "every unique temporary file must be renamed"
            );
        }
        let bytes = std::fs::read(&path).unwrap();
        let stored = serde_json::from_slice::<StoredCache>(&bytes).unwrap();
        assert_eq!(stored.entries.len(), 1);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
                .count(),
            0
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn non_unicode_claude_paths_are_not_listed_as_lossy_broken_sessions() {
        use std::os::unix::ffi::OsStringExt;

        let fixture = TestSession::new("non-unicode-claude-path");
        let directory = fixture
            .dir
            .join(std::ffi::OsString::from_vec(vec![b'p', b'x', 0xff]));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join(format!("{SESSION_ID}.jsonl"));
        std::fs::write(
            &path,
            b"{\"type\":\"custom-title\",\"customTitle\":\"hidden\"}\n",
        )
        .unwrap();
        let mut cache = Cache::new();
        assert!(
            parse_session_file_with_cache(&path, "test", &HashSet::new(), &mut cache).is_none()
        );
        assert!(cache.entries.is_empty());
    }

    #[test]
    fn unchanged_cache_recomputes_favorite_state() {
        let fixture = TestSession::new("favorite");
        let mut cache = Cache::new();
        let mut favs = HashSet::new();
        fixture.write(
            b"{\"type\":\"user\",\"timestamp\":\"2026-11-23T01:00:00Z\",\"message\":{\"content\":\"hello\"}}\n",
        );
        assert!(!fixture.parse(&mut cache, &favs).favorited);
        favs.insert(SESSION_ID.to_string());
        assert!(fixture.parse(&mut cache, &favs).favorited);
    }

    #[test]
    fn codex_cache_keys_normalize_equivalent_paths() {
        let fixture = TestSession::new("codex-cache-key");
        fixture.write(b"{\"type\":\"response_item\"}\n");
        let equivalent = fixture
            .dir
            .join(".")
            .join(fixture.path.file_name().expect("fixture filename"));
        let direct = codex_cache_key(&fixture.path);
        assert_eq!(direct, codex_cache_key(&equivalent));
        assert_eq!(
            direct,
            format!(
                "codex:{}",
                std::fs::canonicalize(&fixture.path)
                    .unwrap()
                    .to_string_lossy()
            )
        );
    }

    #[test]
    fn claude_metadata_filter_preserves_mixed_real_user_text() {
        let event = serde_json::json!({
            "type": "user",
            "message": {"content": "real before\n<system-reminder>private metadata</system-reminder>\nreal after"}
        });
        let parsed = parse_claude_chat_msg(&event).unwrap();
        assert!(parsed.text.contains("real before"));
        assert!(parsed.text.contains("real after"));
        assert!(!parsed.text.contains("private metadata"));
        let mut state = ParsedState::default();
        state.consume_line(event.to_string().as_bytes());
        assert_eq!(state.user_count, 1);

        let caveat = strip_claude_meta_noise(
            "Caveat: generated text\n\n<command-name>/clear</command-name>\nReal request",
        );
        assert_eq!(caveat, "Real request");
        let pure = serde_json::json!({
            "type": "user",
            "message": {"content": "<system-reminder>metadata only</system-reminder>"}
        });
        assert!(parse_claude_chat_msg(&pure).is_none());
    }

    #[test]
    fn codex_messages_skip_environment_and_keep_real_content() {
        let user = serde_json::json!({
            "timestamp": "2026-11-23T01:00:00Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [
                    {"type": "input_text", "text": "<environment_context>private metadata</environment_context>"},
                    {"type": "input_text", "text": "真正的用户问题"}
                ]
            }
        });
        let parsed = parse_codex_chat_msg(&user).unwrap();
        assert_eq!(parsed.role, "user");
        assert_eq!(parsed.text, "真正的用户问题");

        let assistant = serde_json::json!({
            "timestamp": "2026-11-23T01:01:00Z",
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "回答内容"}]
            }
        });
        assert_eq!(parse_codex_chat_msg(&assistant).unwrap().text, "回答内容");
    }

    #[test]
    fn codex_tool_calls_have_compact_summaries() {
        let tool = serde_json::json!({
            "timestamp": "2026-11-23T01:00:00Z",
            "type": "response_item",
            "payload": {"type": "custom_tool_call", "name": "shell"}
        });
        let parsed = parse_codex_chat_msg(&tool).unwrap();
        assert_eq!(parsed.role, "assistant");
        assert_eq!(parsed.text, "[工具调用: shell]");
    }

    #[test]
    fn codex_message_counts_are_cached_and_incremental() {
        let fixture = TestSession::new("codex-count");
        let user = |text: &str| {
            serde_json::json!({
                "timestamp": "2026-11-23T01:00:00Z",
                "type": "response_item",
                "payload": {
                    "type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": text}]
                }
            })
            .to_string()
                + "\n"
        };
        let assistant = serde_json::json!({
            "type": "response_item",
            "payload": {
                "type": "message", "role": "assistant",
                "content": [{"type": "output_text", "text": "回答"}]
            }
        })
        .to_string()
            + "\n";
        fixture.write(
            (user("<environment_context>metadata</environment_context>")
                + &user("第一个问题")
                + &assistant)
                .as_bytes(),
        );
        let cache_key = format!("codex:{}", fixture.path.to_string_lossy());
        let mut cache = Cache::new();
        let (state, _) = parsed_state_with_cache(
            &fixture.path,
            cache_key.clone(),
            Provider::Codex,
            &mut cache,
        )
        .unwrap();
        assert_eq!(state.user_count, 1);

        fixture.append(user("第二个问题").as_bytes());
        let (state, _) =
            parsed_state_with_cache(&fixture.path, cache_key, Provider::Codex, &mut cache).unwrap();
        assert_eq!(state.user_count, 2);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn in_process_applescript_returns_success_and_error_without_panicking() {
        assert_eq!(
            run_applescript("return \"CSM_IN_PROCESS_OK\"").unwrap(),
            "CSM_IN_PROCESS_OK"
        );
        let error = run_applescript("error \"CSM_EXPECTED_TEST_ERROR\" number 1234").unwrap_err();
        assert!(error.contains("CSM_EXPECTED_TEST_ERROR"), "{error}");
    }

    #[test]
    fn codex_names_compare_instants_and_use_file_order_for_ties() {
        let input = [
            serde_json::json!({"id":"thread","thread_name":"early offset","updated_at":"2026-11-24T00:30:00+08:00"}),
            serde_json::json!({"id":"thread","thread_name":"later UTC","updated_at":"2026-11-23T23:00:00Z"}),
            serde_json::json!({"id":"thread","thread_name":"fractional newest","updated_at":"2026-11-23T23:00:00.500Z"}),
            // Lexicographically this whole-second value looks newer than the
            // fractional value above, but its instant is older.
            serde_json::json!({"id":"thread","thread_name":"older whole second","updated_at":"2026-11-23T23:00:00Z"}),
            serde_json::json!({"id":"thread","thread_name":"tie later in file","updated_at":"2026-11-24T07:00:00.500+08:00"}),
            serde_json::json!({"id":"thread","thread_name":"missing time","updated_at":null}),
        ]
        .into_iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let names = load_codex_names_from_reader(std::io::Cursor::new(input));
        assert_eq!(
            names.get("thread").map(String::as_str),
            Some("tie later in file")
        );
    }

    #[test]
    fn launch_cwd_never_silently_falls_back_for_invalid_nonempty_paths() {
        let fixture = TestSession::new("launch-cwd");
        assert_eq!(
            validated_launch_cwd(&fixture.dir.to_string_lossy()).unwrap(),
            fixture.dir
        );
        fixture.write(b"not a directory");
        let file_error = validated_launch_cwd(&fixture.path.to_string_lossy()).unwrap_err();
        assert!(file_error.contains("不是目录") && file_error.contains("未回退到 home"));
        let missing = fixture.dir.join("missing");
        let missing_error = validated_launch_cwd(&missing.to_string_lossy()).unwrap_err();
        assert!(
            missing_error.contains("项目目录不可用") && missing_error.contains("未回退到 home")
        );
        assert!(validated_launch_cwd("").unwrap().is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn explicit_claude_project_symlink_is_consistent_for_listing_and_validation() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = TestSession::new("claude-project-symlink");
        let config = fixture.dir.join("config");
        let projects = config.join("projects");
        let external_project = fixture.dir.join("external-project");
        std::fs::create_dir_all(&projects).unwrap();
        std::fs::create_dir(&external_project).unwrap();
        let transcript = external_project.join(format!("{SESSION_ID}.jsonl"));
        std::fs::write(&transcript, claude_message("linked project")).unwrap();
        let outside_id = "00000000-0000-4000-8000-0000000000ad";
        let outside = fixture.dir.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let outside_transcript = outside.join(format!("{outside_id}.jsonl"));
        std::fs::write(&outside_transcript, claude_message("outside root")).unwrap();
        std::os::unix::fs::symlink(
            &outside_transcript,
            external_project.join(format!("{outside_id}.jsonl")),
        )
        .unwrap();
        let linked = projects.join("linked-project");
        std::os::unix::fs::symlink(&external_project, &linked).unwrap();
        let _claude_environment = EnvGuard::set_path("CLAUDE_CONFIG_DIR", &config);
        let _cache_environment = EnvGuard::set_path("CSM_CACHE_DIR", &fixture.dir.join("cache"));

        let listed = get_claude_sessions(&HashSet::new()).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(
            listed[0].file_path,
            linked.join(format!("{SESSION_ID}.jsonl")).to_string_lossy()
        );
        let validated = validate_session_file(Provider::Claude, SESSION_ID, &listed[0].file_path)
            .expect("an explicitly listed direct project symlink passes subsequent validation");
        let page = try_read_messages_page(&validated, None, None, 0, 20, Provider::Claude).unwrap();
        assert_eq!(page.messages.len(), 1);
        let launch = validate_session_launch(
            Provider::Claude,
            SESSION_ID,
            &external_project.to_string_lossy(),
            &listed[0].file_path,
        )
        .unwrap();
        assert_eq!(launch.cwd, external_project);
        let output_dir = fixture.dir.join("exports");
        let exported = export_markdown_from_file(
            Provider::Claude,
            SESSION_ID,
            "Linked",
            "/tmp/project",
            "now",
            &validated,
            &output_dir,
        )
        .unwrap();
        assert_eq!(exported.message_count, 1);
        std::fs::remove_file(exported.file_path).unwrap();
        assert!(
            validate_session_file(
                Provider::Claude,
                outside_id,
                &external_project
                    .join(format!("{outside_id}.jsonl"))
                    .to_string_lossy(),
            )
            .is_err(),
            "a file link inside an allowed project may not escape that explicit root"
        );
    }

    #[test]
    fn storage_stats_validate_top_level_ids_and_count_only_nested_records() {
        let fixture = TestSession::new("storage-stats");
        let projects = fixture.dir.join("projects");
        let slug_a = projects.join("project-a");
        let slug_b = projects.join("project-b");
        let nested = slug_a.join("subagents");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&slug_b).unwrap();
        let second_id = "00000000-0000-4000-8000-000000000002";
        let history_only_id = "00000000-0000-4000-8000-000000000003";
        for path in [
            slug_a.join(format!("{SESSION_ID}.jsonl")),
            slug_b.join(format!("{SESSION_ID}.jsonl")), // duplicate, still top-level
            slug_b.join(format!("{second_id}.jsonl")),
            slug_a.join("not-a-uuid.jsonl"), // neither recoverable nor subagent
            nested.join(format!("{SESSION_ID}.jsonl")),
            nested.join("worker.jsonl"),
        ] {
            std::fs::write(path, b"{}\n").unwrap();
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&projects, slug_a.join("linked-projects")).unwrap();
            std::os::unix::fs::symlink(
                slug_a.join(format!("{SESSION_ID}.jsonl")),
                slug_a.join("linked-session.jsonl"),
            )
            .unwrap();
        }
        let history = fixture.dir.join("history.jsonl");
        std::fs::write(
            &history,
            format!(
                "{}\n{}\n",
                serde_json::json!({"sessionId":SESSION_ID}),
                serde_json::json!({"sessionId":history_only_id})
            ),
        )
        .unwrap();
        assert_eq!(
            claude_storage_stats_from_roots(&projects, &history).unwrap(),
            ClaudeStorageStats {
                recoverable: 2,
                history_ids: 2,
                history_only: 1,
                subagent_records: 2,
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_storage_stats_permission_errors_are_not_zeroed() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = TestSession::new("storage-stats-permission");
        let projects = fixture.dir.join("projects");
        let project = projects.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join(format!("{SESSION_ID}.jsonl")), b"{}\n").unwrap();
        let history = fixture.dir.join("history.jsonl");
        std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o000)).unwrap();
        let stats = claude_storage_stats_from_roots(&projects, &history);
        std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            stats.is_err(),
            "a permission failure must be returned instead of zero recoverable sessions"
        );
    }

    #[cfg(unix)]
    #[test]
    fn claude_directory_read_errors_do_not_clear_valid_cache() {
        use std::os::unix::fs::PermissionsExt;

        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = TestSession::new("claude-enumeration-error");
        let config = fixture.dir.join("config");
        let projects = config.join("projects");
        let slug = projects.join("project");
        std::fs::create_dir_all(&slug).unwrap();
        let transcript = slug.join(format!("{SESSION_ID}.jsonl"));
        std::fs::write(
            &transcript,
            b"{\"type\":\"custom-title\",\"customTitle\":\"Keep cached\"}\n",
        )
        .unwrap();
        let _claude_environment = EnvGuard::set_path("CLAUDE_CONFIG_DIR", &config);
        let _cache_environment = EnvGuard::set_path("CSM_CACHE_DIR", &fixture.dir.join("cache"));
        let favorites = HashSet::new();
        assert_eq!(get_claude_sessions(&favorites).unwrap().len(), 1);
        let key = transcript.to_string_lossy().to_string();
        assert!(CACHE.lock().unwrap().entries.contains_key(&key));

        std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o000)).unwrap();
        let after_error = get_claude_sessions(&favorites);
        std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(
            after_error.is_err(),
            "an enumeration error must be returned instead of masquerading as an empty provider"
        );
        assert!(CACHE.lock().unwrap().entries.contains_key(&key));
    }

    #[test]
    fn single_terminal_result_requires_window_tty_and_exact_shape() {
        assert_eq!(
            parse_terminal_single_result(
                "TERMINAL_SINGLE_OK WINDOW_ID 42 TAB_TTY /dev/ttys007 TRUST_CONFIRMED false"
            ),
            Some(TerminalSingleMetadata {
                window_id: 42,
                tty: "/dev/ttys007".into(),
            })
        );
        assert!(parse_terminal_single_result("TERMINAL_SINGLE_OK TRUST_CONFIRMED true").is_none());
        assert!(parse_terminal_single_result(
            "TERMINAL_SINGLE_OK WINDOW_ID 42 TAB_TTY not-a-tty TRUST_CONFIRMED true"
        )
        .is_none());
        let error = unconfirmed_launch_error(1, 0);
        assert!(error.contains("CLI 可能不存在"));
        assert!(error.contains("恢复可能已立即退出"));
        assert!(error.contains("Trust 尚未确认"));
        assert!(error.contains("未报告打开成功"));
    }

    #[test]
    fn macos_autostart_validation_checks_contents_and_loaded_executable() {
        let executable =
            Path::new("/Applications/Claude & Session <Manager>.app/Contents/MacOS/csm-app");
        let plist = macos_autostart_plist_contents(executable);
        assert!(plist.contains("Claude &amp; Session &lt;Manager&gt;"));
        assert_ne!(
            plist,
            macos_autostart_plist_contents(Path::new("/old/csm-app"))
        );
        let loaded = format!(
            "{MACOS_AUTOSTART_LABEL} = {{ program = {}; state = running; }}",
            executable.to_string_lossy()
        );
        assert!(launchctl_output_matches_current_executable(
            loaded.as_bytes(),
            executable
        ));
        assert!(!launchctl_output_matches_current_executable(
            loaded.as_bytes(),
            Path::new("/Applications/Replaced.app/Contents/MacOS/csm-app")
        ));
        assert!(!launchctl_output_matches_current_executable(
            b"launchctl: could not find service",
            executable
        ));
    }

    #[test]
    fn failed_autostart_write_restores_or_removes_only_the_test_file() {
        let fixture = TestSession::new("autostart-rollback");
        let path = fixture.dir.join("com.claude.sessionmanager.plist");
        std::fs::write(&path, b"partial new configuration").unwrap();
        restore_autostart_file(&path, &None).unwrap();
        assert!(!path.exists(), "a newly created failed plist is removed");

        let previous_contents = b"previous configuration".to_vec();
        let previous = Some(previous_contents.clone());
        std::fs::write(&path, b"partial replacement").unwrap();
        restore_autostart_file(&path, &previous).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), previous_contents);
    }

    #[test]
    fn failed_autostart_setup_restores_an_existing_loaded_registration() {
        let fixture = TestSession::new("autostart-restore-loaded-registration");
        let path = fixture.dir.join("com.claude.sessionmanager.plist");
        let previous_contents = b"previous executable registration".to_vec();
        let previous = Some(previous_contents.clone());
        std::fs::write(&path, b"new executable registration").unwrap();
        let events = std::cell::RefCell::new(Vec::new());
        let mut unload = || {
            assert_eq!(
                std::fs::read(&path).unwrap(),
                b"new executable registration"
            );
            events.borrow_mut().push("unload-new-before-file-restore");
            // A failed legacy unload exit code does not override an independently
            // verified successful reload of the previous registration.
            false
        };
        let mut load = || {
            events.borrow_mut().push("load-previous");
            true
        };
        let mut loaded = || {
            events.borrow_mut().push("verify-previous-loaded");
            true
        };
        let message = rollback_failed_autostart_setup(
            &path,
            &previous,
            true,
            &mut unload,
            &mut load,
            &mut loaded,
        )
        .unwrap();
        assert!(message.contains("恢复此前已加载"));
        assert_eq!(std::fs::read(&path).unwrap(), previous_contents);
        assert_eq!(
            events.into_inner(),
            vec![
                "unload-new-before-file-restore",
                "load-previous",
                "verify-previous-loaded"
            ]
        );
    }

    #[test]
    fn failed_autostart_setup_unloads_before_removing_a_new_plist() {
        let fixture = TestSession::new("autostart-remove-new-registration");
        let path = fixture.dir.join("com.claude.sessionmanager.plist");
        std::fs::write(&path, b"new executable registration").unwrap();
        let mut unload = || {
            assert!(path.exists(), "unload must happen before plist removal");
            true
        };
        let verification_calls = std::cell::Cell::new(0);
        let mut load = || panic!("a previously unloaded service must not be reloaded");
        let mut loaded = || {
            verification_calls.set(verification_calls.get() + 1);
            false
        };
        rollback_failed_autostart_setup(&path, &None, false, &mut unload, &mut load, &mut loaded)
            .unwrap();
        assert!(!path.exists());
        assert_eq!(verification_calls.get(), 1);
    }

    #[test]
    fn windows_session_launch_locks_serialize_aliases_without_global_blocking() {
        let id = format!("lock-test-{}", std::process::id());
        let first = SessionLaunchGuards::acquire(vec![session_launch_key(
            Provider::Claude,
            &id.to_ascii_uppercase(),
        )]);
        let same = session_launch_key(Provider::Claude, &id);
        let (same_tx, same_rx) = std::sync::mpsc::channel();
        let same_thread = std::thread::spawn(move || {
            let _guard = SessionLaunchGuards::acquire(vec![same]);
            same_tx.send(()).unwrap();
        });
        assert!(
            same_rx
                .recv_timeout(std::time::Duration::from_millis(100))
                .is_err(),
            "the same provider/session must wait even when UUID casing differs"
        );
        drop(first);
        same_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        same_thread.join().unwrap();

        let held = SessionLaunchGuards::acquire(vec![session_launch_key(Provider::Claude, &id)]);
        let unrelated = [
            session_launch_key(Provider::Codex, &id),
            session_launch_key(Provider::Claude, &format!("{id}-other")),
        ];
        let (free_tx, free_rx) = std::sync::mpsc::channel();
        let threads = unrelated
            .into_iter()
            .map(|key| {
                let tx = free_tx.clone();
                std::thread::spawn(move || {
                    let _guard = SessionLaunchGuards::acquire(vec![key]);
                    tx.send(()).unwrap();
                })
            })
            .collect::<Vec<_>>();
        for _ in 0..2 {
            free_rx
                .recv_timeout(std::time::Duration::from_millis(250))
                .expect("an unrelated provider/session must not wait for the held key");
        }
        drop(held);
        for thread in threads {
            thread.join().unwrap();
        }
    }

    #[test]
    fn macos_batch_new_tab_action_is_locale_independent() {
        let script = build_terminal_batch_script(&[
            "printf fixture".to_string(),
            "printf second".to_string(),
        ]);
        assert!(!script.contains("新建标签页"));
        assert!(script.contains("keystroke \"t\" using command down"));
        assert!(!script.contains("menu item defaultProfile"));
    }

    #[test]
    fn macos_transient_provider_match_does_not_confirm_launch() {
        let start = std::time::Instant::now();
        let mut stable_since = None;
        assert!(!macos_stable_launch_confirmed(
            &mut stable_since,
            1,
            1,
            start
        ));
        assert!(!macos_stable_launch_confirmed(
            &mut stable_since,
            0,
            1,
            start + std::time::Duration::from_millis(500)
        ));
        assert!(stable_since.is_none());
        let restarted = start + std::time::Duration::from_millis(1000);
        assert!(!macos_stable_launch_confirmed(
            &mut stable_since,
            1,
            1,
            restarted
        ));
        assert!(macos_stable_launch_confirmed(
            &mut stable_since,
            1,
            1,
            restarted + MACOS_PROVIDER_STABLE_FOR
        ));
    }

    #[test]
    fn macos_launch_stability_requires_complete_batch_and_real_interval() {
        let start = std::time::Instant::now();
        let mut stable_since = None;
        assert!(!macos_stable_launch_confirmed(
            &mut stable_since,
            0,
            0,
            start
        ));
        assert!(!macos_stable_launch_confirmed(
            &mut stable_since,
            1,
            2,
            start
        ));
        assert!(stable_since.is_none());
        assert!(!macos_stable_launch_confirmed(
            &mut stable_since,
            2,
            2,
            start + std::time::Duration::from_millis(100)
        ));
        assert!(!macos_stable_launch_confirmed(
            &mut stable_since,
            2,
            2,
            start + std::time::Duration::from_millis(999)
        ));
        assert!(macos_stable_launch_confirmed(
            &mut stable_since,
            2,
            2,
            start + std::time::Duration::from_millis(1100)
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn single_terminal_script_returns_owned_window_and_tty() {
        let script = build_terminal_single_script("cd /tmp && claude -r test");
        assert!(script.contains("set launchedTab to do script sessionCommand"));
        assert!(script.contains("set launchedTty to tty of launchedTab"));
        assert!(script.contains("TERMINAL_SINGLE_OK WINDOW_ID"));
        assert!(!script.contains("beforeWindowIds"));
    }

    #[test]
    fn terminal_batch_result_requires_exact_owned_metadata() {
        assert!(valid_terminal_batch_result(
            "TERMINAL_BATCH_OK 2 TAB_IDS 11,12 TAB_TTYS /dev/ttys001,/dev/ttys002",
            2
        ));
        assert!(!valid_terminal_batch_result(
            "TERMINAL_BATCH_OK 3 TAB_IDS 11,12 TAB_TTYS /dev/ttys001,/dev/ttys002",
            3
        ));
        assert!(!valid_terminal_batch_result(
            "TERMINAL_BATCH_OK 1 TAB_IDS 11 TAB_TTYS /dev/ttys001 unexpected",
            1
        ));
        assert!(!valid_terminal_batch_result(
            "TERMINAL_BATCH_OK 1 TAB_IDS 11 TAB_TTYS not-a-tty",
            1
        ));
        assert!(!valid_terminal_batch_result(
            "TERMINAL_BATCH_OK 2 TAB_IDS 11,11 TAB_TTYS /dev/ttys001,/dev/ttys002",
            2
        ));
        assert!(!valid_terminal_batch_result(
            "TERMINAL_BATCH_OK 2 TAB_IDS 11,12 TAB_TTYS /dev/ttys001,/dev/ttys001",
            2
        ));
    }

    #[test]
    fn batch_terminal_script_only_creates_one_window_and_native_tabs() {
        let commands = vec![
            "cd /tmp && first".to_string(),
            "cd /tmp && second".to_string(),
        ];
        let script = build_terminal_batch_script(&commands);
        assert!(script.contains("set initialTab to do script \"\""));
        assert!(script.contains("set initialTty to tty of initialTab"));
        assert!(script.contains("keystroke \"t\" using command down"));
        assert!(!script.contains("新建标签页"));
        assert!(!script.contains("menu item defaultProfile"));
        assert!(script.contains(
            "set launchedTab to do script (item tabNumber of sessionCommands) in targetTab"
        ));
        assert!(script.contains(
            "if (tty of launchedTab) is not (item tabNumber of expectedTtys) then error"
        ));
        assert!(script.contains("set selected of tab 1 of anchorWindow to true"));
        assert!(script.contains("set index of anchorWindow to 1"));
        assert!(script.contains("if (id of front window) is anchorWindowId then"));
        assert!(!script.contains("if frontmost of anchorWindow then"));
        assert!(script.contains("set end of createdTabWindowIds to newTabWindowId"));
        assert!(script.contains("set targetWindowId to item tabNumber of createdTabWindowIds"));
        assert!(!script.contains("set sessionWindow to front window"));
        assert!(!script.contains("set index of sessionWindow"));
        assert!(!script.contains("in selected tab of sessionWindow"));
        let create_tabs = script
            .find("repeat with tabNumber from 2 to sessionCount")
            .unwrap();
        let verify_tabs = script
            .find("标签页对象预创建数量与所选会话数量不一致")
            .unwrap();
        let launch_sessions = script
            .find("repeat with tabNumber from 1 to sessionCount")
            .unwrap();
        assert!(create_tabs < verify_tabs && verify_tabs < launch_sessions);
        assert!(script.contains("cd /tmp && first"));
        assert!(script.contains("cd /tmp && second"));
        assert!(!script.contains("close "));
        assert!(!script.contains("quit"));
        assert!(!script.contains("AX"));
        assert!(!script.contains("do shell script"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn batch_terminal_script_compiles_without_running_it() {
        let fixture = TestSession::new("applescript-compile");
        let output_path = fixture.dir.join("batch.scpt");
        let script = build_terminal_batch_script(&[
            "cd /tmp && first".to_string(),
            "cd /tmp && second".to_string(),
            "cd /tmp && third".to_string(),
        ]);
        let output = Command::new("osacompile")
            .arg("-o")
            .arg(&output_path)
            .arg("-e")
            .arg(script)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "AppleScript compilation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output_path.is_file());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "opt-in real Terminal smoke; cleans up only test object IDs with matching TTYs"]
    fn real_terminal_batch_smoke() {
        let commands = vec!["/bin/sleep 10".to_string(); 3];
        let script = build_terminal_batch_script(&commands);

        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let list_after = |text: &str, key: &str| {
            text.find(key)
                .and_then(|start| {
                    text[start + key.len()..]
                        .split_whitespace()
                        .next()
                        .map(str::to_string)
                })
                .unwrap_or_default()
        };
        let ids_text = {
            let value = list_after(&stdout, "TAB_IDS ");
            if value.is_empty() {
                list_after(&stderr, " tabIds=")
            } else {
                value
            }
        };
        let ttys_text = {
            let value = list_after(&stdout, "TAB_TTYS ");
            if value.is_empty() {
                list_after(&stderr, " tabTtys=")
            } else {
                value
            }
        };
        let tab_ids = ids_text
            .split(',')
            .filter_map(|value| value.parse::<i64>().ok())
            .collect::<Vec<_>>();
        let tab_ttys = ttys_text
            .split(',')
            .filter(|value| value.starts_with("/dev/"))
            .map(str::to_string)
            .collect::<Vec<_>>();

        let verification = if output.status.success() && tab_ids.len() == 3 && tab_ttys.len() == 3 {
            let native_anchor_id = tab_ids[2];
            let mut verify_steps = String::new();
            for (window_id, tty) in tab_ids.iter().zip(&tab_ttys) {
                verify_steps.push_str(&format!(
                    r#"  if not (exists window id {window_id}) then error "missing test object {window_id}"
  set testWindow to window id {window_id}
  if (count of tabs of testWindow) is not 1 then error "test script object contains multiple tabs"
  set testTab to tab 1 of testWindow
  if (tty of testTab) is not "{tty}" then error "test TTY mismatch"
  if not (busy of testTab) then error "test command is not running"
  set processNames to (processes of testTab) as text
  if processNames does not contain "sleep" then error "sleep process missing from test tab"
"#
                ));
            }
            let script = format!(
                r##"tell application "Terminal"
{verify_steps}  set nativeAnchor to window id {native_anchor_id}
  set selected of tab 1 of nativeAnchor to true
  set index of nativeAnchor to 1
  activate
  delay 0.2
  if (id of front window) is not {native_anchor_id} then error "owned Terminal object is not the current script window"
  set ownedBounds to bounds of nativeAnchor
end tell
tell application "System Events" to tell process "Terminal"
  if (count of windows) is 0 then error "Terminal exposed no native windows"
  set nativeWindow to window 1
  set nativePosition to position of nativeWindow
  set nativeSize to size of nativeWindow
  if (item 1 of nativePosition) is not (item 1 of ownedBounds) then error "native window X does not match the owned Terminal object"
  if (item 2 of nativePosition) is not (item 2 of ownedBounds) then error "native window Y does not match the owned Terminal object"
  set expectedWidth to (item 3 of ownedBounds) - (item 1 of ownedBounds)
  set expectedHeight to (item 4 of ownedBounds) - (item 2 of ownedBounds)
  if (item 1 of nativeSize) is not expectedWidth then error "native window width does not match the owned Terminal object"
  if (item 2 of nativeSize) is not expectedHeight then error "native window height does not match the owned Terminal object"
  set tabGroups to UI elements of nativeWindow whose role is "AXTabGroup"
  if (count of tabGroups) is not 1 then error "expected one native tab group, got " & (count of tabGroups)
  set tabButtons to UI elements of item 1 of tabGroups whose role is "AXRadioButton"
  if (count of tabButtons) is not 3 then error "expected 3 native tabs, got " & (count of tabButtons)
end tell
return "TERMINAL_SMOKE_VERIFIED 3 NATIVE_TABS 3""##
            );
            let result = Command::new("osascript")
                .arg("-e")
                .arg(script)
                .output()
                .unwrap();
            format!(
                "status={} stdout={} stderr={}",
                result.status,
                String::from_utf8_lossy(&result.stdout).trim(),
                String::from_utf8_lossy(&result.stderr).trim()
            )
        } else {
            "SKIPPED_PRIMARY_FAILED".to_string()
        };

        let cleanup_template = r##"set candidateIds to {__IDS__}
set candidateTtys to {__TTYS__}
tell application "Terminal"
  if (count of candidateIds) is 0 then return "ALREADY_GONE"
  if (count of candidateIds) is not (count of candidateTtys) then return "CLEANUP_REFUSED_METADATA"
  if (count of candidateIds) is greater than 3 then return "CLEANUP_REFUSED_AMBIGUOUS"

  set allIdle to false
  repeat 200 times
    set allIdle to true
    repeat with objectNumber from 1 to (count of candidateIds)
      set wid to item objectNumber of candidateIds
      set expectedTty to item objectNumber of candidateTtys
      if exists window id wid then
        set targetWindow to window id wid
        if (count of tabs of targetWindow) is not 1 then return "CLEANUP_REFUSED_OWNERSHIP"
        set targetTab to tab 1 of targetWindow
        if (tty of targetTab) is not expectedTty then return "CLEANUP_REFUSED_TTY"
        if busy of targetTab then set allIdle to false
      end if
    end repeat
    if allIdle then exit repeat
    delay 0.1
  end repeat
  if not allIdle then return "CLEANUP_REFUSED_BUSY"

  -- Every close is guarded by both the object ID returned by this test and its
  -- unique TTY. Pre-existing windows and reused IDs with another TTY are refused.
  repeat with objectNumber from 1 to (count of candidateIds)
    set wid to item objectNumber of candidateIds
    set expectedTty to item objectNumber of candidateTtys
    if exists window id wid then
      set targetWindow to window id wid
      if (count of tabs of targetWindow) is not 1 then return "CLEANUP_REFUSED_OWNERSHIP"
      if (tty of tab 1 of targetWindow) is not expectedTty then return "CLEANUP_REFUSED_TTY"
      close targetWindow saving no
    end if
  end repeat
  return "CLOSE_REQUESTED " & (count of candidateIds)
end tell"##;
        let id_list = tab_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let tty_list = tab_ttys
            .iter()
            .map(|tty| applescript_quote(tty))
            .collect::<Vec<_>>()
            .join(",");
        let cleanup_script = cleanup_template
            .replace("__IDS__", &id_list)
            .replace("__TTYS__", &tty_list);
        let cleanup_output = Command::new("osascript")
            .arg("-e")
            .arg(cleanup_script)
            .output()
            .unwrap();
        let cleanup = format!(
            "status={} stdout={} stderr={}",
            cleanup_output.status,
            String::from_utf8_lossy(&cleanup_output.stdout).trim(),
            String::from_utf8_lossy(&cleanup_output.stderr).trim()
        );

        let confirmation_template = r##"set candidateIds to {__IDS__}
set candidateTtys to {__TTYS__}
tell application "Terminal"
  set remainingCount to 0
  repeat with objectNumber from 1 to (count of candidateIds)
    set wid to item objectNumber of candidateIds
    set expectedTty to item objectNumber of candidateTtys
    if exists window id wid then
      set targetWindow to window id wid
      if (count of tabs of targetWindow) is 1 then
        if (tty of tab 1 of targetWindow) is expectedTty then set remainingCount to remainingCount + 1
      end if
    end if
  end repeat
  if remainingCount is not 0 then error "test objects remain: " & remainingCount
  return "TEST_CLEANUP_CONFIRMED"
end tell"##;
        let confirmation_script = confirmation_template
            .replace("__IDS__", &id_list)
            .replace("__TTYS__", &tty_list);
        let mut confirmation_output = None;
        for _ in 0..30 {
            let result = Command::new("osascript")
                .arg("-e")
                .arg(&confirmation_script)
                .output()
                .unwrap();
            let confirmed = result.status.success();
            confirmation_output = Some(result);
            if confirmed {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let confirmation_output = confirmation_output.unwrap();
        let confirmation = format!(
            "status={} stdout={} stderr={}",
            confirmation_output.status,
            String::from_utf8_lossy(&confirmation_output.stdout).trim(),
            String::from_utf8_lossy(&confirmation_output.stderr).trim()
        );

        assert!(
            output.status.success()
                && stdout.starts_with("TERMINAL_BATCH_OK 3 TAB_IDS ")
                && tab_ids.len() == 3
                && tab_ttys.len() == 3,
            "real Terminal batch failed: stdout={stdout} stderr={stderr}; verification={verification}; cleanup={cleanup}; confirmation={confirmation}"
        );
        assert!(
            verification.contains("TERMINAL_SMOKE_VERIFIED 3"),
            "real Terminal verification failed: {verification}; cleanup={cleanup}; confirmation={confirmation}"
        );
        assert!(
            cleanup_output.status.success()
                && (cleanup.contains("stdout=CLOSE_REQUESTED ")
                    || cleanup.contains("stdout=ALREADY_GONE"))
                && confirmation_output.status.success()
                && confirmation.contains("TEST_CLEANUP_CONFIRMED"),
            "test window cleanup was not confirmed: {cleanup}; confirmation={confirmation}; primary={stdout} {stderr}"
        );
        println!("REAL_TERMINAL_SMOKE_OK tabs=3 {verification}; {cleanup}; {confirmation}");
    }

    #[cfg(target_os = "macos")]
    fn run_external_applescript_for_test(script: &str) -> Result<String, String> {
        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|error| format!("unable to start osascript: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if output.status.success() {
            Ok(stdout)
        } else {
            Err(format!(
                "osascript status={} stdout={stdout} stderr={stderr}",
                output.status
            ))
        }
    }

    #[cfg(target_os = "macos")]
    fn terminal_trace_objects_for_test(text: &str) -> Vec<(i64, String)> {
        let field = |keys: &[&str]| {
            keys.iter().find_map(|key| {
                text.find(key).and_then(|start| {
                    text[start + key.len()..]
                        .split_whitespace()
                        .next()
                        .map(str::to_string)
                })
            })
        };
        let ids = field(&["TAB_IDS ", "tabIds="]).unwrap_or_default();
        let ttys = field(&["TAB_TTYS ", "tabTtys="]).unwrap_or_default();
        ids.split(',')
            .filter_map(|value| value.parse::<i64>().ok())
            .zip(
                ttys.split(',')
                    .filter(|value| value.starts_with("/dev/tty"))
                    .map(str::to_string),
            )
            .collect()
    }

    #[cfg(target_os = "macos")]
    fn run_single_provider_launch_for_test(
        launch: &MacTerminalLaunch,
    ) -> Result<(TerminalSingleMetadata, Result<(), String>), String> {
        let result =
            run_external_applescript_for_test(&build_terminal_single_script(&launch.command))?;
        let metadata = parse_terminal_single_result(&result)
            .ok_or_else(|| format!("Terminal 单会话元数据无效: {result}"))?;
        let verification_launch = MacTerminalLaunch {
            provider: launch.provider,
            session_id: launch.session_id.clone(),
            command: launch.command.clone(),
        };
        let verification =
            wait_for_provider_launches(&[verification_launch], std::slice::from_ref(&metadata.tty));
        Ok((metadata, verification))
    }

    #[cfg(target_os = "macos")]
    fn run_batch_provider_launch_for_test(
        launches: &[MacTerminalLaunch],
    ) -> Result<(TerminalBatchMetadata, Result<(), String>), String> {
        let commands = launches
            .iter()
            .map(|launch| launch.command.clone())
            .collect::<Vec<_>>();
        let result = run_external_applescript_for_test(&build_terminal_batch_script(&commands))?;
        let metadata = parse_terminal_batch_result(&result, launches.len())
            .ok_or_else(|| format!("Terminal 批量元数据无效: {result}"))?;
        let verification = wait_for_provider_launches(launches, &metadata.ttys);
        Ok((metadata, verification))
    }

    #[cfg(target_os = "macos")]
    fn owned_fixture_provider_pids(
        system: &sysinfo::System,
        process_ttys: &HashMap<u32, String>,
        providers: &[(Provider, String, String)],
        fixture_script: &str,
    ) -> Vec<u32> {
        system
            .processes()
            .values()
            .filter(|process| {
                let command = process
                    .cmd()
                    .iter()
                    .map(|value| value.to_string_lossy())
                    .collect::<Vec<_>>();
                command.iter().any(|argument| argument == fixture_script)
                    && providers.iter().any(|(provider, session_id, tty)| {
                        provider_process::process_matches_resume(process, *provider, session_id)
                            && process_tty(process, process_ttys).as_ref() == Some(tty)
                    })
            })
            .map(|process| process.pid().as_u32())
            .collect()
    }

    #[cfg(target_os = "macos")]
    fn cleanup_isolated_provider_terminals_for_test(
        objects: &[(i64, String)],
        providers: &[(Provider, String, String)],
        fixture_script: &str,
    ) -> Result<String, String> {
        let mut errors = Vec::new();
        if !providers.is_empty() {
            let system = sysinfo::System::new_all();
            let process_ttys = terminal_process_ttys();
            for pid in
                owned_fixture_provider_pids(&system, &process_ttys, providers, fixture_script)
            {
                let status = Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .status();
                if !status.is_ok_and(|status| status.success()) {
                    // The short-lived fixture may exit after enumeration but before
                    // SIGTERM. Re-enumerating the same strict owned identity below
                    // distinguishes that normal race from a provider that remains.
                    eprintln!("owned provider SIGTERM needs liveness recheck: pid={pid}");
                }
            }
            let mut remaining = Vec::new();
            for _ in 0..50 {
                let system = sysinfo::System::new_all();
                let process_ttys = terminal_process_ttys();
                remaining =
                    owned_fixture_provider_pids(&system, &process_ttys, providers, fixture_script);
                if remaining.is_empty() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if !remaining.is_empty() {
                errors.push(format!("owned providers remain: {}", remaining.len()));
            }
        }

        let ids = objects
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let ttys = objects
            .iter()
            .map(|(_, tty)| applescript_quote(tty))
            .collect::<Vec<_>>()
            .join(",");
        let cleanup = format!(
            r##"set candidateIds to {{{ids}}}
set candidateTtys to {{{ttys}}}
tell application "Terminal"
  if (count of candidateIds) is not (count of candidateTtys) then error "metadata mismatch"
  if (count of candidateIds) is greater than 7 then error "ambiguous object count"
  set allIdle to false
  repeat 400 times
    set allIdle to true
    repeat with objectNumber from 1 to (count of candidateIds)
      set wid to item objectNumber of candidateIds
      set expectedTty to item objectNumber of candidateTtys
      if exists window id wid then
        set targetWindow to window id wid
        if (count of tabs of targetWindow) is not 1 then error "ownership mismatch"
        set targetTab to tab 1 of targetWindow
        if (tty of targetTab) is not expectedTty then error "TTY mismatch"
        if busy of targetTab then set allIdle to false
      end if
    end repeat
    if allIdle then exit repeat
    delay 0.1
  end repeat
  if not allIdle then error "owned tabs remain busy"
  repeat with objectNumber from 1 to (count of candidateIds)
    set wid to item objectNumber of candidateIds
    set expectedTty to item objectNumber of candidateTtys
    if exists window id wid then
      set targetWindow to window id wid
      if (count of tabs of targetWindow) is not 1 then error "ownership changed"
      if (tty of tab 1 of targetWindow) is not expectedTty then error "TTY changed"
      close targetWindow saving no
    end if
  end repeat
  return "OWNED_CLOSE_REQUESTED"
end tell"##
        );
        if let Err(error) = run_external_applescript_for_test(&cleanup) {
            errors.push(format!("owned Terminal cleanup: {error}"));
        }

        let confirmation = format!(
            r##"set candidateIds to {{{ids}}}
set candidateTtys to {{{ttys}}}
tell application "Terminal"
  set remainingCount to 0
  repeat with objectNumber from 1 to (count of candidateIds)
    set wid to item objectNumber of candidateIds
    set expectedTty to item objectNumber of candidateTtys
    if exists window id wid then
      set targetWindow to window id wid
      if (count of tabs of targetWindow) is 1 then
        if (tty of tab 1 of targetWindow) is expectedTty then set remainingCount to remainingCount + 1
      end if
    end if
  end repeat
  if remainingCount is not 0 then error "owned objects remain: " & remainingCount
  return "OWNED_TERMINALS_REMAINING=0"
end tell"##
        );
        let mut confirmed = false;
        for _ in 0..30 {
            match run_external_applescript_for_test(&confirmation) {
                Ok(result) if result.contains("OWNED_TERMINALS_REMAINING=0") => {
                    confirmed = true;
                    break;
                }
                _ => std::thread::sleep(std::time::Duration::from_millis(100)),
            }
        }
        if !confirmed {
            errors.push("owned Terminal objects remain after cleanup".into());
        }
        if errors.is_empty() {
            Ok("OWNED_PROVIDER_PROCESSES_REMAINING=0 OWNED_TERMINALS_REMAINING=0".into())
        } else {
            Err(errors.join("; "))
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "opt-in isolated real Terminal provider confirmation; no real CLI or session content"]
    fn real_terminal_provider_confirmation_and_failures_smoke() {
        if std::env::var("CSM_ISOLATED_TERMINAL_SMOKE").as_deref() != Ok("1") {
            return;
        }
        assert!(
            accessibility_permission_granted(false),
            "accessibility preflight denied; terminal_objects_created=0"
        );
        let fixture = TestSession::new("real-terminal-provider-confirmation");
        let node_output = Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .unwrap();
        assert!(
            node_output.status.success(),
            "Node fixture launcher is unavailable"
        );
        let node = String::from_utf8(node_output.stdout)
            .unwrap()
            .trim()
            .to_string();
        let provider_script = fixture.dir.join("provider-fixture.cjs");
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../smoke-tests/windows/provider-fixture.cjs"
            ),
            &provider_script,
        )
        .unwrap();
        let claude_dir = fixture.dir.join("claude-config");
        let codex_dir = fixture.dir.join("codex-home");
        let project = fixture.dir.join("project");
        let rollout = codex_dir.join("rollout-codex.jsonl");
        for directory in [&claude_dir, &codex_dir, &project] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(&rollout, "{}\n").unwrap();
        let claude_ids = [
            "11111111-1111-4111-8111-111111111111",
            "22222222-2222-4222-8222-222222222222",
            "44444444-4444-4444-8444-444444444444",
        ];
        let codex_id = "33333333-3333-4333-8333-333333333333";
        let config = serde_json::json!({
            "sessions": [
                {"provider":"claude","id":claude_ids[0],"filePath":fixture.dir.join("single.jsonl")},
                {"provider":"claude","id":claude_ids[1],"filePath":fixture.dir.join("batch.jsonl")},
                {"provider":"codex","id":codex_id,"filePath":rollout},
            ],
            "failureSessions": [],
            "claudeDir": claude_dir,
            "codexDir": codex_dir,
            "token": "isolated-no-credentials",
            "logPath": fixture.dir.join("providers.jsonl"),
        });
        let config_path = fixture.dir.join("fixture.json");
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let provider_launch = |provider: Provider, session_id: &str| {
            let (role, flags) = match provider {
                Provider::Claude => ("claude", "--dangerously-skip-permissions -r"),
                Provider::Codex => ("codex", "--dangerously-bypass-approvals-and-sandbox resume"),
            };
            let command = format!(
                "cd {} && env -u CLAUDE_CODE_CHILD_SESSION -u CLAUDECODE CSM_FIXTURE_CONFIG={} {} {} {} {} {}",
                shell_quote(&project.to_string_lossy()),
                shell_quote(&config_path.to_string_lossy()),
                shell_quote(&node),
                shell_quote(&provider_script.to_string_lossy()),
                role,
                flags,
                session_id,
            );
            MacTerminalLaunch {
                provider,
                session_id: session_id.into(),
                command,
            }
        };

        let mut objects = Vec::new();
        let mut providers = Vec::new();
        let mut failures = Vec::new();
        let mut evidence: Vec<String> = Vec::new();
        let single = provider_launch(Provider::Claude, claude_ids[0]);
        eprintln!("ISOLATED_TERMINAL_STAGE single");
        match run_single_provider_launch_for_test(&single) {
            Ok((metadata, outcome)) => {
                objects.push((metadata.window_id, metadata.tty.clone()));
                providers.push((Provider::Claude, single.session_id.clone(), metadata.tty));
                match outcome {
                    Ok(()) => evidence.push("single=opened".into()),
                    Err(error) => failures.push(format!("single positive: {error}")),
                }
            }
            Err(error) => {
                objects.extend(terminal_trace_objects_for_test(&error));
                failures.push(format!("single AppleScript: {error}"));
            }
        }

        // Sequential cross-provider regression: a live Claude process must be
        // detected as itself, while the still-unopened Codex rollout remains
        // launchable. The later batch must not inherit Claude's running state.
        match running_session_token(Provider::Claude, claude_ids[0], "", "") {
            Ok(Some(tty))
                if providers
                    .last()
                    .is_some_and(|(_, _, owned_tty)| *owned_tty == tty) =>
            {
                evidence.push("sequence_claude_running=true".into());
            }
            other => failures.push(format!("Claude running detection mismatch: {other:?}")),
        }
        match running_session_token(Provider::Codex, codex_id, &rollout.to_string_lossy(), "") {
            Ok(None) => evidence.push("sequence_codex_still_launchable=true".into()),
            other => failures.push(format!(
                "unopened Codex was blocked by a running Claude window: {other:?}"
            )),
        }

        let batch = vec![
            provider_launch(Provider::Claude, claude_ids[1]),
            provider_launch(Provider::Codex, codex_id),
        ];
        eprintln!("ISOLATED_TERMINAL_STAGE batch");
        match run_batch_provider_launch_for_test(&batch) {
            Ok((metadata, outcome)) => {
                for (launch, tty) in batch.iter().zip(&metadata.ttys) {
                    providers.push((launch.provider, launch.session_id.clone(), tty.clone()));
                }
                objects.extend(
                    metadata
                        .window_ids
                        .iter()
                        .copied()
                        .zip(metadata.ttys.iter().cloned()),
                );
                match outcome {
                    Ok(()) => evidence.push("batch=opened:2".into()),
                    Err(error) => failures.push(format!("batch positive: {error}")),
                }
            }
            Err(error) => {
                let partial = terminal_trace_objects_for_test(&error);
                for (launch, (_, tty)) in batch.iter().zip(&partial) {
                    providers.push((launch.provider, launch.session_id.clone(), tty.clone()));
                }
                objects.extend(partial);
                failures.push(format!("batch AppleScript: {error}"));
            }
        }

        let negative_commands = [
            format!(
                "{} --dangerously-skip-permissions -r {}",
                shell_quote(&fixture.dir.join("missing-cli").to_string_lossy()),
                claude_ids[2]
            ),
            "/bin/false".into(),
            "/bin/sleep 30".into(),
        ];
        for (name, command) in ["missing-cli", "immediate-exit", "trust-provider-absent"]
            .into_iter()
            .zip(negative_commands)
        {
            let launch = MacTerminalLaunch {
                provider: Provider::Claude,
                session_id: claude_ids[2].into(),
                command,
            };
            eprintln!("ISOLATED_TERMINAL_STAGE negative={name}");
            match run_single_provider_launch_for_test(&launch) {
                Ok((metadata, outcome)) => {
                    objects.push((metadata.window_id, metadata.tty));
                    match outcome {
                        Err(error) if error.contains("0/1") && error.contains("未执行自动清理") =>
                        {
                            evidence.push(name.into());
                        }
                        other => failures.push(format!("negative {name}: {other:?}")),
                    }
                }
                Err(error) => {
                    objects.extend(terminal_trace_objects_for_test(&error));
                    failures.push(format!("negative {name} AppleScript: {error}"));
                }
            }
        }

        eprintln!("ISOLATED_TERMINAL_STAGE cleanup");
        match cleanup_isolated_provider_terminals_for_test(
            &objects,
            &providers,
            &provider_script.to_string_lossy(),
        ) {
            Ok(result) => evidence.push(result),
            Err(error) => failures.push(format!("cleanup: {error}")),
        }
        assert!(
            failures.is_empty(),
            "{}; evidence={evidence:?}",
            failures.join(" | ")
        );
        println!(
            "REAL_TERMINAL_PROVIDER_CONFIRMATION_OK single=1 batch=2 negative=3 cleanup=true evidence={evidence:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "opt-in isolated real Terminal transient Provider regression; no real CLI or session content"]
    fn real_terminal_transient_provider_does_not_confirm_launch() {
        if std::env::var("CSM_ISOLATED_TERMINAL_SMOKE").as_deref() != Ok("1") {
            return;
        }
        assert!(
            accessibility_permission_granted(false),
            "accessibility preflight denied; terminal_objects_created=0"
        );
        let fixture = TestSession::new("real-terminal-transient-provider");
        let node_output = Command::new("node")
            .args(["-p", "process.execPath"])
            .output()
            .unwrap();
        assert!(
            node_output.status.success(),
            "Node fixture launcher is unavailable"
        );
        let node = String::from_utf8(node_output.stdout)
            .unwrap()
            .trim()
            .to_string();
        let provider_script = fixture.dir.join("provider-fixture.cjs");
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../smoke-tests/windows/provider-fixture.cjs"
            ),
            &provider_script,
        )
        .unwrap();
        let claude_dir = fixture.dir.join("claude-config");
        let codex_dir = fixture.dir.join("codex-home");
        let project = fixture.dir.join("project");
        for directory in [&claude_dir, &codex_dir, &project] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let session_id = "55555555-5555-4555-8555-555555555555";
        let config = serde_json::json!({
            "sessions": [],
            "failureSessions": [{
                "provider": "claude",
                "id": session_id,
                "filePath": fixture.dir.join("transient.jsonl"),
                "launchBehavior": "exit-after-ms",
                "exitAfterMs": 7500,
            }],
            "claudeDir": claude_dir,
            "codexDir": codex_dir,
            "token": "isolated-no-credentials",
            "logPath": fixture.dir.join("providers.jsonl"),
        });
        let config_path = fixture.dir.join("fixture.json");
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let command = format!(
            "cd {} && env -u CLAUDE_CODE_CHILD_SESSION -u CLAUDECODE CSM_FIXTURE_CONFIG={} {} {} claude --dangerously-skip-permissions -r {}",
            shell_quote(&project.to_string_lossy()),
            shell_quote(&config_path.to_string_lossy()),
            shell_quote(&node),
            shell_quote(&provider_script.to_string_lossy()),
            session_id,
        );
        let launch = MacTerminalLaunch {
            provider: Provider::Claude,
            session_id: session_id.into(),
            command,
        };
        let mut objects = Vec::new();
        let mut providers = Vec::new();
        let mut failures = Vec::new();
        let mut outcome_description = "not-run".to_string();
        match run_single_provider_launch_for_test(&launch) {
            Ok((metadata, outcome)) => {
                objects.push((metadata.window_id, metadata.tty.clone()));
                providers.push((Provider::Claude, session_id.into(), metadata.tty));
                outcome_description = format!("{outcome:?}");
                match outcome {
                    Ok(()) => failures
                        .push("transient Provider was incorrectly reported as opened".to_string()),
                    Err(error) if error.contains("0/1") && error.contains("未执行自动清理") =>
                        {}
                    Err(error) => {
                        failures.push(format!("unexpected transient Provider error: {error}"))
                    }
                }
            }
            Err(error) => {
                objects.extend(terminal_trace_objects_for_test(&error));
                failures.push(format!("transient Provider AppleScript: {error}"));
            }
        }
        let provider_log_entries = std::fs::read_to_string(fixture.dir.join("providers.jsonl"))
            .map(|log| log.lines().filter(|line| line.contains(session_id)).count())
            .unwrap_or(0);
        if provider_log_entries != 1 {
            failures.push(format!(
                "transient Provider did not produce exactly one start record: {provider_log_entries}"
            ));
        }
        let cleanup = cleanup_isolated_provider_terminals_for_test(
            &objects,
            &providers,
            &provider_script.to_string_lossy(),
        );
        eprintln!(
            "MACOS_TRANSIENT_PROVIDER outcome={outcome_description} provider_log_entries={provider_log_entries} cleanup={cleanup:?} owned_windows={}",
            objects.len()
        );
        if let Err(error) = cleanup {
            failures.push(format!("cleanup: {error}"));
        }
        assert!(failures.is_empty(), "{}", failures.join(" | "));
        println!("REAL_TERMINAL_TRANSIENT_PROVIDER_GUARD_OK negative=1 cleanup=true");
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "opt-in real Claude/Codex resume smoke; no prompts, message bodies, or trust changes"]
    fn real_claude_codex_native_tabs_smoke() {
        if std::env::var("CSM_REAL_TERMINAL_SMOKE").is_err() {
            return;
        }
        let favorites = HashSet::new();
        let protected_claude_id = std::env::var("CSM_PROTECT_CLAUDE_ID").ok();
        let claude_config: serde_json::Value =
            std::fs::read_to_string(dirs::home_dir().unwrap().join(".claude.json"))
                .unwrap()
                .parse()
                .unwrap();
        let claude = get_claude_sessions(&favorites)
            .unwrap()
            .into_iter()
            .find(|session| {
                protected_claude_id.as_deref() != Some(session.session_id.as_str())
                    && find_running_session_tty(&session.session_id).is_none()
                    && Path::new(&session.project_path).is_dir()
                    && claude_config
                        .get("projects")
                        .and_then(|projects| projects.get(&session.project_path))
                        .and_then(|project| project.get("hasTrustDialogAccepted"))
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
            })
            .expect("no stopped Claude session in an already-trusted project");
        let codex = get_codex_sessions(&favorites)
            .unwrap()
            .into_iter()
            .find(|session| {
                session.session_kind == "main"
                    && Path::new(&session.project_path).is_dir()
                    && Path::new(&session.file_path).is_file()
                    && find_codex_session_tty(&session.session_id, &session.file_path).is_none()
            })
            .expect("no stopped Codex session with an existing rollout");

        let commands = [&claude, &codex]
            .into_iter()
            .map(|session| {
                session_launch_command_for(
                    Provider::parse(Some(&session.provider)).unwrap(),
                    &session.session_id,
                    &session.project_path,
                    &session.file_path,
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        let claude_cli = provider_cli::resolve(Provider::Claude).unwrap();
        let codex_cli = provider_cli::resolve(Provider::Codex).unwrap();
        assert!(commands[0].contains(&format!(
            "{} --dangerously-skip-permissions -r ",
            shell_quote(&claude_cli.to_string_lossy())
        )));
        assert!(commands[0].ends_with(&claude.session_id));
        assert!(commands[1].contains(&format!(
            "{} --dangerously-bypass-approvals-and-sandbox resume ",
            shell_quote(&codex_cli.to_string_lossy())
        )));
        assert!(commands[1].ends_with(&codex.session_id));

        let script = build_terminal_batch_script(&commands);
        let output = Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let list_after = |text: &str, key: &str| {
            text.find(key)
                .and_then(|start| {
                    text[start + key.len()..]
                        .split_whitespace()
                        .next()
                        .map(str::to_string)
                })
                .unwrap_or_default()
        };
        let ids_text = {
            let value = list_after(&stdout, "TAB_IDS ");
            if value.is_empty() {
                list_after(&stderr, " tabIds=")
            } else {
                value
            }
        };
        let ttys_text = {
            let value = list_after(&stdout, "TAB_TTYS ");
            if value.is_empty() {
                list_after(&stderr, " tabTtys=")
            } else {
                value
            }
        };
        let tab_ids = ids_text
            .split(',')
            .filter_map(|value| value.parse::<i64>().ok())
            .collect::<Vec<_>>();
        let tab_ttys = ttys_text
            .split(',')
            .filter(|value| value.starts_with("/dev/"))
            .map(str::to_string)
            .collect::<Vec<_>>();

        let ps_pids = |tty: &str, session_id: &str, flag: &str| {
            let output = Command::new("ps")
                .args(["-axo", "pid=,tty=,command="])
                .output()
                .unwrap();
            let tty = tty.trim_start_matches("/dev/");
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace();
                    let pid = fields.next()?.parse::<i64>().ok()?;
                    if fields.next()? != tty || !line.contains(session_id) || !line.contains(flag) {
                        return None;
                    }
                    Some(pid)
                })
                .collect::<Vec<_>>()
        };
        let claude_flag = "claude --dangerously-skip-permissions -r";
        let codex_flag = "codex --dangerously-bypass-approvals-and-sandbox resume";
        let mut claude_pids = Vec::new();
        let mut codex_pids = Vec::new();
        let mut claude_registered = false;
        let mut codex_registered = false;
        if output.status.success() && tab_ttys.len() == 2 {
            for _ in 0..100 {
                claude_pids = tab_ttys
                    .iter()
                    .flat_map(|tty| ps_pids(tty, &claude.session_id, claude_flag))
                    .collect();
                codex_pids = tab_ttys
                    .iter()
                    .flat_map(|tty| ps_pids(tty, &codex.session_id, codex_flag))
                    .collect();
                claude_registered = find_running_session_tty(&claude.session_id)
                    .is_some_and(|tty| tab_ttys.contains(&tty));
                codex_registered = find_codex_session_tty(&codex.session_id, &codex.file_path)
                    .is_some_and(|tty| tab_ttys.contains(&tty));
                if !claude_pids.is_empty()
                    && !codex_pids.is_empty()
                    && claude_registered
                    && codex_registered
                {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        let process_verification = format!(
            "claude_flag_pids={} codex_flag_pids={} claude_registered={} codex_registered={}",
            claude_pids.len(),
            codex_pids.len(),
            claude_registered,
            codex_registered
        );

        let native_verification = if output.status.success() && tab_ids.len() == 2 {
            let token = format!("CSM_PROVIDER_VERIFY_{}", std::process::id());
            let quoted_token = applescript_quote(&token);
            let native_anchor_id = tab_ids[1];
            let mut steps = String::new();
            for (window_id, tty) in tab_ids.iter().zip(&tab_ttys) {
                steps.push_str(&format!(
                    r#"  if not (exists window id {window_id}) then error "missing provider test object"
  set testWindow to window id {window_id}
  if (count of tabs of testWindow) is not 1 then error "provider test object has multiple tabs"
  set testTab to tab 1 of testWindow
  if (tty of testTab) is not "{tty}" then error "provider test TTY mismatch"
  if not (busy of testTab) then error "provider CLI is not running"
  set custom title of testTab to {quoted_token}
  set title displays custom title of testTab to true
"#
                ));
            }
            let script = format!(
                r##"tell application "Terminal"
{steps}  set nativeAnchor to window id {native_anchor_id}
  set selected of tab 1 of nativeAnchor to true
  set index of nativeAnchor to 1
  activate
  delay 0.2
  if (id of front window) is not {native_anchor_id} then error "owned provider object is not the current script window"
  set ownedBounds to bounds of nativeAnchor
end tell
tell application "System Events" to tell process "Terminal"
  if (count of windows) is 0 then error "Terminal exposed no native windows"
  set nativeWindow to window 1
  set nativePosition to position of nativeWindow
  set nativeSize to size of nativeWindow
  if (item 1 of nativePosition) is not (item 1 of ownedBounds) then error "provider native window X does not match the owned object"
  if (item 2 of nativePosition) is not (item 2 of ownedBounds) then error "provider native window Y does not match the owned object"
  set expectedWidth to (item 3 of ownedBounds) - (item 1 of ownedBounds)
  set expectedHeight to (item 4 of ownedBounds) - (item 2 of ownedBounds)
  if (item 1 of nativeSize) is not expectedWidth then error "provider native window width does not match the owned object"
  if (item 2 of nativeSize) is not expectedHeight then error "provider native window height does not match the owned object"
  set tabGroups to UI elements of nativeWindow whose role is "AXTabGroup"
  if (count of tabGroups) is not 1 then error "expected one provider tab group"
  set tabButtons to UI elements of item 1 of tabGroups whose role is "AXRadioButton"
  if (count of tabButtons) is not 2 then error "expected 2 native provider tabs"
end tell
return "REAL_PROVIDER_NATIVE_TABS_OK 2""##
            );
            let result = Command::new("osascript")
                .arg("-e")
                .arg(script)
                .output()
                .unwrap();
            format!(
                "status={} stdout={} stderr={}",
                result.status,
                String::from_utf8_lossy(&result.stdout).trim(),
                String::from_utf8_lossy(&result.stderr).trim()
            )
        } else {
            "SKIPPED_PRIMARY_FAILED".to_string()
        };

        // Terminate only processes on the newly returned test TTYs whose argv also
        // contains both the selected session ID and the exact provider flag.
        for tty in &tab_ttys {
            for (session_id, flag) in [
                (&claude.session_id, claude_flag),
                (&codex.session_id, codex_flag),
            ] {
                for pid in ps_pids(tty, session_id, flag) {
                    let _ = Command::new("kill")
                        .args(["-TERM", &pid.to_string()])
                        .status();
                }
            }
        }

        let id_list = tab_ids
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let tty_list = tab_ttys
            .iter()
            .map(|tty| applescript_quote(tty))
            .collect::<Vec<_>>()
            .join(",");
        let cleanup_template = r##"set candidateIds to {__IDS__}
set candidateTtys to {__TTYS__}
tell application "Terminal"
  if (count of candidateIds) is 0 then return "ALREADY_GONE"
  if (count of candidateIds) is not (count of candidateTtys) then return "CLEANUP_REFUSED_METADATA"
  if (count of candidateIds) is greater than 2 then return "CLEANUP_REFUSED_AMBIGUOUS"
  set allIdle to false
  repeat 200 times
    set allIdle to true
    repeat with objectNumber from 1 to (count of candidateIds)
      set wid to item objectNumber of candidateIds
      set expectedTty to item objectNumber of candidateTtys
      if exists window id wid then
        set targetWindow to window id wid
        if (count of tabs of targetWindow) is not 1 then return "CLEANUP_REFUSED_OWNERSHIP"
        set targetTab to tab 1 of targetWindow
        if (tty of targetTab) is not expectedTty then return "CLEANUP_REFUSED_TTY"
        if busy of targetTab then set allIdle to false
      end if
    end repeat
    if allIdle then exit repeat
    delay 0.1
  end repeat
  if not allIdle then return "CLEANUP_REFUSED_BUSY"
  repeat with objectNumber from 1 to (count of candidateIds)
    set wid to item objectNumber of candidateIds
    set expectedTty to item objectNumber of candidateTtys
    if exists window id wid then
      set targetWindow to window id wid
      if (count of tabs of targetWindow) is not 1 then return "CLEANUP_REFUSED_OWNERSHIP"
      if (tty of tab 1 of targetWindow) is not expectedTty then return "CLEANUP_REFUSED_TTY"
      close targetWindow saving no
    end if
  end repeat
  return "CLOSE_REQUESTED " & (count of candidateIds)
end tell"##;
        let cleanup_script = cleanup_template
            .replace("__IDS__", &id_list)
            .replace("__TTYS__", &tty_list);
        let cleanup_output = Command::new("osascript")
            .arg("-e")
            .arg(cleanup_script)
            .output()
            .unwrap();
        let cleanup = format!(
            "status={} stdout={} stderr={}",
            cleanup_output.status,
            String::from_utf8_lossy(&cleanup_output.stdout).trim(),
            String::from_utf8_lossy(&cleanup_output.stderr).trim()
        );

        let confirmation_template = r##"set candidateIds to {__IDS__}
set candidateTtys to {__TTYS__}
tell application "Terminal"
  set remainingCount to 0
  repeat with objectNumber from 1 to (count of candidateIds)
    set wid to item objectNumber of candidateIds
    set expectedTty to item objectNumber of candidateTtys
    if exists window id wid then
      set targetWindow to window id wid
      if (count of tabs of targetWindow) is 1 then
        if (tty of tab 1 of targetWindow) is expectedTty then set remainingCount to remainingCount + 1
      end if
    end if
  end repeat
  if remainingCount is not 0 then error "provider test objects remain: " & remainingCount
  return "PROVIDER_TEST_CLEANUP_CONFIRMED"
end tell"##;
        let confirmation_script = confirmation_template
            .replace("__IDS__", &id_list)
            .replace("__TTYS__", &tty_list);
        let mut confirmation_output = None;
        for _ in 0..30 {
            let result = Command::new("osascript")
                .arg("-e")
                .arg(&confirmation_script)
                .output()
                .unwrap();
            let confirmed = result.status.success();
            confirmation_output = Some(result);
            if confirmed {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let confirmation_output = confirmation_output.unwrap();

        assert!(
            output.status.success()
                && stdout.starts_with("TERMINAL_BATCH_OK 2")
                && tab_ids.len() == 2
                && tab_ttys.len() == 2,
            "real provider batch failed: stdout={stdout} stderr={stderr}; {process_verification}; native={native_verification}; cleanup={cleanup}"
        );
        assert!(
            !claude_pids.is_empty()
                && !codex_pids.is_empty()
                && claude_registered
                && codex_registered,
            "provider process verification failed: {process_verification}; native={native_verification}; cleanup={cleanup}"
        );
        assert!(
            native_verification.contains("REAL_PROVIDER_NATIVE_TABS_OK 2"),
            "native provider tab verification failed: {native_verification}; cleanup={cleanup}"
        );
        assert!(
            cleanup_output.status.success()
                && (cleanup.contains("stdout=CLOSE_REQUESTED ")
                    || cleanup.contains("stdout=ALREADY_GONE"))
                && confirmation_output.status.success(),
            "provider test cleanup failed: {cleanup}; confirmation={}",
            String::from_utf8_lossy(&confirmation_output.stderr)
        );
        println!(
            "REAL_PROVIDER_MIXED_SMOKE_OK exact_flags=true native_tabs=2 claude_registered=true codex_registered=true cleanup=true"
        );
    }

    #[test]
    fn stored_cache_round_trip_preserves_incremental_parsing_without_message_bodies() {
        let fixture = TestSession::new("cache-round-trip");
        let secret = "DO_NOT_PERSIST_MESSAGE_BODY";
        fixture.write(
            format!(
                "{{\"type\":\"user\",\"timestamp\":\"2026-11-23T01:00:00Z\",\"cwd\":\"/tmp/project\",\"message\":{{\"content\":\"{secret}\"}}}}\n"
            )
            .as_bytes(),
        );
        let favs = HashSet::new();
        let mut cache = Cache::new();
        assert_eq!(fixture.parse(&mut cache, &favs).message_count, 1);

        let stored = StoredCache {
            version: CACHE_FORMAT_VERSION,
            entries: cache.entries.clone(),
        };
        let json = serde_json::to_vec(&stored).unwrap();
        assert!(!String::from_utf8_lossy(&json).contains(secret));
        let restored = serde_json::from_slice::<StoredCache>(&json).unwrap();
        assert_eq!(restored.version, CACHE_FORMAT_VERSION);
        let mut cache = Cache {
            entries: restored.entries,
            dirty: false,
        };

        fixture.append(claude_message("second").as_bytes());
        assert_eq!(fixture.parse(&mut cache, &favs).message_count, 2);
        assert!(cache.dirty);
    }

    #[test]
    fn persisted_cache_revalidates_every_append_prefix() {
        let fixture = TestSession::new("persisted-cache-append-budget");
        let large_record = serde_json::json!({"type":"progress", "data":"x".repeat(1024 * 1024)})
            .to_string()
            + "\n";
        fixture.write(large_record.as_bytes());
        let favs = HashSet::new();
        let mut cache = Cache::new();
        fixture.parse(&mut cache, &favs);
        let stored = StoredCache {
            version: CACHE_FORMAT_VERSION,
            entries: cache.entries.clone(),
        };
        let json = serde_json::to_vec(&stored).unwrap();
        let restored = serde_json::from_slice::<StoredCache>(&json).unwrap();
        let mut cache = Cache {
            entries: restored.entries,
            dirty: false,
        };
        let key = fixture.path.to_string_lossy().to_string();
        assert!(cache.entries.get(&key).unwrap().growth_probes.is_some());

        fixture.append(claude_message("after-restart-1").as_bytes());
        take_cache_test_bytes_read();
        assert_eq!(fixture.parse(&mut cache, &favs).message_count, 1);
        let first_read = take_cache_test_bytes_read();
        assert!(
            first_read >= large_record.len() as u64,
            "the first post-restart append must fully validate the old prefix"
        );

        let second_previous_len = fixture.path.metadata().unwrap().len();
        fixture.append(claude_message("after-restart-2").as_bytes());
        take_cache_test_bytes_read();
        assert_eq!(fixture.parse(&mut cache, &favs).message_count, 2);
        let second_read = take_cache_test_bytes_read();
        assert!(
            second_read >= second_previous_len,
            "every append must fully validate its old prefix, read={second_read} previous={second_previous_len}"
        );
        let cached_hash = cache.entries.get(&key).unwrap().content_hash;
        let mut file = std::fs::File::open(&fixture.path).unwrap();
        let full_hash =
            read_content_hash(&mut file, fixture.path.metadata().unwrap().len()).unwrap();
        assert_eq!(cached_hash, Some(full_hash));
    }

    #[test]
    fn embedded_codex_sqlite_reads_rows_without_external_cli() {
        let fixture = TestSession::new("embedded-codex-sqlite");
        let db_path = fixture.dir.join("state_5.sqlite");
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT, rollout_path TEXT, created_at INTEGER, updated_at INTEGER,
                    source TEXT, cwd TEXT, title TEXT, first_user_message TEXT,
                    name TEXT, preview TEXT, archived INTEGER NOT NULL DEFAULT 0
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)",
                rusqlite::params![
                    "00000000-0000-4000-8000-0000000000aa",
                    "/mock/rollout.jsonl",
                    10_i64,
                    20_i64,
                    "cli",
                    "/mock/project",
                    "Visible thread",
                    "First question",
                    Option::<String>::None,
                    "Preview",
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, 0, 0, 'cli', '/mock', 'Archived', '', NULL, '', 1)",
                ["00000000-0000-4000-8000-0000000000bb", "/mock/old.jsonl"],
            )
            .unwrap();
        drop(connection);

        let rows = read_codex_thread_rows(&db_path).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["id"], "00000000-0000-4000-8000-0000000000aa");
        assert_eq!(rows[0]["title"], "Visible thread");
        assert_eq!(rows[0]["created_at"], 10);
        assert!(rows[0]["name"].is_null());
    }

    #[cfg(unix)]
    #[test]
    fn codex_database_preserves_path_whitespace() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = TestSession::new("codex-path-whitespace");
        let _codex_home = EnvGuard::set_path("CODEX_HOME", &fixture.dir);
        let project = fixture.dir.join("project ");
        std::fs::create_dir(&project).unwrap();
        let rollout = fixture.dir.join("rollout.jsonl ");
        std::fs::write(&rollout, claude_message("path-preservation")).unwrap();
        let db_path = fixture.dir.join("state_5.sqlite");
        let connection = rusqlite::Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads (
                    id TEXT, rollout_path TEXT, created_at INTEGER, updated_at INTEGER,
                    source TEXT, cwd TEXT, title TEXT, first_user_message TEXT,
                    name TEXT, preview TEXT, archived INTEGER NOT NULL DEFAULT 0
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO threads VALUES (?1, ?2, 10, 20, 'cli', ?3, 'Whitespace', '', NULL, '', 0)",
                rusqlite::params![
                    "00000000-0000-4000-8000-0000000000ac",
                    rollout.to_string_lossy(),
                    project.to_string_lossy(),
                ],
            )
            .unwrap();
        drop(connection);

        let sessions = get_codex_sessions_from_db(&db_path, &HashSet::new()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].project_path, project.to_string_lossy());
        assert_eq!(sessions[0].file_path, rollout.to_string_lossy());
        assert!(sessions[0].project_path.ends_with(' '));
        assert!(sessions[0].file_path.ends_with(' '));
    }

    #[cfg(unix)]
    #[test]
    fn codex_database_metadata_permission_errors_are_not_an_empty_library() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = TestSession::new("codex-db-metadata-permission");
        let private = fixture.dir.join("private");
        std::fs::create_dir(&private).unwrap();
        let db_path = private.join("state_5.sqlite");
        std::fs::write(&db_path, b"not reached by the metadata check").unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = get_codex_sessions_from_db(&db_path, &HashSet::new());
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            result.is_err(),
            "metadata permission errors must not masquerade as an empty Codex library"
        );
    }

    fn claude_message(text: &str) -> String {
        serde_json::json!({
            "type": "user",
            "timestamp": "2026-11-23T01:00:00Z",
            "message": {"content": text}
        })
        .to_string()
            + "\n"
    }

    fn middle_rewrite_pair() -> (String, String) {
        let prefix =
            serde_json::json!({"type":"progress", "data":"x".repeat(70000)}).to_string() + "\n";
        let suffix =
            serde_json::json!({"type":"progress", "data":"z".repeat(80000)}).to_string() + "\n";
        let original = prefix.clone()
            + "{\"type\":\"ai-title\",\"aiTitle\":\"old title\",\"cwd\":\"/old/path\"}\n"
            + &claude_message("first")
            + &suffix;
        let rewritten = prefix
            + "{\"type\":\"ai-title\",\"aiTitle\":\"new title\",\"cwd\":\"/new/path\"}\n"
            + &claude_message("first")
            + &suffix
            + &claude_message("second");
        assert!(original.len() > 150_000 && rewritten.len() > original.len());
        (original, rewritten)
    }

    #[test]
    fn middle_rewrite_then_append_rejects_message_snapshot() {
        let fixture = TestSession::new("middle-rewrite-append-snapshot");
        let (original, rewritten) = middle_rewrite_pair();
        fixture.write(original.as_bytes());
        let first =
            try_read_messages_page(&fixture.path, None, None, 0, 20, Provider::Claude).unwrap();
        fixture.write(rewritten.as_bytes());
        let result = try_read_messages_page(
            &fixture.path,
            Some(first.snapshot_len),
            Some(first.snapshot_token),
            0,
            20,
            Provider::Claude,
        );
        assert!(
            result.is_err(),
            "a middle rewrite outside the edge probes must not be accepted after an append"
        );
    }

    #[test]
    fn reverse_pages_return_latest_chunks_in_display_order() {
        let fixture = TestSession::new("reverse-pages");
        let contents = ["one", "two", "three", "four", "five"]
            .iter()
            .map(|text| claude_message(text))
            .collect::<String>();
        fixture.write(contents.as_bytes());

        let first = read_messages_page(&fixture.path, 0, 2, Provider::Claude);
        assert_eq!(
            first
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            ["four", "five"]
        );
        assert_eq!(first.next_offset, 2);
        assert_eq!(first.offset, first.next_offset);
        assert!(first.has_more);
        assert!(!first.total_exact);
        assert_eq!(first.total, 3);

        let second = read_messages_page(&fixture.path, first.next_offset, 2, Provider::Claude);
        assert_eq!(
            second
                .messages
                .iter()
                .map(|m| m.text.as_str())
                .collect::<Vec<_>>(),
            ["two", "three"]
        );
        assert_eq!(second.next_offset, 4);
        assert!(second.has_more);

        let third = read_messages_page(&fixture.path, second.next_offset, 2, Provider::Claude);
        assert_eq!(third.messages.len(), 1);
        assert_eq!(third.messages[0].text, "one");
        assert_eq!(third.next_offset, 5);
        assert!(!third.has_more);
        assert!(third.total_exact);
        assert_eq!(third.total, 5);

        let beyond = read_messages_page(&fixture.path, 99, 2, Provider::Claude);
        assert!(beyond.messages.is_empty());
        assert_eq!(beyond.next_offset, 5);
        assert_eq!(beyond.total, 5);
        assert!(beyond.total_exact);
        assert!(!beyond.has_more);
    }

    #[test]
    fn reverse_pages_accept_final_line_and_ignore_partial_json() {
        let fixture = TestSession::new("reverse-final-line");
        let mut contents = claude_message("one") + &claude_message("two");
        contents.push_str(&claude_message("three"));
        fixture.write(contents.trim_end().as_bytes());
        let page = read_messages_page(&fixture.path, 0, 3, Provider::Claude);
        assert_eq!(page.messages.len(), 3);
        assert_eq!(page.messages[2].text, "three");
        assert!(page.total_exact);
        assert_eq!(page.total, 3);

        fixture.append(b"\n{\"type\":\"user\"");
        let with_partial = read_messages_page(&fixture.path, 0, 3, Provider::Claude);
        assert_eq!(with_partial.messages.len(), 3);
        assert!(with_partial.total_exact);
    }

    #[test]
    fn reverse_pages_cross_large_non_message_records() {
        let fixture = TestSession::new("reverse-large-record");
        let noise = serde_json::json!({
            "type": "progress",
            "data": "x".repeat(300 * 1024)
        })
        .to_string();
        let contents = claude_message("older") + &noise + "\n" + &claude_message("newest");
        fixture.write(contents.as_bytes());
        assert!(fixture.path.metadata().unwrap().len() > 256 * 1024);

        let first = read_messages_page(&fixture.path, 0, 1, Provider::Claude);
        assert_eq!(first.messages.len(), 1);
        assert_eq!(first.messages[0].text, "newest");
        assert!(first.has_more);
        let second = read_messages_page(&fixture.path, first.next_offset, 1, Provider::Claude);
        assert_eq!(second.messages[0].text, "older");
        assert!(!second.has_more);
        assert!(second.total_exact);
    }

    #[test]
    fn reverse_scan_reassembles_multichunk_single_records() {
        let fixture = TestSession::new("reverse-multichunk-single-record");
        let large = claude_message(&"x".repeat(900 * 1024));
        assert_eq!(large.bytes().filter(|byte| *byte == b'\n').count(), 1);
        let contents = claude_message("older") + &large + &claude_message("newest");
        fixture.write(contents.as_bytes());
        let mut file = std::fs::File::open(&fixture.path).unwrap();
        let mut lines = Vec::new();
        assert!(scan_jsonl_reverse(&mut file, |line| {
            lines.push(line.to_vec());
            true
        })
        .unwrap());
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], claude_message("newest").trim_end().as_bytes());
        assert_eq!(lines[1], large.trim_end().as_bytes());
        assert_eq!(lines[2], claude_message("older").trim_end().as_bytes());
    }

    #[test]
    fn reverse_scan_stops_as_soon_as_page_and_older_marker_are_found() {
        let fixture = TestSession::new("reverse-early-stop");
        let contents = (0..100)
            .map(|index| claude_message(&format!("message-{index}")))
            .collect::<String>();
        fixture.write(contents.as_bytes());
        let mut file = std::fs::File::open(&fixture.path).unwrap();
        let mut visited = 0;
        let complete = scan_jsonl_reverse(&mut file, |_| {
            visited += 1;
            visited < 3
        })
        .unwrap();
        assert!(!complete);
        assert_eq!(visited, 3);
    }

    #[test]
    fn reverse_pages_use_codex_filtering() {
        let fixture = TestSession::new("reverse-codex");
        let records = [
            serde_json::json!({
                "type": "response_item",
                "payload": {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "<environment_context>noise</environment_context>"}
                ]}
            }),
            serde_json::json!({
                "type": "response_item",
                "payload": {"type": "message", "role": "user", "content": [
                    {"type": "input_text", "text": "codex-user"}
                ]}
            }),
            serde_json::json!({
                "type": "response_item",
                "payload": {"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": "codex-assistant"}
                ]}
            }),
        ];
        let contents = records
            .iter()
            .map(|record| record.to_string() + "\n")
            .collect::<String>();
        fixture.write(contents.as_bytes());
        let page = read_messages_page(&fixture.path, 0, 10, Provider::Codex);
        assert_eq!(page.messages.len(), 2);
        assert_eq!(page.messages[0].text, "codex-user");
        assert_eq!(page.messages[1].text, "codex-assistant");
        assert_eq!(page.total, 2);
        assert!(page.total_exact);
    }

    #[test]
    fn missing_codex_database_is_an_empty_provider() {
        let fixture = TestSession::new("missing-codex-database");
        let sessions =
            get_codex_sessions_from_db(&fixture.dir.join("state_5.sqlite"), &HashSet::new())
                .unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn message_pages_require_a_validated_in_root_session_file() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let fixture = TestSession::new("validated-message-page");
        let claude_home = fixture.dir.join("claude-home");
        let projects = claude_home.join("projects").join("project");
        std::fs::create_dir_all(&projects).unwrap();
        let transcript = projects.join(format!("{SESSION_ID}.jsonl"));
        std::fs::write(
            &transcript,
            b"{\"type\":\"user\",\"message\":{\"content\":\"inside root\"}}\n",
        )
        .unwrap();
        fixture.write(b"{\"type\":\"user\",\"message\":{\"content\":\"outside root\"}}\n");
        let _env = EnvGuard::set_path("CLAUDE_CONFIG_DIR", &claude_home);

        let missing_project = fixture.dir.join("deleted-project");
        let page = read_validated_messages_page(
            Provider::Claude,
            SESSION_ID,
            &missing_project.to_string_lossy(),
            &transcript.to_string_lossy(),
            MessageSnapshotCursor::default(),
            0,
            200,
        )
        .unwrap();
        assert_eq!(page.messages.len(), 1);
        assert_eq!(page.messages[0].text, "inside root");
        let launch_error = validate_session_launch(
            Provider::Claude,
            SESSION_ID,
            &missing_project.to_string_lossy(),
            &transcript.to_string_lossy(),
        )
        .unwrap_err();
        assert!(launch_error.contains("项目目录不可用"));

        let outside = read_validated_messages_page(
            Provider::Claude,
            SESSION_ID,
            "",
            &fixture.path.to_string_lossy(),
            MessageSnapshotCursor::default(),
            0,
            200,
        )
        .unwrap_err();
        assert!(outside.contains("不在当前 Claude projects"));
    }

    #[test]
    fn codex_rollout_name_must_end_with_the_exact_session_id() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let fixture = TestSession::new("codex-rollout-name");
        let codex_home = fixture.dir.join("codex-home");
        std::fs::create_dir(&codex_home).unwrap();
        let rollout = codex_home.join(format!("rollout-{SESSION_ID}-suffix.jsonl"));
        std::fs::write(&rollout, b"").unwrap();
        let _env = EnvGuard::set_path("CODEX_HOME", &codex_home);

        let error = read_validated_messages_page(
            Provider::Codex,
            SESSION_ID,
            "",
            &rollout.to_string_lossy(),
            MessageSnapshotCursor::default(),
            0,
            200,
        )
        .unwrap_err();
        assert!(error.contains("session ID"), "unexpected error: {error}");
    }

    #[test]
    fn export_workspace_cleanup_is_flat_and_refuses_nested_entries() {
        let fixture = TestSession::new("flat-export-workspace");
        let flat = fixture.dir.join("flat-workspace");
        std::fs::create_dir(&flat).unwrap();
        std::fs::write(flat.join("page-00000000.md"), "page").unwrap();
        std::fs::write(flat.join("export.md"), "export").unwrap();
        remove_export_workspace(&flat).unwrap();
        assert!(!flat.exists());

        let nested = fixture.dir.join("nested-workspace");
        std::fs::create_dir(&nested).unwrap();
        std::fs::create_dir(nested.join("unexpected")).unwrap();
        let error = remove_export_workspace(&nested).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(nested.join("unexpected").is_dir());
    }

    #[test]
    fn markdown_export_streams_every_message_in_chronological_order() {
        let fixture = TestSession::new("markdown-export");
        let contents = (0..1505)
            .map(|index| {
                serde_json::json!({
                    "type": "user",
                    "timestamp": format!("2026-11-23T01:{:02}:00Z", index % 60),
                    "message": {"content": format!("export-{index:04}")}
                })
                .to_string()
                    + "\n"
            })
            .collect::<String>();
        fixture.write(contents.as_bytes());
        let output_dir = fixture.dir.join("exports");
        let result = export_markdown_from_file(
            Provider::Claude,
            SESSION_ID,
            "Export Test",
            "/tmp/project",
            "now",
            &fixture.path,
            &output_dir,
        )
        .unwrap();
        assert_eq!(result.message_count, 1505);
        assert!(result.cleanup_warning.is_none());
        let markdown = std::fs::read_to_string(&result.file_path).unwrap();
        assert_eq!(markdown.matches("## You").count(), 1505);
        let markers = markdown
            .lines()
            .filter(|line| line.starts_with("export-"))
            .collect::<Vec<_>>();
        assert_eq!(markers.len(), 1505);
        for (index, marker) in markers.iter().enumerate() {
            assert_eq!(*marker, format!("export-{index:04}"));
        }
        let remaining = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(remaining.len(), 1, "temporary export pages must be removed");
        assert!(!remaining[0].starts_with(".csm-export-"));
    }

    #[test]
    fn markdown_export_handles_empty_and_exact_page_files() {
        for (name, count) in [("empty-export", 0), ("exact-page-export", 1000)] {
            let fixture = TestSession::new(name);
            let contents = (0..count)
                .map(|index| {
                    serde_json::json!({
                        "type": "user",
                        "message": {"content": format!("boundary-{index}")}
                    })
                    .to_string()
                        + "\n"
                })
                .collect::<String>();
            fixture.write(contents.as_bytes());
            let output_dir = fixture.dir.join("exports");
            let result = export_markdown_from_file(
                Provider::Claude,
                SESSION_ID,
                "Boundary Export",
                "/tmp/project",
                "now",
                &fixture.path,
                &output_dir,
            )
            .unwrap();
            assert_eq!(result.message_count, count);
            assert!(result.cleanup_warning.is_none());
            let markdown = std::fs::read_to_string(result.file_path).unwrap();
            assert_eq!(markdown.matches("## You").count(), count);
            if count > 0 {
                assert!(markdown.contains("boundary-0"));
                assert!(markdown.contains(&format!("boundary-{}", count - 1)));
            }
        }
    }

    #[test]
    fn markdown_export_sanitizes_and_never_overwrites_existing_files() {
        let fixture = TestSession::new("unique-export");
        fixture.write(b"{\"type\":\"user\",\"message\":{\"content\":\"safe synthetic body\"}}\n");
        let output_dir = fixture.dir.join("exports");
        let first = export_markdown_from_file(
            Provider::Claude,
            SESSION_ID,
            "../<秘密>:会话?",
            "/tmp/project",
            "now",
            &fixture.path,
            &output_dir,
        )
        .unwrap();
        let second = export_markdown_from_file(
            Provider::Claude,
            SESSION_ID,
            "../<秘密>:会话?",
            "/tmp/project",
            "now",
            &fixture.path,
            &output_dir,
        )
        .unwrap();
        assert_ne!(first.file_path, second.file_path);
        for result in [first, second] {
            assert!(result.cleanup_warning.is_none());
            let path = PathBuf::from(result.file_path);
            assert_eq!(path.parent(), Some(output_dir.as_path()));
            let filename = path.file_name().unwrap().to_string_lossy();
            assert!(!filename.chars().any(|c| "/\\:*?\"<>|".contains(c)));
            assert!(path.exists());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
            }
        }
        let remaining = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(remaining.len(), 2);
        assert!(remaining
            .iter()
            .all(|name| !name.starts_with(".csm-export-")));
    }

    #[test]
    fn concurrent_exports_reserve_unique_files_without_overwriting() {
        let fixture = TestSession::new("concurrent-export");
        fixture
            .write(b"{\"type\":\"user\",\"message\":{\"content\":\"race-safe synthetic body\"}}\n");
        let output_dir = fixture.dir.join("exports");
        std::fs::create_dir_all(&output_dir).unwrap();
        let existing = output_dir.join(format!("Race-{}.md", &SESSION_ID[..8]));
        std::fs::write(&existing, "pre-existing sentinel").unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let results = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for _ in 0..4 {
                let barrier = barrier.clone();
                let source = fixture.path.clone();
                let output = output_dir.clone();
                handles.push(scope.spawn(move || {
                    barrier.wait();
                    export_markdown_from_file(
                        Provider::Claude,
                        SESSION_ID,
                        "Race",
                        "/tmp/project",
                        "now",
                        &source,
                        &output,
                    )
                }));
            }
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });
        let paths = results
            .into_iter()
            .map(|result| result.unwrap().file_path)
            .collect::<HashSet<_>>();
        assert_eq!(paths.len(), 4);
        assert!(!paths.contains(&existing.to_string_lossy().into_owned()));
        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            "pre-existing sentinel"
        );
        for path in paths {
            assert!(std::fs::read_to_string(path)
                .unwrap()
                .contains("race-safe synthetic body"));
        }
        let workspaces = std::fs::read_dir(&output_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".csm-export-")
            })
            .count();
        assert_eq!(workspaces, 0);
    }

    #[test]
    fn unfinished_export_destination_is_removed_on_all_exit_paths() {
        let fixture = TestSession::new("export-destination-cleanup");
        let output_dir = fixture.dir.join("exports");
        std::fs::create_dir_all(&output_dir).unwrap();
        let mut destination = ExportDestination::new(&output_dir, "Failure", SESSION_ID).unwrap();
        write!(destination.file_mut(), "partial synthetic output").unwrap();
        let explicit_path = destination.path.clone();
        destination.cleanup().unwrap();
        assert!(!explicit_path.exists());

        let mut destination = ExportDestination::new(&output_dir, "Failure", SESSION_ID).unwrap();
        write!(destination.file_mut(), "fallback synthetic output").unwrap();
        let fallback_path = destination.path.clone();
        drop(destination);
        assert!(!fallback_path.exists());

        let combined = combine_export_failure(
            "operation".into(),
            Some("destination cleanup".into()),
            Some("workspace cleanup".into()),
        );
        assert!(combined.contains("operation"));
        assert!(combined.contains("destination cleanup"));
        assert!(combined.contains("workspace cleanup"));
    }

    #[cfg(unix)]
    #[test]
    fn export_rejects_middle_rewrite_after_scan_and_cleans_workspace() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = TestSession::new("export-middle-rewrite-after-scan");
        let (original, rewritten) = middle_rewrite_pair();
        fixture.write(original.as_bytes());
        let output_dir = fixture.dir.join("exports");
        let source_path = fixture.path.clone();
        *EXPORT_TEST_AFTER_SCAN
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(Box::new(move || {
            std::fs::write(&source_path, rewritten.as_bytes()).unwrap();
        }));

        let result = export_markdown_from_file(
            Provider::Claude,
            SESSION_ID,
            "Changed during export",
            "/tmp/project",
            "now",
            &fixture.path,
            &output_dir,
        );
        assert!(
            result.is_err(),
            "an in-place middle rewrite after the scan must not produce a successful mixed export"
        );
        let remaining = std::fs::read_dir(&output_dir).unwrap().count();
        assert_eq!(
            remaining, 0,
            "failed changed-source export leaves no plaintext artifacts"
        );
    }

    #[test]
    fn failed_export_explicitly_removes_its_workspace() {
        let fixture = TestSession::new("failed-export-cleanup");
        let output_dir = fixture.dir.join("exports");
        // Opening a directory as a File succeeds on Unix, but reading it fails
        // after the export workspace has been created.
        let error = export_markdown_from_file(
            Provider::Claude,
            SESSION_ID,
            "Failure",
            "/tmp/project",
            "now",
            &fixture.dir,
            &output_dir,
        )
        .unwrap_err();
        assert!(
            error.contains("无法打开导出会话") || error.contains("无法固定导出会话快照"),
            "directory input must fail before any export is committed: {error}"
        );
        let remaining = std::fs::read_dir(&output_dir)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(remaining, 0, "failed export leaves no plaintext pages");
    }

    #[test]
    fn fixed_snapshot_rejects_a_source_shorter_than_the_snapshot() {
        let fixture = TestSession::new("truncated-snapshot");
        fixture.write(b"{\"type\":\"user\",\"message\":{\"content\":\"one\"}}\n");
        let snapshot_len = fixture.path.metadata().unwrap().len() + 1;
        let mut file = std::fs::File::open(&fixture.path).unwrap();
        let error = scan_jsonl_reverse_from(&mut file, snapshot_len, |_| true).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn message_page_io_errors_are_not_converted_to_empty_success() {
        let fixture = TestSession::new("message-page-io-errors");
        let missing = fixture.path.with_extension("missing.jsonl");
        let error =
            try_read_messages_page(&missing, None, None, 0, 20, Provider::Claude).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        fixture.write(b"{\"type\":\"user\",\"message\":{\"content\":\"one\"}}\n");
        let snapshot_len = fixture.path.metadata().unwrap().len();
        let page =
            try_read_messages_page(&fixture.path, None, None, 0, 20, Provider::Claude).unwrap();
        assert_eq!(page.snapshot_len, snapshot_len);
        assert_eq!(page.messages.len(), 1);
        assert!(!page.snapshot_token.is_empty());

        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&fixture.path)
            .unwrap();
        file.set_len(snapshot_len - 1).unwrap();
        drop(file);
        let error = try_read_messages_page(
            &fixture.path,
            Some(snapshot_len),
            Some(page.snapshot_token),
            0,
            20,
            Provider::Claude,
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn fixed_snapshot_rejects_same_length_rewrite_between_pages() {
        let fixture = TestSession::new("rewritten-snapshot");
        let records = |version: &str| {
            (0..4)
                .map(|index| {
                    serde_json::json!({
                        "type":"user",
                        "message":{"content":format!("version-{version}-{index}")}
                    })
                    .to_string()
                        + "\n"
                })
                .collect::<String>()
        };
        let original = records("a");
        fixture.write(original.as_bytes());
        let first =
            try_read_messages_page(&fixture.path, None, None, 0, 2, Provider::Claude).unwrap();
        let snapshot_len = first.snapshot_len;
        let modified = fixture.path.metadata().unwrap().modified().unwrap();
        let rewritten = records("b");
        assert_eq!(rewritten.len(), original.len());
        fixture.write(rewritten.as_bytes());
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&fixture.path)
            .unwrap();
        file.set_modified(modified).unwrap();

        let result = try_read_messages_page(
            &fixture.path,
            Some(snapshot_len),
            Some(first.snapshot_token.clone()),
            first.next_offset,
            2,
            Provider::Claude,
        );
        assert!(
            result.is_err(),
            "a same-length in-place rewrite must not mix two transcript versions"
        );
    }

    #[test]
    fn fixed_snapshot_rechecks_probes_when_change_stamp_is_tied() {
        let fixture = TestSession::new("snapshot-tied-change-stamp");
        fixture.write(b"{\"type\":\"user\",\"message\":{\"content\":\"version-a\"}}\n");
        let first =
            try_read_messages_page(&fixture.path, None, None, 0, 1, Provider::Claude).unwrap();
        fixture.write(b"{\"type\":\"user\",\"message\":{\"content\":\"version-b\"}}\n");

        let mut file = std::fs::File::open(&fixture.path).unwrap();
        let mut snapshot = decode_message_snapshot(&first.snapshot_token).unwrap();
        let (_, tied_change) = file_identity_and_change(&file).unwrap();
        snapshot.change_stamp = tied_change;
        let result = validate_message_snapshot(&mut file, &snapshot, first.snapshot_len);
        assert!(
            result.is_err(),
            "tied metadata must not bypass same-length content probes"
        );
    }

    #[test]
    fn snapshot_token_rejects_unbounded_probe_descriptors() {
        let fixture = TestSession::new("snapshot-token-probe-bounds");
        fixture.write(b"{\"type\":\"user\",\"message\":{\"content\":\"one\"}}\n");
        let first =
            try_read_messages_page(&fixture.path, None, None, 0, 1, Provider::Claude).unwrap();
        let mut snapshot = decode_message_snapshot(&first.snapshot_token).unwrap();
        snapshot.probes.head_len = snapshot.len + 1;
        let forged = encode_message_snapshot(&snapshot).unwrap();
        let error = try_read_messages_page(
            &fixture.path,
            Some(first.snapshot_len),
            Some(forged),
            0,
            1,
            Provider::Claude,
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn fixed_snapshot_token_allows_a_normal_append_between_pages() {
        let fixture = TestSession::new("appended-snapshot-token");
        let record = |index: usize| {
            serde_json::json!({
                "type": "user",
                "message": {"content": format!("snapshot-{index}")}
            })
            .to_string()
                + "\n"
        };
        fixture.write((0..6).map(record).collect::<String>().as_bytes());
        let first =
            try_read_messages_page(&fixture.path, None, None, 0, 2, Provider::Claude).unwrap();
        fixture.append((6..8).map(record).collect::<String>().as_bytes());

        let second = try_read_messages_page(
            &fixture.path,
            Some(first.snapshot_len),
            Some(first.snapshot_token.clone()),
            first.next_offset,
            2,
            Provider::Claude,
        )
        .unwrap();
        assert_eq!(second.messages[0].text, "snapshot-2");
        assert_eq!(second.messages[1].text, "snapshot-3");
        assert_eq!(second.snapshot_len, first.snapshot_len);
        assert_eq!(second.snapshot_token, first.snapshot_token);
    }

    #[test]
    fn export_snapshot_is_not_shifted_by_later_appends() {
        let fixture = TestSession::new("export-snapshot");
        let record = |index: usize| {
            serde_json::json!({
                "type": "user",
                "message": {"content": format!("snapshot-{index}")}
            })
            .to_string()
                + "\n"
        };
        fixture.write((0..6).map(record).collect::<String>().as_bytes());
        let snapshot_len = fixture.path.metadata().unwrap().len();
        let mut file = std::fs::File::open(&fixture.path).unwrap();
        let first =
            read_messages_page_snapshot(&mut file, snapshot_len, 0, 2, Provider::Claude).unwrap();
        assert_eq!(first.messages[0].text, "snapshot-4");
        assert_eq!(first.messages[1].text, "snapshot-5");
        fixture.append((6..8).map(record).collect::<String>().as_bytes());
        let second = read_messages_page_snapshot(
            &mut file,
            snapshot_len,
            first.next_offset,
            2,
            Provider::Claude,
        )
        .unwrap();
        assert_eq!(second.messages[0].text, "snapshot-2");
        assert_eq!(second.messages[1].text, "snapshot-3");
        let current = read_messages_page(&fixture.path, 0, 2, Provider::Claude);
        assert_eq!(current.messages[0].text, "snapshot-6");
        assert_eq!(current.messages[1].text, "snapshot-7");
    }

    #[test]
    #[ignore = "opt-in smoke test against a real local rollout"]
    fn real_large_codex_rollout_pagination_smoke() {
        let Ok(path) = std::env::var("CSM_REAL_CODEX_PATH") else {
            return;
        };
        let path = PathBuf::from(path);
        let bytes = path.metadata().unwrap().len();
        let started = std::time::Instant::now();
        let first = try_read_messages_page(&path, None, None, 0, 50, Provider::Codex).unwrap();
        let first_elapsed = started.elapsed();
        assert!(!first.messages.is_empty());
        assert!(!first.snapshot_token.is_empty());
        println!(
            "real rollout bytes={bytes} first_messages={} next_offset={} has_more={} total_exact={} elapsed_ms={}",
            first.messages.len(),
            first.next_offset,
            first.has_more,
            first.total_exact,
            first_elapsed.as_millis()
        );
        if first.has_more {
            let started = std::time::Instant::now();
            let second = try_read_messages_page(
                &path,
                Some(first.snapshot_len),
                Some(first.snapshot_token.clone()),
                first.next_offset,
                50,
                Provider::Codex,
            )
            .unwrap();
            assert!(!second.messages.is_empty());
            assert!(second.next_offset > first.next_offset);
            assert_eq!(second.snapshot_len, first.snapshot_len);
            assert_eq!(second.snapshot_token, first.snapshot_token);
            println!(
                "second_messages={} next_offset={} has_more={} total_exact={} elapsed_ms={} snapshot_revalidated=true",
                second.messages.len(),
                second.next_offset,
                second.has_more,
                second.total_exact,
                started.elapsed().as_millis()
            );
        }
    }

    #[test]
    #[ignore = "opt-in smoke test against real Codex list metadata"]
    fn real_codex_list_metadata_smoke() {
        if std::env::var("CSM_REAL_STORAGE_SMOKE").is_err() {
            return;
        }
        let started = std::time::Instant::now();
        let codex = get_codex_sessions(&HashSet::new()).unwrap();
        assert!(!codex.is_empty());
        let pending = codex
            .iter()
            .filter(|session| !session.message_count_loaded)
            .count();
        println!(
            "real Codex list total={} pending_counts={} elapsed_ms={}",
            codex.len(),
            pending,
            started.elapsed().as_millis()
        );
    }

    #[tokio::test]
    #[ignore = "opt-in smoke test against the smallest real Codex rollout"]
    async fn real_codex_single_count_smoke() {
        if std::env::var("CSM_REAL_STORAGE_SMOKE").is_err() {
            return;
        }
        let codex = get_codex_sessions(&HashSet::new()).unwrap();
        let session = codex
            .iter()
            .min_by_key(|session| {
                PathBuf::from(&session.file_path)
                    .metadata()
                    .map(|metadata| metadata.len())
                    .unwrap_or(u64::MAX)
            })
            .unwrap();
        let bytes = PathBuf::from(&session.file_path).metadata().unwrap().len();
        let started = std::time::Instant::now();
        let count = get_codex_message_count(session.file_path.clone())
            .await
            .unwrap();
        let cached = get_codex_message_count(session.file_path.clone())
            .await
            .unwrap();
        assert_eq!(count, cached);
        println!(
            "real Codex single count bytes={} count={} elapsed_ms={}",
            bytes,
            count,
            started.elapsed().as_millis()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn missing_claude_session_records_do_not_bypass_resume_scan() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Child, Command, Stdio};

        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let fixture = TestSession::new("missing-claude-session-records");
        let missing_sessions = fixture.dir.join("sessions-directory-does-not-exist");
        let script = fixture.dir.join("claude");
        std::fs::write(&script, "#!/bin/sh\nwhile :; do /bin/sleep 1; done\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();
        let child = Command::new(&script)
            .args(["--dangerously-skip-permissions", "-r", SESSION_ID])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut child = OwnedChild(child);
        let expected_tty = "/dev/ttys-csm-regression".to_string();
        let mut ttys = HashMap::new();
        ttys.insert(child.0.id(), expected_tty.clone());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let system = sysinfo::System::new_all();
            let process = system.process(sysinfo::Pid::from_u32(child.0.id()));
            if process.is_some_and(|process| {
                provider_process::process_matches_resume(process, Provider::Claude, SESSION_ID)
            }) {
                assert_eq!(
                    find_running_session_tty_in(&missing_sessions, &system, &ttys, SESSION_ID)
                        .as_deref(),
                    Some(expected_tty.as_str()),
                    "missing session records must not skip the exact live argv fallback"
                );
                assert!(find_running_session_tty_in(
                    &missing_sessions,
                    &system,
                    &ttys,
                    "00000000-0000-4000-8000-000000000002",
                )
                .is_none());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owned provider fixture was not visible through sysinfo"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        child.0.kill().unwrap();
        child.0.wait().unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "opt-in read-only smoke for the current recorded Claude process"]
    fn real_recorded_claude_process_detection_smoke() {
        let session_id = std::env::var("CSM_PROTECT_CLAUDE_ID")
            .expect("CSM_PROTECT_CLAUDE_ID is required for this read-only smoke");
        assert!(is_valid_uuid(&session_id));
        let tty = find_running_session_tty(&session_id).expect(
            "current Claude record must match its live PID, start time and provider identity",
        );
        assert!(tty.starts_with("/dev/tty"));
        println!("REAL_RECORDED_CLAUDE_PROCESS_OK tty=true session_id_redacted=true");
    }

    #[test]
    #[ignore = "opt-in smoke test against real local session storage"]
    fn real_provider_storage_and_launch_command_smoke() {
        if std::env::var("CSM_REAL_STORAGE_SMOKE").is_err() {
            return;
        }
        let favs = HashSet::new();
        let claude = get_claude_sessions(&favs).unwrap();
        let codex = get_codex_sessions(&favs).unwrap();
        assert!(!claude.is_empty());
        assert!(!codex.is_empty());
        assert!(claude.iter().all(|session| {
            session.provider == "claude"
                && is_valid_uuid(&session.session_id)
                && PathBuf::from(&session.file_path).is_file()
        }));
        assert!(codex.iter().all(|session| {
            session.provider == "codex"
                && is_valid_uuid(&session.session_id)
                && PathBuf::from(&session.file_path).is_file()
        }));
        let claude_ids: HashSet<&str> = claude
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        let codex_ids: HashSet<&str> = codex
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        assert_eq!(claude_ids.len(), claude.len());
        assert_eq!(codex_ids.len(), codex.len());
        let codex_main = codex
            .iter()
            .filter(|session| session.session_kind == "main")
            .count();
        let codex_counts_cached = codex
            .iter()
            .filter(|session| session.message_count_loaded)
            .count();
        let cached_user_messages: usize = codex.iter().map(|session| session.message_count).sum();
        println!(
            "real storage claude={} codex={} codex_main={} codex_subagent={} codex_counts_cached={} cached_user_messages={}",
            claude.len(),
            codex.len(),
            codex_main,
            codex.len() - codex_main,
            codex_counts_cached,
            cached_user_messages
        );

        let session = &claude[0];
        let command = session_launch_command_for(
            Provider::Claude,
            &session.session_id,
            &session.project_path,
            &session.file_path,
        )
        .unwrap();
        let cli = provider_cli::resolve(Provider::Claude).unwrap();
        assert!(command.contains(&format!(
            "{} --dangerously-skip-permissions -r ",
            shell_quote(&cli.to_string_lossy())
        )));
        if let Some(path) = provider_cli::launch_path().unwrap() {
            let path = unicode_launch_path(Path::new(&path), "Node PATH").unwrap();
            assert!(command.contains(&format!("PATH={} ", shell_quote(path))));
        }
        assert!(command.ends_with(&session.session_id));

        let session = codex
            .iter()
            .find(|session| session.session_kind == "main")
            .unwrap();
        let command = session_launch_command_for(
            Provider::Codex,
            &session.session_id,
            &session.project_path,
            &session.file_path,
        )
        .unwrap();
        let cli = provider_cli::resolve(Provider::Codex).unwrap();
        assert!(command.contains(&format!(
            "{} --dangerously-bypass-approvals-and-sandbox resume ",
            shell_quote(&cli.to_string_lossy())
        )));
        if let Some(path) = provider_cli::launch_path().unwrap() {
            let path = unicode_launch_path(Path::new(&path), "Node PATH").unwrap();
            assert!(command.contains(&format!("PATH={} ", shell_quote(path))));
        }
        assert!(command.ends_with(&session.session_id));
    }

    #[test]
    #[ignore = "opt-in isolated provider favorite persistence smoke"]
    fn isolated_provider_favorite_persistence_smoke() {
        let Ok(root) = std::env::var("CSM_TEST_CONFIG_DIR") else {
            return;
        };
        let root = std::fs::canonicalize(PathBuf::from(root)).unwrap();
        assert!(root.starts_with(std::fs::canonicalize(std::env::temp_dir()).unwrap()));
        assert!(root
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("csm-favorites-smoke.")));
        std::fs::create_dir_all(&root).unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", &root);
        assert!(!favorites_path().exists());

        assert!(toggle_favorite(SESSION_ID.into(), Some("claude".into())).unwrap());
        assert!(toggle_favorite(SESSION_ID.into(), Some("codex".into())).unwrap());
        let favorites = load_favorites();
        assert!(favorites.contains(SESSION_ID));
        assert!(favorites.contains(&format!("codex:{SESSION_ID}")));
        assert_eq!(favorites.len(), 2);

        assert!(!toggle_favorite(SESSION_ID.into(), Some("claude".into())).unwrap());
        let favorites = load_favorites();
        assert!(!favorites.contains(SESSION_ID));
        assert!(favorites.contains(&format!("codex:{SESSION_ID}")));
        assert!(!toggle_favorite(SESSION_ID.into(), Some("codex".into())).unwrap());
        assert!(load_favorites().is_empty());

        std::env::remove_var("CLAUDE_CONFIG_DIR");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn provider_config_directories_use_overrides_and_the_current_user_home() {
        let home = PathBuf::from("/Users/automatically-discovered-user");
        assert_eq!(
            provider_config_dir(None, Some(home.clone()), ".claude"),
            home.join(".claude")
        );
        assert_eq!(
            provider_config_dir(
                Some(std::ffi::OsString::new()),
                Some(home.clone()),
                ".codex"
            ),
            home.join(".codex")
        );
        let relative = PathBuf::from("relative config/Claude Config");
        let current = std::env::current_dir().unwrap();
        assert_eq!(
            provider_config_dir(
                Some(relative.clone().into_os_string()),
                Some(home.clone()),
                ".claude"
            ),
            current.join(relative)
        );
        #[cfg(not(target_os = "windows"))]
        let custom = PathBuf::from("/Volumes/Custom Location/Claude Config");
        #[cfg(target_os = "windows")]
        let custom = PathBuf::from(r"C:\Volumes\Custom Location\Claude Config");
        assert_eq!(
            provider_config_dir(Some(custom.clone().into_os_string()), Some(home), ".claude"),
            custom
        );
        let windows_home = PathBuf::from(r"C:\Users\Different User");
        assert_eq!(
            provider_config_dir(None, Some(windows_home.clone()), ".codex"),
            windows_home.join(".codex")
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_launch_paths_fail_before_shell_construction() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(std::ffi::OsString::from_vec(
            b"/tmp/csm-non-unicode-\xff".to_vec(),
        ));
        let error = unicode_launch_path(&path, "CLI").unwrap_err();
        assert!(error.contains("Unicode"));
        assert!(error.contains("未执行恢复"));
    }

    #[cfg(unix)]
    #[test]
    fn relative_provider_config_roots_stay_absolute_in_launch_commands() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = TestSession::new("relative-provider-config");
        let current = std::env::current_dir().unwrap();
        let relative_to_current = |path: &Path| {
            let mut relative = PathBuf::new();
            for component in current.components() {
                if matches!(component, std::path::Component::Normal(_)) {
                    relative.push("..");
                }
            }
            relative.push(path.strip_prefix(Path::new("/")).unwrap());
            relative
        };

        let claude_home = fixture.dir.join("Claude Config ' 中文");
        let claude_projects = claude_home.join("projects").join("project");
        std::fs::create_dir_all(&claude_projects).unwrap();
        let transcript = claude_projects.join(format!("{SESSION_ID}.jsonl"));
        std::fs::write(&transcript, b"").unwrap();
        let codex_home = fixture.dir.join("Codex Config ' 中文");
        std::fs::create_dir_all(&codex_home).unwrap();
        let rollout = codex_home.join(format!("rollout-{SESSION_ID}.jsonl"));
        std::fs::write(&rollout, b"").unwrap();

        let relative_claude = relative_to_current(&claude_home);
        let relative_codex = relative_to_current(&codex_home);
        let _claude_home_env = EnvGuard::set_path("CLAUDE_CONFIG_DIR", &relative_claude);
        let _codex_home_env = EnvGuard::set_path("CODEX_HOME", &relative_codex);
        let _claude_cli = EnvGuard::set_path("CSM_CLAUDE_CLI", Path::new("/bin/sh"));
        let _codex_cli = EnvGuard::set_path("CSM_CODEX_CLI", Path::new("/bin/sh"));
        let cwd = fixture.dir.to_string_lossy();

        let claude_command = session_launch_command_for(
            Provider::Claude,
            SESSION_ID,
            &cwd,
            &transcript.to_string_lossy(),
        )
        .unwrap();
        let expected_claude = current.join(&relative_claude);
        let expected_claude_prefix = format!(
            "CLAUDE_CONFIG_DIR={} ",
            shell_quote(&expected_claude.to_string_lossy())
        );
        assert!(claude_command.contains(&expected_claude_prefix));
        assert!(claude_command.contains(" --dangerously-skip-permissions -r "));

        let codex_command = session_launch_command_for(
            Provider::Codex,
            SESSION_ID,
            &cwd,
            &rollout.to_string_lossy(),
        )
        .unwrap();
        let expected_codex = current.join(&relative_codex);
        let expected_codex_prefix = format!(
            "CODEX_HOME={} ",
            shell_quote(&expected_codex.to_string_lossy())
        );
        assert!(codex_command.contains(&expected_codex_prefix));
        assert!(codex_command.contains(" --dangerously-bypass-approvals-and-sandbox resume "));
    }

    #[test]
    fn favorite_persistence_errors_are_returned_to_the_caller() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = TestSession::new("favorite-write-error");
        fixture.write(b"this file prevents using the path as a config directory");
        let _environment = EnvGuard::set_path("CLAUDE_CONFIG_DIR", &fixture.path);
        let error = toggle_favorite(SESSION_ID.into(), Some("claude".into())).unwrap_err();
        assert!(
            error.contains("读取收藏")
                || error.contains("创建收藏目录")
                || error.contains("写入收藏")
        );
        assert!(!favorites_path().exists());
    }

    #[test]
    fn corrupted_favorites_are_not_overwritten_by_a_toggle() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = TestSession::new("favorite-corrupt-read");
        let config = fixture.dir.join("config");
        std::fs::create_dir(&config).unwrap();
        let path = config.join("favorites.json");
        let original = b"{ definitely not valid JSON\n";
        std::fs::write(&path, original).unwrap();
        let _environment = EnvGuard::set_path("CLAUDE_CONFIG_DIR", &config);

        let error = toggle_favorite(SESSION_ID.into(), Some("claude".into())).unwrap_err();
        assert!(error.contains("解析收藏文件"), "unexpected error: {error}");
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(!config.join("favorites.tmp").exists());
    }

    #[test]
    fn concurrent_favorite_toggles_preserve_every_confirmed_key() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let fixture = TestSession::new("favorite-concurrent-writers");
        let config = fixture.dir.join("config");
        std::fs::create_dir(&config).unwrap();
        let _environment = EnvGuard::set_path("CLAUDE_CONFIG_DIR", &config);
        let worker_count = 24usize;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(worker_count));
        let workers: Vec<_> = (0..worker_count)
            .map(|index| {
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let id = format!("00000000-0000-4000-8000-{index:012}");
                    barrier.wait();
                    let result = toggle_favorite(id.clone(), Some("claude".into()));
                    (id, result)
                })
            })
            .collect();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        for (_, result) in &results {
            assert!(matches!(result, Ok(true)), "toggle failed: {result:?}");
        }
        let favorites = read_favorites().unwrap();
        assert_eq!(favorites.len(), worker_count);
        for (id, _) in results {
            assert!(favorites.contains(&id), "lost concurrent favorite {id}");
        }
    }

    #[test]
    fn favorite_keys_do_not_collide_across_providers() {
        assert_eq!(Provider::Claude.favorite_key(SESSION_ID), SESSION_ID);
        assert_eq!(
            Provider::Codex.favorite_key(SESSION_ID),
            format!("codex:{SESSION_ID}")
        );
    }
}
