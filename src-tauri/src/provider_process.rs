//! Structured provider-process identity checks shared by macOS and Windows.
//!
//! A session UUID or the word `claude`/`codex` anywhere in a command line is not
//! sufficient evidence. The provider executable/script role, exact resume flags
//! and session argument must all agree. Recorded Claude PIDs additionally need a
//! matching process start time when the session record provides one.

use crate::Provider;

fn is_resume_flag(provider: Provider, argument: &str) -> bool {
    match provider {
        Provider::Claude => argument == "-r" || argument == "--resume",
        Provider::Codex => argument == "resume",
    }
}

fn resume_flag_position(provider: Provider, command: &[String]) -> Option<usize> {
    command
        .iter()
        .position(|argument| is_resume_flag(provider, argument))
}

fn path_components(argument: &str) -> impl Iterator<Item = &str> {
    argument
        .trim_matches(|character| character == '"' || character == '\'')
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
}

fn executable_name_matches(provider: Provider, value: &str) -> bool {
    let marker = provider.as_str();
    let name = value.to_ascii_lowercase();
    let stem = name
        .rsplit_once('.')
        .map_or(name.as_str(), |(stem, _)| stem);
    stem == marker
        || stem
            .strip_prefix(marker)
            .is_some_and(|suffix| suffix.starts_with('-'))
}

fn argument_identifies_provider(provider: Provider, argument: &str) -> bool {
    let components = path_components(argument).collect::<Vec<_>>();
    let Some(name) = components.last() else {
        return false;
    };
    if executable_name_matches(provider, name) {
        return true;
    }
    // Node-based official installs can expose `node .../cli.js` rather than a
    // provider executable name. Require a provider-named installation component;
    // never accept a provider word in an arbitrary later argument.
    components[..components.len() - 1].iter().any(|component| {
        component.eq_ignore_ascii_case(provider.as_str())
            || (provider == Provider::Claude && component.eq_ignore_ascii_case("claude-code"))
    })
}

fn is_script_launcher(command: &[String]) -> bool {
    command.first().is_some_and(|argument| {
        path_components(argument).last().is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            let stem = name
                .rsplit_once('.')
                .map_or(name.as_str(), |(stem, _)| stem);
            matches!(
                stem,
                "node" | "nodejs" | "python" | "python3" | "bun" | "deno" | "sh" | "bash" | "zsh"
            )
        })
    })
}

pub(crate) fn provider_marker_matches(
    provider: Provider,
    process_name: &str,
    command: &[String],
) -> bool {
    if path_components(process_name)
        .last()
        .is_some_and(|name| executable_name_matches(provider, name))
    {
        return true;
    }
    if command
        .first()
        .is_some_and(|argument| argument_identifies_provider(provider, argument))
    {
        return true;
    }
    if !is_script_launcher(command) {
        return false;
    }
    if command
        .get(1)
        .is_some_and(|argument| argument_identifies_provider(provider, argument))
    {
        return true;
    }
    // The isolated Windows fixture invokes `node provider-fixture.cjs claude`
    // (or `codex`). Restrict this test-only role slot to that exact Node script;
    // an unrelated script with a provider-looking argument is not evidence.
    let fixture_launch = command.first().is_some_and(|argument| {
        path_components(argument).last().is_some_and(|name| {
            let name = name.to_ascii_lowercase();
            let stem = name
                .rsplit_once('.')
                .map_or(name.as_str(), |(stem, _)| stem);
            matches!(stem, "node" | "nodejs")
        })
    }) && command.get(1).is_some_and(|argument| {
        path_components(argument)
            .last()
            .is_some_and(|name| name.eq_ignore_ascii_case("provider-fixture.cjs"))
    });
    fixture_launch
        && command
            .get(2)
            .is_some_and(|argument| executable_name_matches(provider, argument))
}

pub(crate) fn resumed_command_matches(
    provider: Provider,
    session_id: &str,
    command: &[String],
) -> bool {
    let required_flag = match provider {
        Provider::Claude => "--dangerously-skip-permissions",
        Provider::Codex => "--dangerously-bypass-approvals-and-sandbox",
    };
    // Node itself also supports `-r` for `--require`. Do not let an unrelated
    // earlier option hide the actual provider resume flag and session argument.
    command.iter().enumerate().any(|(position, argument)| {
        is_resume_flag(provider, argument)
            && command.get(position + 1).map(String::as_str) == Some(session_id)
            && command[..position]
                .iter()
                .any(|argument| argument == required_flag)
    })
}

pub(crate) fn process_is_provider(process: &sysinfo::Process, provider: Provider) -> bool {
    let command = process
        .cmd()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let executable = process
        .exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| process.name().to_string_lossy().into_owned());
    provider_marker_matches(provider, &executable, &command)
}

#[cfg(any(target_os = "windows", test))]
fn rollout_owner_counts_for_source(source: &str, executable: Option<&str>) -> bool {
    let is_app_server_daemon = executable.is_some_and(|path| {
        path_components(path).any(|component| component.eq_ignore_ascii_case("app-server-daemon"))
    });
    // A CLI rollout can remain open in Codex's service-like daemon after its
    // interactive Terminal exits. Only this exact source and installation path
    // combination is ignored. Unknown sources, VS Code and missing executable
    // paths retain the conservative file-owner protection.
    !(is_app_server_daemon && source.trim().eq_ignore_ascii_case("cli"))
}

