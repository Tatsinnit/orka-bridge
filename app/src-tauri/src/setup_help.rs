//! Native-owned help destinations. Renderer input selects a topic, never a URL.
#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HelpTopic {
    Virtualization,
    Hypervisor,
    Workspace,
    GithubCliInstall,
    GithubSignIn,
    OpenaiKey,
    AnthropicKey,
}
impl HelpTopic {
    fn url(self) -> &'static str {
        match self {
            Self::Virtualization => "https://support.microsoft.com/en-us/windows/experience/enable-virtualization-on-windows",
            Self::Hypervisor => "https://learn.microsoft.com/en-us/dotnet/maui/android/emulator/hardware-acceleration?view=net-maui-10.0",
            Self::Workspace => "https://github.com/orka-agents/orka-bridge/blob/main/CONTRIBUTING.md",
            Self::GithubCliInstall => "https://cli.github.com/",
            Self::GithubSignIn => "https://cli.github.com/manual/gh_auth_login",
            Self::OpenaiKey => "https://platform.openai.com/api-keys",
            Self::AnthropicKey => "https://console.anthropic.com/settings/keys",
        }
    }
}
#[cfg(any(all(test, unix), target_os = "macos"))]
fn help_command(command: &mut std::process::Command) -> std::io::Result<std::process::ExitStatus> {
    let mut child = crate::runtime::spawn_guard::run_blocking(|| command.spawn())?;
    child.wait()
}
pub fn open_help(topic: HelpTopic) -> Result<(), &'static str> {
    let url = topic.url();
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        let url: Vec<_> = url.encode_utf16().chain(Some(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                std::ptr::null(),
                url.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            )
        };
        if result as isize > 32 {
            return Ok(());
        }
    }
    #[cfg(target_os = "macos")]
    if help_command(std::process::Command::new("/usr/bin/open").arg(url))
        .is_ok_and(|status| status.success())
    {
        return Ok(());
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = url;
    Err("Could not open setup help.")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn help_spawn_cannot_enter_the_private_descriptor_window() {
        let held = crate::runtime::spawn_guard::tests::hold();
        let (entered, ready) = std::sync::mpsc::channel();
        let (finished, result) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            entered.send(()).unwrap();
            let outcome = help_command(&mut std::process::Command::new(
                "/nonexistent/fixture-opener",
            ));
            finished.send(outcome).unwrap();
        });
        ready.recv().unwrap();
        let during_setup = result.recv_timeout(std::time::Duration::from_millis(100));
        drop(held);
        thread.join().unwrap();
        assert!(
            matches!(
                during_setup,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "help attempted a spawn inside the descriptor window"
        );
        assert_eq!(
            result.recv().unwrap().unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }
    #[test]
    fn help_accepts_only_fixed_topics_and_routes_to_owned_destinations() {
        for (topic, expected) in [
            ("virtualization", "https://support.microsoft.com/en-us/windows/experience/enable-virtualization-on-windows"),
            ("hypervisor", "https://learn.microsoft.com/en-us/dotnet/maui/android/emulator/hardware-acceleration?view=net-maui-10.0"),
            ("workspace", "https://github.com/orka-agents/orka-bridge/blob/main/CONTRIBUTING.md"),
            ("githubCliInstall", "https://cli.github.com/"),
            ("githubSignIn", "https://cli.github.com/manual/gh_auth_login"),
            ("openaiKey", "https://platform.openai.com/api-keys"),
            ("anthropicKey", "https://console.anthropic.com/settings/keys"),
        ] {
            let topic: HelpTopic = serde_json::from_value(serde_json::json!(topic)).unwrap();
            assert_eq!(topic.url(), expected);
        }
        for input in [
            "https://example.com",
            "file:///etc/passwd",
            "workspace;calc.exe",
            "github_cli_install",
            "githubSignIn ",
            "unknown",
            "",
        ] {
            assert!(serde_json::from_value::<HelpTopic>(serde_json::json!(input)).is_err());
        }
    }
}
