//! Windows Terminal and operating-system integration.
//!
//! Pure command-line construction is compiled on every platform so its safety
//! tests also run on macOS. APIs that touch Windows processes or the registry
//! are restricted to `target_os = "windows"`.

#![cfg_attr(not(target_os = "windows"), allow(dead_code))]

#[cfg(target_os = "windows")]
use crate::provider_process::{
    process_matches_codex_rollout_owner, process_matches_resume, recorded_claude_process_matches,
};
use crate::{Provider, ValidatedLaunch};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use std::ffi::OsString;
#[cfg(target_os = "windows")]
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowsLaunchSpec {
    launch: ValidatedLaunch,
    title: String,
    executable: PathBuf,
    config_override: Option<(&'static str, PathBuf)>,
    path_override: Option<OsString>,
}

impl WindowsLaunchSpec {
    #[cfg(target_os = "windows")]
    pub(crate) fn new(launch: ValidatedLaunch, title: &str) -> Result<Self, String> {
        let provider = launch.provider;
        let executable = resolve_cli(provider)?;
        crate::unicode_launch_path(&executable, provider.as_str())?;
        let config_override = crate::configured_provider_dir(provider);
        if let Some((variable, directory)) = &config_override {
            crate::unicode_launch_path(directory, variable)?;
        }
        let path_override = crate::provider_cli::launch_path()?;
        if let Some(path) = &path_override {
            crate::unicode_launch_path(Path::new(path), "Node PATH")?;
        }
        Ok(Self {
            launch,
            title: sanitized_title(title),
            executable,
            config_override,
            path_override,
        })
    }

    #[cfg(test)]
    fn for_test(provider: Provider, session_id: &str, cwd: &str, title: &str, exe: &str) -> Self {
        Self {
            launch: ValidatedLaunch {
                provider,
                session_id: session_id.into(),
                cwd: PathBuf::from(cwd),
                file_path: PathBuf::new(),
            },
            title: sanitized_title(title),
            executable: PathBuf::from(exe),
            config_override: None,
            path_override: None,
        }
    }
}

fn sanitized_title(title: &str) -> String {
    let title = title
        .chars()
        .filter(|c| !c.is_control())
        .map(|c| if c == ';' { '；' } else { c })
        .take(80)
        .collect::<String>();
    if title.trim().is_empty() {
        "Claude Session".into()
    } else {
        title
    }
}

fn powershell_single_quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn encoded_provider_command(spec: &WindowsLaunchSpec) -> String {
    let mut script =
        String::from("$env:CLAUDE_CODE_CHILD_SESSION = $null; $env:CLAUDECODE = $null;");
    if let Some(path) = &spec.path_override {
        script.push_str(" $env:PATH = ");
        script.push_str(&powershell_single_quoted(&path.to_string_lossy()));
        script.push(';');
    }
    if let Some((variable, directory)) = &spec.config_override {
        script.push_str(" $env:");
        script.push_str(variable);
        script.push_str(" = ");
        script.push_str(&powershell_single_quoted(&directory.to_string_lossy()));
        script.push(';');
    }
    if spec
        .executable
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("ps1"))
    {
        script.push_str(
            " Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass -Force -ErrorAction Stop;",
        );
    }
    script.push_str(" Set-Location -LiteralPath ");
    script.push_str(&powershell_single_quoted(
        &spec.launch.cwd.to_string_lossy(),
    ));
    script.push_str(" -ErrorAction Stop; & ");
    script.push_str(&powershell_single_quoted(
        &spec.executable.to_string_lossy(),
    ));
    let arguments: &[&str] = match spec.launch.provider {
        Provider::Claude => &["--dangerously-skip-permissions", "-r"],
        Provider::Codex => &["--dangerously-bypass-approvals-and-sandbox", "resume"],
    };
    for argument in arguments {
        script.push(' ');
        script.push_str(&powershell_single_quoted(argument));
    }
    script.push(' ');
    script.push_str(&powershell_single_quoted(&spec.launch.session_id));

    let utf16 = script
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    BASE64.encode(utf16)
}

