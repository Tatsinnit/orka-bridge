use super::*;

#[test]
fn stored_output_is_trimmed_without_changing_token_bytes() {
    let token = parse(b" \tFIXTURE_ONLY_credential\r\n".to_vec()).unwrap();
    assert_eq!(token.as_bytes(), b"FIXTURE_ONLY_credential");
}

#[test]
fn invalid_or_oversized_output_is_never_a_credential() {
    for raw in [
        vec![],
        b" \t\r\n".to_vec(),
        b"fixture token".to_vec(),
        b"fixture\ntoken".to_vec(),
        b"fixture\0token".to_vec(),
        vec![0xff],
        "fixture\u{e9}".as_bytes().to_vec(),
        vec![b'x'; MAX_CAPTURE + 1],
    ] {
        assert_eq!(parse(raw).unwrap_err(), CaptureError::InvalidOutput);
    }
}

#[test]
fn exact_raw_output_bound_is_accepted() {
    assert_eq!(
        parse(vec![b'x'; MAX_CAPTURE]).unwrap().as_bytes().len(),
        MAX_CAPTURE
    );
}

#[test]
fn credential_debug_never_contains_bytes() {
    let token = CapturedGithubToken(b"FIXTURE_ONLY_credential".to_vec(), Some("octocat".into()));
    assert_eq!(format!("{token:?}"), "CapturedGithubToken([REDACTED])");
}

#[test]
fn account_output_is_one_github_login_or_unavailable() {
    assert_eq!(parse_account(b"octo-cat1\n").as_deref(), Some("octo-cat1"));
    assert_eq!(parse_account(b"octocat\r\n").as_deref(), Some("octocat"));
    let longest = "a".repeat(39);
    assert_eq!(
        parse_account(longest.as_bytes()).as_deref(),
        Some(longest.as_str())
    );
    for raw in [
        &b""[..],
        b"\n",
        b"-leading\n",
        b"two words\n",
        b"first\nsecond\n",
        b"under_score\n",
        b"\xff\n",
        "ünicode\n".as_bytes(),
        &[b'a'; 40],
        &[b'a'; MAX_ACCOUNT + 1],
    ] {
        assert_eq!(parse_account(raw), None, "{raw:?}");
    }
}

#[test]
fn account_command_has_fixed_argv_and_removes_ambient_token_overrides() {
    let path = Path::new("/fixture only/not installed gh");
    let cmd = account_command(path);
    let std = cmd.as_std();
    assert_eq!(std.get_program(), path);
    assert_eq!(
        std.get_args().collect::<Vec<_>>(),
        ["config", "get", "user", "--host", "github.com"]
    );
    for key in ["GH_TOKEN", "GITHUB_TOKEN"] {
        assert!(std.get_envs().any(|(k, v)| k == key && v.is_none()));
    }
}

use std::{
    io,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::Duration,
};

fn executable(mode: &str) -> (tempfile::TempDir, PathBuf) {
    static FIXTURE: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    let (_, original) = FIXTURE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("fixture.rs");
        let binary = dir
            .path()
            .join(format!("fixture{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(
            &source,
            include_str!("../../tests/support/credential_fixture.rs"),
        )
        .unwrap();
        let spawn = || {
            Command::new("rustc")
                .args(["--edition=2021"])
                .arg(source)
                .arg("-o")
                .arg(&binary)
                .spawn()
        };
        #[cfg(unix)]
        let mut compiler = std::thread::scope(|scope| {
            scope
                .spawn(|| super::super::spawn_guard::run_blocking(spawn))
                .join()
                .unwrap()
        })
        .unwrap();
        #[cfg(not(unix))]
        let mut compiler = spawn().unwrap();
        assert!(compiler.wait().unwrap().success());
        (dir, binary)
    });
    let dir = tempfile::Builder::new()
        .prefix("Bridge credential fixture ")
        .tempdir()
        .unwrap();
    let exe = dir
        .path()
        .join(format!("{mode}{}", std::env::consts::EXE_SUFFIX));
    // No writable executable handle during parallel child spawns (ETXTBSY).
    std::fs::hard_link(original, &exe).unwrap();
    (dir, exe)
}

#[test]
fn command_has_fixed_argv_and_removes_ambient_token_overrides() {
    let path = Path::new("/fixture only/not installed gh");
    let cmd = capture_command(path);
    let std = cmd.as_std();
    assert_eq!(std.get_program(), path);
    assert_eq!(
        std.get_args().collect::<Vec<_>>(),
        ["auth", "token", "--hostname", "github.com"]
    );
    for key in ["GH_TOKEN", "GITHUB_TOKEN"] {
        assert!(std.get_envs().any(|(k, v)| k == key && v.is_none()));
    }
}

#[test]
fn capture_deadline_is_the_approved_five_seconds() {
    assert_eq!(CAPTURE_TIMEOUT, Duration::from_secs(5));
}

