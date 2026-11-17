//! Reveal only an existing Bridge-owned document. IPC supplies an ID, never a path.
use crate::{agent_folder::DocumentId, agents::Agents};
use std::path::PathBuf;
const ERROR: &str = "Could not show the agent folder.";

fn owned_path(agents: &Agents, id: DocumentId) -> Result<PathBuf, &'static str> {
    let folder = agents.get(id)?;
    if !folder.available() {
        return Err(ERROR);
    }
    folder.file_path().ok_or(ERROR)
}

#[cfg(any(all(test, unix), target_os = "macos"))]
fn reveal_command(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    let mut child = crate::runtime::spawn_guard::run_blocking(|| command.spawn())?;
    child.wait()
}

pub fn reveal(agents: &Agents, id: DocumentId) -> Result<(), &'static str> {
    let path = owned_path(agents, id)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        // Open the folder, never execute the agent file or trust explorer's exit code.
        let parent = path.parent().ok_or(ERROR)?;
        let folder: Vec<_> = parent.as_os_str().encode_wide().chain(Some(0)).collect();
        let _spawn_guard = crate::runtime::spawn_guard::blocking_guard();
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                std::ptr::null(),
                folder.as_ptr(),
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
    if reveal_command(
        std::process::Command::new("/usr/bin/open")
            .arg("-R")
            .arg(path),
    )
    .is_ok_and(|status| status.success())
    {
        return Ok(());
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    let _ = path;
    Err(ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reveals_only_existing_owned_documents() {
        let home = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let agents = Agents::new(Some(home.path().into()), Some(app_data.path().into()));
        assert!(owned_path(&agents, DocumentId::SAMPLE).is_err());
        agents.sample().open().unwrap();
        assert_eq!(
            owned_path(&agents, DocumentId::SAMPLE).unwrap(),
            home.path()
                .join("Orka/agents/web-reader/web-reader.agent.md")
        );
        let unknown = serde_json::from_str("\"00000000-0000-4000-8000-000000000001\"").unwrap();
        assert!(owned_path(&agents, unknown).is_err());
        for input in ["\"/etc/passwd\"", "{\"path\":\"/etc/passwd\"}"] {
            assert!(serde_json::from_str::<DocumentId>(input).is_err());
        }
        std::fs::remove_file(
            home.path()
                .join("Orka/agents/web-reader/web-reader.agent.md"),
        )
        .unwrap();
        assert!(owned_path(&agents, DocumentId::SAMPLE).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn reveal_spawn_waits_for_the_private_descriptor_window() {
        let held = crate::runtime::spawn_guard::tests::hold();
        let (entered, ready) = std::sync::mpsc::channel();
        let (finished, result) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            entered.send(()).unwrap();
            finished
                .send(reveal_command(&mut std::process::Command::new(
                    "/nonexistent/fixture-opener",
                )))
                .unwrap();
        });
        ready.recv().unwrap();
        let during_setup = result.recv_timeout(std::time::Duration::from_millis(100));
        drop(held);
        thread.join().unwrap();
        assert!(matches!(
            during_setup,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(
            result.recv().unwrap().unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
    }
}