/// Build a single new-window invocation. Each `new-tab` is terminated by an
/// argv-only semicolon; no command string is interpreted by `cmd.exe`.
/// Working directories are set inside the encoded PowerShell command instead of
/// `--startingDirectory`, so semicolons and other path characters cannot affect
/// Windows Terminal's subcommand parsing.
///
/// `-w new` is intentional: never target `0`, `last`, a named window or an
/// existing numeric ID, because those can modify a user's existing window.
pub(crate) fn terminal_arguments(launches: &[WindowsLaunchSpec]) -> Result<Vec<String>, String> {
    if launches.is_empty() {
        return Err("没有可启动的 Windows Terminal 会话".into());
    }
    let mut args = vec!["-w".into(), "new".into()];
    for (index, spec) in launches.iter().enumerate() {
        args.extend([
            "new-tab".into(),
            "--title".into(),
            spec.title.clone(),
            "--suppressApplicationTitle".into(),
            "--inheritEnvironment".into(),
            "powershell.exe".into(),
            "-NoLogo".into(),
            "-NoExit".into(),
            "-EncodedCommand".into(),
            encoded_provider_command(spec),
        ]);
        if index + 1 != launches.len() {
            args.push(";".into());
        }
    }
    Ok(args)
}

#[cfg(target_os = "windows")]
fn resolve_cli(provider: Provider) -> Result<PathBuf, String> {
    // Shared discovery searches PATH and per-user installation locations. Keep
    // the returned spelling intact: canonicalize would turn .cmd launchers into
    // \\?\ verbatim paths that cmd.exe cannot execute reliably.
    crate::provider_cli::resolve(provider)
}

#[cfg(target_os = "windows")]
fn recorded_claude_pid(system: &sysinfo::System, session_id: &str) -> Option<sysinfo::Pid> {
    let sessions_dir = crate::claude_dir().join("sessions");
    let mut candidates = std::fs::read_dir(sessions_dir)
        .ok()?
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
            Some((updated, sysinfo::Pid::from_u32(pid), started_at))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    candidates
        .into_iter()
        .map(|(_, pid, started_at)| (pid, started_at))
        .find(|(pid, started_at)| {
            system.process(*pid).is_some_and(|process| {
                recorded_claude_process_matches(process, session_id, *started_at)
            })
        })
        .map(|(pid, _)| pid)
}

#[cfg(any(target_os = "windows", test))]
fn next_restart_manager_capacity(current: usize, needed: u32) -> Option<usize> {
    let needed = needed as usize;
    if needed > current {
        Some(needed)
    } else {
        current.checked_add(1)
    }
}

/// Read-only Restart Manager query for processes holding a rollout file.
/// This never calls RmShutdown or RmRestart and never terminates a process.
#[cfg(target_os = "windows")]
fn processes_using_file(path: &Path) -> Result<Vec<u32>, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
    use windows_sys::Win32::System::RestartManager::{
        RmEndSession, RmGetList, RmRegisterResources, RmStartSession, CCH_RM_SESSION_KEY,
        RM_PROCESS_INFO,
    };

    struct Session(u32);
    impl Drop for Session {
        fn drop(&mut self) {
            unsafe {
                RmEndSession(self.0);
            }
        }
    }

    let wide_path = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let file_pointer = wide_path.as_ptr();
    let mut session_key = vec![0u16; CCH_RM_SESSION_KEY as usize + 1];
    let mut handle = 0u32;
    let status = unsafe { RmStartSession(&mut handle, 0, session_key.as_mut_ptr()) };
    if status != 0 {
        return Err(format!("RmStartSession 失败: {status}"));
    }
    let session = Session(handle);
    let status = unsafe {
        RmRegisterResources(
            session.0,
            1,
            &file_pointer,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
        )
    };
    if status != 0 {
        return Err(format!("RmRegisterResources 失败: {status}"));
    }

    let mut needed = 0u32;
    let mut count = 0u32;
    let mut reboot_reasons = 0u32;
    let status = unsafe {
        RmGetList(
            session.0,
            &mut needed,
            &mut count,
            std::ptr::null_mut(),
            &mut reboot_reasons,
        )
    };
    if status == 0 && count == 0 {
        return Ok(Vec::new());
    }
    if status != ERROR_MORE_DATA {
        return Err(format!("RmGetList(size) 失败: {status}"));
    }

    let mut capacity = next_restart_manager_capacity(0, needed)
        .ok_or_else(|| "RmGetList 返回了无效记录数".to_string())?;
    for _ in 0..8 {
        let mut processes = vec![unsafe { std::mem::zeroed::<RM_PROCESS_INFO>() }; capacity];
        count = capacity as u32;
        let status = unsafe {
            RmGetList(
                session.0,
                &mut needed,
                &mut count,
                processes.as_mut_ptr(),
                &mut reboot_reasons,
            )
        };
        if status == 0 {
            return Ok(processes
                .into_iter()
                .take(count as usize)
                .map(|process| process.Process.dwProcessId)
                .collect());
        }
        if status != ERROR_MORE_DATA {
            return Err(format!("RmGetList(records) 失败: {status}"));
        }
        capacity = next_restart_manager_capacity(capacity, needed)
            .ok_or_else(|| "RmGetList 记录数溢出".to_string())?;
    }
    Err("RmGetList 的进程集合持续变化，8 次重试后仍未获得稳定列表".into())
}