#[test]
fn mac_secondary_path_is_used_only_when_first_entry_is_absent() {
    let mut seen = Vec::new();
    let selected = mac_path(|p| {
        seen.push(p.to_owned());
        if seen.len() == 1 {
            Err(io::Error::from(io::ErrorKind::NotFound))
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert_eq!(
        seen,
        [
            PathBuf::from("/opt/homebrew/bin/gh"),
            PathBuf::from("/usr/local/bin/gh")
        ]
    );
    assert_eq!(selected, seen[1]);
}

#[test]
fn mac_existing_or_inaccessible_primary_never_uses_another_account_source() {
    for first in [
        Ok(()),
        Err(io::ErrorKind::PermissionDenied),
        Err(io::ErrorKind::NotADirectory),
    ] {
        let mut seen = 0;
        let result = mac_path(|_| {
            seen += 1;
            assert_eq!(seen, 1, "must not probe the fallback");
            first.map_err(io::Error::from)
        });
        if first.is_ok() {
            assert_eq!(result.unwrap(), Path::new("/opt/homebrew/bin/gh"));
        } else {
            assert_eq!(result.unwrap_err(), CaptureError::Unavailable);
        }
    }
}

#[cfg(unix)]
#[test]
fn broken_primary_symlink_does_not_authorize_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("primary");
    std::os::unix::fs::symlink(dir.path().join("absent"), &link).unwrap();
    let mut probes = 0;
    assert_eq!(
        mac_path(|_| {
            probes += 1;
            std::fs::symlink_metadata(&link).map(|_| ())
        })
        .unwrap(),
        Path::new("/opt/homebrew/bin/gh")
    );
    assert_eq!(probes, 1);
}

#[test]
fn windows_path_is_derived_from_the_os_folder() {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(
        windows_path(folder),
        folder.join("GitHub CLI").join("gh.exe")
    );
}

#[tokio::test]
async fn actual_child_captures_private_stdout_and_discards_large_stderr() {
    let (_dir, exe) = executable("valid");
    let result = run_capture(&exe, CAPTURE_TIMEOUT).await.unwrap();
    assert_eq!(result.as_bytes(), b"FIXTURE_ONLY_credential");
    assert_eq!(result.account(), Some("fixture-login"));
    let token_pid = std::fs::read_to_string(exe.with_extension("pid")).unwrap();
    let account_pid = std::fs::read_to_string(exe.with_extension("account-pid")).unwrap();
    assert_ne!(
        token_pid, account_pid,
        "token and account proofs must name distinct children"
    );
    assert_eq!(format!("{result:?}"), "CapturedGithubToken([REDACTED])");
}

#[tokio::test]
async fn account_read_failure_or_change_around_capture_is_unavailable_without_failing_capture() {
    for (mode, timeout) in [
        ("account-fail", CAPTURE_TIMEOUT),
        ("account-invalid", CAPTURE_TIMEOUT),
        ("account-switch", CAPTURE_TIMEOUT),
        ("account-hang", Duration::from_millis(300)),
    ] {
        let (_dir, exe) = executable(mode);
        let result = run_capture(&exe, timeout).await.unwrap();
        assert_eq!(result.as_bytes(), b"FIXTURE_ONLY_credential", "{mode}");
        assert_eq!(result.account(), None, "{mode}");
    }
    // The switch fixture really answered twice: once before and once after the token read.
    let (dir, exe) = executable("account-switch");
    run_capture(&exe, CAPTURE_TIMEOUT).await.unwrap();
    assert_eq!(std::fs::read(exe.with_extension("count")).unwrap().len(), 2);
    drop(dir);
}

#[tokio::test]
async fn optional_account_lookup_cannot_consume_the_token_capture_budget() {
    let (_dir, exe) = executable("account-hang");
    let result = tokio::time::timeout(Duration::from_secs(2), run_capture(&exe, CAPTURE_TIMEOUT))
        .await
        .expect("optional account lookup held up credential capture")
        .unwrap();
    assert_eq!(result.as_bytes(), b"FIXTURE_ONLY_credential");
    assert_eq!(result.account(), None);
}

#[tokio::test]
async fn actual_child_rejects_nonzero_invalid_and_oversized_output_without_diagnostics() {
    for (mode, expected) in [
        ("nonzero", CaptureError::Failed),
        ("invalid", CaptureError::InvalidOutput),
        ("oversize", CaptureError::InvalidOutput),
    ] {
        let (_dir, exe) = executable(mode);
        let error = run_capture(&exe, CAPTURE_TIMEOUT).await.unwrap_err();
        assert!(
            exe.with_extension("pid").exists(),
            "fixture did not execute: {mode}"
        );
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains("FIXTURE_ONLY"));
    }
}

#[tokio::test]
async fn actual_child_deadline_includes_stdout_and_exit_wait() {
    for mode in ["hang", "hang-exit"] {
        let (_dir, exe) = executable(mode);
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            run_capture(&exe, Duration::from_millis(100)),
        )
        .await
        .unwrap();
        assert_eq!(result.unwrap_err(), CaptureError::TimedOut, "{mode}");
    }
}

#[tokio::test]
async fn missing_executable_does_not_search_path_or_fallback() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        run_capture(&dir.path().join("not-a-command"), CAPTURE_TIMEOUT)
            .await
            .unwrap_err(),
        CaptureError::Unavailable
    );
}

#[cfg(unix)]
async fn wait_for_exit(pid: i32) {
    let exited = tokio::time::timeout(Duration::from_secs(2), async {
        while unsafe { libc::kill(pid, 0) } == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_ok();
    if !exited {
        // Negative tests must not leave the observed, owned sleeping fixture alive.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    assert!(exited, "owned fixture was not reaped");
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_capture_kills_the_observed_owned_direct_child() {
    for (mode, extension) in [("hang", "pid"), ("account-hang", "account-pid")] {
        let (_dir, exe) = executable(mode);
        let pid_path = exe.with_extension(extension);
        let task = tokio::spawn(async move { run_capture(&exe, Duration::from_secs(60)).await });
        let pid: i32 = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&pid_path) {
                    break text.parse().unwrap();
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("fixture did not start");
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        wait_for_exit(pid).await;
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
#[tokio::test]
async fn unsupported_host_does_not_capture_an_ambient_account() {
    assert_eq!(
        capture_stored_github_token().await.unwrap_err(),
        CaptureError::Unavailable
    );
}