#[cfg(target_os = "windows")]
pub(crate) fn process_matches_codex_rollout_owner(
    process: &sysinfo::Process,
    source: &str,
) -> bool {
    if !process_is_provider(process, Provider::Codex) {
        return false;
    }
    let executable = process
        .exe()
        .map(|path| path.to_string_lossy().into_owned());
    rollout_owner_counts_for_source(source, executable.as_deref())
}

pub(crate) fn process_matches_resume(
    process: &sysinfo::Process,
    provider: Provider,
    session_id: &str,
) -> bool {
    if !process_is_provider(process, provider) {
        return false;
    }
    let command = process
        .cmd()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    resumed_command_matches(provider, session_id, &command)
}

pub(crate) fn recorded_start_time_matches(process_start_secs: u64, started_at_ms: i64) -> bool {
    let process_start_ms = i128::from(process_start_secs) * 1000;
    (process_start_ms - i128::from(started_at_ms)).abs() <= 15_000
}

fn recorded_claude_argv_matches(
    session_id: &str,
    has_matching_start_time: bool,
    command: &[String],
) -> bool {
    if command.iter().any(|argument| argument == session_id) {
        return resumed_command_matches(Provider::Claude, session_id, command);
    }
    if !has_matching_start_time {
        return false;
    }
    // Interactive `claude -r` has no UUID in argv, and an originally new
    // conversation has no resume flag at all. A matching recorded PID/start
    // time proves that this process still owns the recorded session in either
    // case; an explicit different UUID is still rejected.
    if resume_flag_position(Provider::Claude, command)
        .and_then(|position| command.get(position + 1))
        .is_some_and(|argument| crate::is_valid_uuid(argument) && argument != session_id)
    {
        return false;
    }
    true
}