#[cfg(target_os = "windows")]
pub(crate) fn running_session_token(
    provider: Provider,
    session_id: &str,
    file_path: &str,
    source: &str,
) -> Result<Option<String>, String> {
    let system = sysinfo::System::new_all();
    if provider == Provider::Claude {
        if let Some(pid) = recorded_claude_pid(&system, session_id) {
            return Ok(Some(pid.to_string()));
        }
    }

    if let Some(process) = system
        .processes()
        .values()
        .find(|process| process_matches_resume(process, provider, session_id))
    {
        return Ok(Some(process.pid().to_string()));
    }

    if provider == Provider::Codex {
        let pids = processes_using_file(Path::new(file_path))
            .map_err(|error| format!("无法核实 Codex rollout 是否仍被占用：{error}；未重复启动"))?;
        for pid in pids {
            let pid = sysinfo::Pid::from_u32(pid);
            if system
                .process(pid)
                .is_some_and(|process| process_matches_codex_rollout_owner(process, source))
            {
                return Ok(Some(pid.to_string()));
            }
        }
    }
    Ok(None)
}

#[cfg(target_os = "windows")]
pub(crate) fn focus_running_session(_pid: &str) -> Result<bool, String> {
    // Process ownership is enough to prevent a duplicate. Focusing a native tab
    // requires a separately verified Windows UI Automation implementation.
    Ok(false)
}

#[cfg(any(target_os = "windows", test))]
const PROVIDER_STABLE_FOR: std::time::Duration = std::time::Duration::from_millis(1000);

/// A single process snapshot can catch a CLI just before it exits. Require the
/// complete launch set to remain visible for a short interval before reporting
/// success. Any incomplete observation restarts that interval.
#[cfg(any(target_os = "windows", test))]
fn stable_launch_confirmed(
    stable_since: &mut Option<std::time::Instant>,
    running: usize,
    requested: usize,
    now: std::time::Instant,
) -> bool {
    if requested == 0 || running != requested {
        *stable_since = None;
        return false;
    }
    let started = stable_since.get_or_insert(now);
    now.duration_since(*started) >= PROVIDER_STABLE_FOR
}

#[cfg(target_os = "windows")]
fn process_matches_launch(process: &sysinfo::Process, spec: &WindowsLaunchSpec) -> bool {
    process_matches_resume(process, spec.launch.provider, &spec.launch.session_id)
}

#[cfg(target_os = "windows")]
pub(crate) fn open_launches(launches: &[WindowsLaunchSpec]) -> Result<(), String> {
    let args = terminal_arguments(launches)?;
    let system = sysinfo::System::new_all();
    let mut preexisting = std::collections::HashSet::new();
    for spec in launches {
        for process in system
            .processes()
            .values()
            .filter(|process| process_matches_launch(process, spec))
        {
            preexisting.insert((process.pid(), process.start_time()));
        }
    }

    let mut terminal = std::process::Command::new("wt.exe")
        .args(&args)
        .spawn()
        .map_err(|e| {
            format!("无法启动 wt.exe，请确认已安装 Windows Terminal 并启用应用执行别名: {e}")
        })?;
    crate::debug_log(&format!(
        "windows_terminal_launch requested={} window_target=new",
        launches.len()
    ));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
    let mut stable_since = None;
    loop {
        let system = sysinfo::System::new_all();
        let running = launches
            .iter()
            .filter(|spec| {
                system.processes().values().any(|process| {
                    process_matches_launch(process, spec)
                        && !preexisting.contains(&(process.pid(), process.start_time()))
                })
            })
            .count();
        let now = std::time::Instant::now();
        if stable_launch_confirmed(&mut stable_since, running, launches.len(), now) {
            crate::debug_log(&format!(
                "windows_terminal_launch verified_new_processes={running} requested={} stable_ms={}",
                launches.len(),
                PROVIDER_STABLE_FOR.as_millis()
            ));
            return Ok(());
        }
        if let Ok(Some(status)) = terminal.try_wait() {
            if !status.success() {
                return Err(format!(
                    "Windows Terminal 启动器返回失败状态 {status}；已创建内容不会自动清理"
                ));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "Windows Terminal 请求已发送，但只核实到 {running}/{} 个本次新 Provider 进程；已打开内容会保留，未执行自动清理",
                launches.len()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

#[cfg(target_os = "windows")]
pub(crate) fn reveal_in_file_manager(file_path: &str) -> Result<(), String> {
    let path = std::fs::canonicalize(file_path)
        .map_err(|e| format!("无法定位会话文件 {file_path}: {e}"))?;
    std::process::Command::new("explorer.exe")
        .arg(format!("/select,{}", path.to_string_lossy()))
        .spawn()
        .map_err(|e| format!("无法打开 Windows 文件资源管理器: {e}"))?;
    Ok(())
}

#[cfg(target_os = "windows")]
const AUTOSTART_VALUE: &str = "ClaudeSessionManager";

#[cfg(any(target_os = "windows", test))]
fn autostart_command(executable: &std::path::Path) -> String {
    format!("\"{}\"", executable.to_string_lossy())
}

#[cfg(any(target_os = "windows", test))]
fn autostart_registration_matches(command: &str, executable: &std::path::Path) -> bool {
    executable.is_absolute() && executable.is_file() && command == autostart_command(executable)
}

#[cfg(target_os = "windows")]
pub(crate) fn setup_autostart() -> Result<(), String> {
    use winreg::enums::{KEY_READ, KEY_SET_VALUE};

    let executable = std::env::current_exe().map_err(|e| format!("获取可执行路径失败: {e}"))?;
    let command = autostart_command(&executable);
    let key = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_READ | KEY_SET_VALUE,
        )
        .map_err(|e| format!("无法打开当前用户启动项注册表: {e}"))?;
    key.set_value(AUTOSTART_VALUE, &command)
        .map_err(|e| format!("写入登录启动项失败: {e}"))
}

#[cfg(target_os = "windows")]
pub(crate) fn autostart_enabled() -> bool {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let Ok(executable) = std::env::current_exe() else {
        return false;
    };
    winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Run", KEY_READ)
        .and_then(|key| key.get_value::<String, _>(AUTOSTART_VALUE))
        .ok()
        .is_some_and(|command| autostart_registration_matches(&command, &executable))
}

#[cfg(target_os = "windows")]
pub(crate) fn remove_autostart() -> Result<(), String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    let key = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            r"Software\Microsoft\Windows\CurrentVersion\Run",
            KEY_SET_VALUE,
        )
        .map_err(|e| format!("无法打开当前用户启动项注册表: {e}"))?;
    match key.delete_value(AUTOSTART_VALUE) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("移除登录启动项失败: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_process::resumed_command_matches;

    const CLAUDE_ID: &str = "00000000-0000-4000-8000-0000000000c1";
    const CODEX_ID: &str = "00000000-0000-4000-8000-0000000000x1";

    #[test]
    fn native_tab_titles_are_sanitized_without_changing_owned_identity_rules() {
        assert_eq!(
            sanitized_title("CSM-WIN-unique\nClaude;Gamma"),
            "CSM-WIN-uniqueClaude；Gamma"
        );
        assert_eq!(sanitized_title(" \t\r\n "), "Claude Session");
        let long = sanitized_title(&"界".repeat(100));
        assert_eq!(long.chars().count(), 80);
        assert_eq!(long, "界".repeat(80));
    }

    #[test]
    fn windows_autostart_requires_the_current_existing_executable_command() {
        let directory = std::env::temp_dir().join(format!(
            "csm-autostart-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("csm-app.exe");
        std::fs::write(&executable, b"fixture").unwrap();
        let command = autostart_command(&executable);
        assert!(autostart_registration_matches(&command, &executable));
        assert!(!autostart_registration_matches(
            "\"C:\\missing\\old-csm.exe\"",
            &executable
        ));
        assert!(!autostart_registration_matches(
            &command,
            &directory.join("missing.exe")
        ));
        assert!(!autostart_registration_matches(
            "\"csm-app.exe\"",
            std::path::Path::new("csm-app.exe")
        ));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn windows_terminal_always_uses_one_new_window_and_native_tabs() {
        let launches = vec![
            WindowsLaunchSpec::for_test(
                Provider::Claude,
                CLAUDE_ID,
                r"D:\work space\项目",
                "Claude\nAlpha",
                r"C:\Tools\Claude Code\claude.exe",
            ),
            WindowsLaunchSpec::for_test(
                Provider::Codex,
                CODEX_ID,
                r"D:\codex",
                "Codex;Main",
                r"C:\Tools\codex.cmd",
            ),
        ];
        let args = terminal_arguments(&launches).unwrap();
        assert_eq!(&args[..2], &["-w", "new"]);
        assert_eq!(args.iter().filter(|arg| *arg == "new-tab").count(), 2);
        assert_eq!(args.iter().filter(|arg| *arg == ";").count(), 1);
        assert!(!args.iter().any(|arg| arg == "0" || arg == "last"));
        assert!(!args.contains(&r"D:\work space\项目".to_string()));
        assert!(args.contains(&"ClaudeAlpha".to_string()));
        assert!(args.contains(&"Codex；Main".to_string()));
        assert!(!args.contains(&"Codex;Main".to_string()));
        assert_eq!(
            args.iter().filter(|arg| *arg == "-EncodedCommand").count(),
            2
        );
        assert_eq!(
            args.iter()
                .filter(|arg| *arg == "--inheritEnvironment")
                .count(),
            2
        );
    }

    #[test]
    fn encoded_commands_preserve_exact_provider_flags_and_quote_paths() {
        let spec = WindowsLaunchSpec::for_test(
            Provider::Codex,
            CODEX_ID,
            r"D:\project",
            "Codex",
            r"C:\Users\O'Brien\codex.cmd",
        );
        let encoded = encoded_provider_command(&spec);
        let bytes = BASE64.decode(encoded).unwrap();
        assert_eq!(bytes.len() % 2, 0);
        let utf16 = bytes
            .chunks(2)
            .map(|pair| u16::from_le_bytes(pair.try_into().unwrap()))
            .collect::<Vec<_>>();
        let script = String::from_utf16(&utf16).unwrap();
        assert!(script.contains("Set-Location -LiteralPath 'D:\\project' -ErrorAction Stop; & "));
        assert!(script.contains("'C:\\Users\\O''Brien\\codex.cmd'"));
        assert!(script.contains("'--dangerously-bypass-approvals-and-sandbox' 'resume'"));
        assert!(script.ends_with(&format!("'{CODEX_ID}'")));
        assert!(script.contains("$env:CLAUDE_CODE_CHILD_SESSION = $null"));
    }

    #[test]
    fn encoded_commands_pin_config_roots_before_changing_directory() {
        let mut spec = WindowsLaunchSpec::for_test(
            Provider::Codex,
            CODEX_ID,
            r"D:\project",
            "Codex",
            r"C:\Tools\codex.cmd",
        );
        spec.config_override = Some((
            "CODEX_HOME",
            PathBuf::from(r"C:\Config Root\O'Brien\Codex 中文"),
        ));
        spec.path_override = Some(OsString::from(r"C:\Node Runtime\O'Brien;C:\Tools"));
        let encoded = encoded_provider_command(&spec);
        let bytes = BASE64.decode(encoded).unwrap();
        let utf16 = bytes
            .chunks(2)
            .map(|pair| u16::from_le_bytes(pair.try_into().unwrap()))
            .collect::<Vec<_>>();
        let script = String::from_utf16(&utf16).unwrap();
        assert!(script.contains(
            "$env:PATH = 'C:\\Node Runtime\\O''Brien;C:\\Tools'; $env:CODEX_HOME = 'C:\\Config Root\\O''Brien\\Codex 中文'"
        ));
        assert!(script.contains(
            "$env:CODEX_HOME = 'C:\\Config Root\\O''Brien\\Codex 中文'; Set-Location -LiteralPath 'D:\\project'"
        ));
        assert!(!script.contains("Set-ExecutionPolicy"));
    }

    #[test]
    fn ps1_launch_changes_only_the_owned_process_execution_policy() {
        let spec = WindowsLaunchSpec::for_test(
            Provider::Claude,
            CLAUDE_ID,
            r"D:\project",
            "Claude",
            r"C:\Tools\claude.PS1",
        );
        let encoded = encoded_provider_command(&spec);
        let bytes = BASE64.decode(encoded).unwrap();
        let utf16 = bytes
            .chunks(2)
            .map(|pair| u16::from_le_bytes(pair.try_into().unwrap()))
            .collect::<Vec<_>>();
        let script = String::from_utf16(&utf16).unwrap();
        assert!(script.contains(
            "Set-ExecutionPolicy -Scope Process -ExecutionPolicy Bypass -Force -ErrorAction Stop;"
        ));
        assert!(script.contains("& 'C:\\Tools\\claude.PS1' '--dangerously-skip-permissions' '-r'"));
        assert!(!script.contains("LocalMachine"));
        assert!(!script.contains("CurrentUser"));
    }

    #[test]
    fn restart_manager_capacity_always_grows_for_more_data_retries() {
        assert_eq!(next_restart_manager_capacity(0, 4), Some(4));
        assert_eq!(next_restart_manager_capacity(4, 9), Some(9));
        assert_eq!(next_restart_manager_capacity(4, 4), Some(5));
        assert_eq!(next_restart_manager_capacity(usize::MAX, 0), None);
    }

    #[test]
    fn transient_provider_match_does_not_confirm_launch() {
        let start = std::time::Instant::now();
        let mut stable_since = None;
        assert!(!stable_launch_confirmed(&mut stable_since, 1, 1, start));
        // The old implementation returned success at the first observation.
        // This process disappears before the next poll and must never be
        // reported as a successfully restored session.
        assert!(!stable_launch_confirmed(
            &mut stable_since,
            0,
            1,
            start + std::time::Duration::from_millis(500)
        ));
        assert!(stable_since.is_none());
        assert!(!stable_launch_confirmed(
            &mut stable_since,
            1,
            1,
            start + std::time::Duration::from_millis(600)
        ));
        assert!(!stable_launch_confirmed(
            &mut stable_since,
            1,
            1,
            start + std::time::Duration::from_millis(1100)
        ));
        assert!(stable_launch_confirmed(
            &mut stable_since,
            1,
            1,
            start + std::time::Duration::from_millis(1600)
        ));
    }

    #[test]
    fn launch_stability_requires_the_complete_batch_and_a_real_interval() {
        let start = std::time::Instant::now();
        let mut stable_since = None;
        assert!(!stable_launch_confirmed(&mut stable_since, 0, 0, start));
        assert!(!stable_launch_confirmed(&mut stable_since, 1, 2, start));
        assert!(!stable_launch_confirmed(
            &mut stable_since,
            2,
            2,
            start + std::time::Duration::from_millis(100)
        ));
        assert!(!stable_launch_confirmed(
            &mut stable_since,
            2,
            2,
            start + std::time::Duration::from_millis(999)
        ));
        assert!(stable_launch_confirmed(
            &mut stable_since,
            2,
            2,
            start + std::time::Duration::from_millis(1100)
        ));
    }

    #[test]
    fn windows_terminal_rejects_empty_batches() {
        assert!(terminal_arguments(&[]).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn missing_provider_cli_is_an_error_before_terminal_creation() {
        struct RestoreEnvironment(Option<std::ffi::OsString>);
        impl Drop for RestoreEnvironment {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("CSM_CLAUDE_CLI", value),
                    None => std::env::remove_var("CSM_CLAUDE_CLI"),
                }
            }
        }
        let _restore = RestoreEnvironment(std::env::var_os("CSM_CLAUDE_CLI"));
        let missing = std::env::temp_dir().join(format!(
            "csm-missing-cli-{}-{}/claude.exe",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::env::set_var("CSM_CLAUDE_CLI", &missing);
        let error = resolve_cli(Provider::Claude).unwrap_err();
        assert!(error.contains("CSM_CLAUDE_CLI"));
        assert!(error.contains("不存在"));
    }

    #[test]
    fn resumed_process_matching_requires_provider_session_and_exact_flags() {
        let claude = vec![
            "claude".into(),
            "--dangerously-skip-permissions".into(),
            "-r".into(),
            CLAUDE_ID.into(),
        ];
        assert!(resumed_command_matches(
            Provider::Claude,
            CLAUDE_ID,
            &claude
        ));
        assert!(!resumed_command_matches(
            Provider::Claude,
            CODEX_ID,
            &claude
        ));
        assert!(!resumed_command_matches(
            Provider::Codex,
            CLAUDE_ID,
            &claude
        ));
        assert!(!resumed_command_matches(
            Provider::Claude,
            CLAUDE_ID,
            &[format!("prefix-{CLAUDE_ID}")]
        ));
    }
}