pub(crate) fn recorded_claude_process_matches(
    process: &sysinfo::Process,
    session_id: &str,
    started_at_ms: Option<i64>,
) -> bool {
    if !process_is_provider(process, Provider::Claude) {
        return false;
    }
    if started_at_ms
        .is_some_and(|started_at| !recorded_start_time_matches(process.start_time(), started_at))
    {
        return false;
    }
    let command = process
        .cmd()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    recorded_claude_argv_matches(session_id, started_at_ms.is_some(), &command)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_ID: &str = "00000000-0000-4000-8000-0000000000c1";
    const CODEX_ID: &str = "00000000-0000-4000-8000-0000000000a1";

    #[test]
    fn provider_role_rejects_arbitrary_substrings_and_later_arguments() {
        assert!(!provider_marker_matches(
            Provider::Codex,
            "not-codex-indexer",
            &[
                "indexer".into(),
                "--output".into(),
                format!("/tmp/{CODEX_ID}/codex")
            ]
        ));
        assert!(!provider_marker_matches(
            Provider::Claude,
            "node",
            &[
                "node".into(),
                "script.js".into(),
                "--dangerously-skip-permissions".into(),
                "-r".into(),
                CLAUDE_ID.into(),
                "/tmp/claude".into(),
            ]
        ));
        assert!(provider_marker_matches(
            Provider::Claude,
            "node",
            &[
                "node".into(),
                "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js".into(),
                "-r".into(),
                CLAUDE_ID.into(),
            ]
        ));
        assert!(provider_marker_matches(
            Provider::Codex,
            "codex-aarch64-pc-windows-msvc.exe",
            &[]
        ));
    }

    #[test]
    fn resume_arguments_must_be_exact_and_ordered() {
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
            Provider::Claude,
            CLAUDE_ID,
            &[
                "claude".into(),
                "-r".into(),
                "--dangerously-skip-permissions".into(),
                CLAUDE_ID.into(),
            ]
        ));
        assert!(!resumed_command_matches(
            Provider::Codex,
            CODEX_ID,
            &[
                "codex".into(),
                "--dangerously-bypass-approvals-and-sandbox".into(),
                "resume".into(),
                format!("prefix-{CODEX_ID}"),
            ]
        ));
    }

    #[test]
    fn cli_rollout_ignores_only_an_identified_app_server_daemon() {
        let daemon = r"C:\Users\test\.codex\packages\app-server-daemon\releases\0.172.0-aarch64-pc-windows-msvc\bin\codex.exe";
        assert!(!rollout_owner_counts_for_source("cli", Some(daemon)));
        assert!(!rollout_owner_counts_for_source(" CLI ", Some(daemon)));
        assert!(rollout_owner_counts_for_source("vscode", Some(daemon)));
        assert!(rollout_owner_counts_for_source("", Some(daemon)));
        assert!(rollout_owner_counts_for_source("main", Some(daemon)));
        assert!(rollout_owner_counts_for_source("cli", None));
        assert!(rollout_owner_counts_for_source(
            "cli",
            Some(r"C:\Tools\Codex\codex.exe")
        ));
        assert!(rollout_owner_counts_for_source(
            "cli",
            Some(r"C:\Tools\app-server-daemon-copy\codex.exe")
        ));
    }

    #[test]
    fn recorded_pid_requires_the_same_process_start_instant() {
        assert!(recorded_start_time_matches(
            1_795_480_000,
            1_795_480_000_000
        ));
        assert!(recorded_start_time_matches(
            1_795_480_000,
            1_795_479_985_001
        ));
        assert!(!recorded_start_time_matches(
            1_795_480_000,
            1_795_479_984_999
        ));
        assert!(!recorded_start_time_matches(
            1_795_480_000,
            1_795_480_016_000
        ));
    }

    #[test]
    fn interactive_recorded_resume_requires_start_time_but_not_a_uuid_argument() {
        let interactive = vec![
            "claude".into(),
            "--dangerously-skip-permissions".into(),
            "-r".into(),
        ];
        assert!(recorded_claude_argv_matches(CLAUDE_ID, true, &interactive));
        assert!(!recorded_claude_argv_matches(
            CLAUDE_ID,
            false,
            &interactive
        ));
        let different = vec![
            "claude".into(),
            "--dangerously-skip-permissions".into(),
            "-r".into(),
            CODEX_ID.into(),
        ];
        assert!(!recorded_claude_argv_matches(CLAUDE_ID, true, &different));
    }

    #[test]
    fn fixture_role_slot_requires_the_exact_node_fixture_script() {
        let unrelated = vec![
            "node".into(),
            "/tmp/unrelated.js".into(),
            "claude".into(),
            "--dangerously-skip-permissions".into(),
            "-r".into(),
            CLAUDE_ID.into(),
        ];
        assert!(!provider_marker_matches(
            Provider::Claude,
            "node",
            &unrelated
        ));
        let wrong_launcher = vec![
            "bash".into(),
            "/tmp/provider-fixture.cjs".into(),
            "claude".into(),
        ];
        assert!(!provider_marker_matches(
            Provider::Claude,
            "bash",
            &wrong_launcher
        ));
        let fixture = vec![
            "node".into(),
            "/tmp/provider-fixture.cjs".into(),
            "claude".into(),
            "--dangerously-skip-permissions".into(),
            "-r".into(),
            CLAUDE_ID.into(),
        ];
        assert!(provider_marker_matches(Provider::Claude, "node", &fixture));
        let windows_fixture = vec![
            r"C:\Program Files\nodejs\node.EXE".into(),
            r"C:\Temp\provider-fixture.cjs".into(),
            "codex".into(),
            "--dangerously-bypass-approvals-and-sandbox".into(),
            "resume".into(),
            CODEX_ID.into(),
        ];
        assert!(provider_marker_matches(
            Provider::Codex,
            r"C:\Program Files\nodejs\node.exe",
            &windows_fixture
        ));
    }

    #[test]
    fn recorded_original_conversation_with_matching_start_is_still_running() {
        let new_conversation = vec!["claude".into(), "--dangerously-skip-permissions".into()];
        assert!(recorded_claude_argv_matches(
            CLAUDE_ID,
            true,
            &new_conversation
        ));
        assert!(!recorded_claude_argv_matches(
            CLAUDE_ID,
            false,
            &new_conversation
        ));
    }

    #[test]
    fn node_require_option_does_not_hide_the_actual_resume_arguments() {
        let command = vec![
            "node".into(),
            "-r".into(),
            "/tmp/register.js".into(),
            "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js".into(),
            "--dangerously-skip-permissions".into(),
            "-r".into(),
            CLAUDE_ID.into(),
        ];
        assert!(resumed_command_matches(
            Provider::Claude,
            CLAUDE_ID,
            &command
        ));
        assert!(!resumed_command_matches(
            Provider::Claude,
            CODEX_ID,
            &command
        ));
    }

    #[cfg(unix)]
    #[test]
    fn live_sysinfo_process_requires_the_provider_script_and_exact_resume_argv() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::{Child, Command, Stdio};

        struct OwnedChild(Child);
        impl Drop for OwnedChild {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let root = std::env::temp_dir().join(format!(
            "csm-process-fixture-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let script = root.join("claude");
        std::fs::write(&script, "#!/bin/sh\nwhile :; do /bin/sleep 1; done\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&script, permissions).unwrap();
        let child = Command::new(&script)
            .args(["--dangerously-skip-permissions", "-r", CLAUDE_ID])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut child = OwnedChild(child);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let system = sysinfo::System::new_all();
            if let Some(process) = system.process(sysinfo::Pid::from_u32(child.0.id())) {
                if process_matches_resume(process, Provider::Claude, CLAUDE_ID) {
                    assert!(recorded_claude_process_matches(
                        process,
                        CLAUDE_ID,
                        Some((process.start_time() * 1000) as i64)
                    ));
                    assert!(!recorded_claude_process_matches(
                        process,
                        CLAUDE_ID,
                        Some(((process.start_time() + 60) * 1000) as i64)
                    ));
                    break;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owned provider fixture was not visible through sysinfo"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        std::fs::remove_file(&script).unwrap();
        std::fs::remove_dir(&root).unwrap();
    }
}
