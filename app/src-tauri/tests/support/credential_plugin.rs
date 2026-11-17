// Synthetic child executable compiled by credential tests, never a real login plugin.
use std::{env, io::{self, Read, Write}, time::Duration};
#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetConsoleProcessList(processes: *mut u32, count: u32) -> u32;
    fn GetConsoleWindow() -> *mut std::ffi::c_void;
}
fn main() {
    if let Ok(path) = env::var("RECORD_FILE") {
        std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(b"x").unwrap();
    }
    if let Ok(path) = env::var("PID_FILE") { std::fs::write(path, std::process::id().to_string()).unwrap(); }
    match env::args().nth(1).as_deref() {
        Some("hang") => loop { std::thread::sleep(Duration::from_secs(60)); },
        Some("overflow") => { io::stdout().write_all(&vec![b'x'; 1024 * 1024 + 1]).unwrap(); },
        Some("stderr") => { io::stderr().write_all(&vec![b'x'; 65537]).unwrap(); },
        Some("fail") => { eprintln!("fixture-private-diagnostic"); std::process::exit(1); },
        _ => {
            #[cfg(windows)]
            if env::args().nth(1).as_deref() == Some("no-console") {
                let parent: u32 = env::var("CONSOLE_PARENT_PID").unwrap().parse().unwrap();
                let mut pids = vec![0; 16];
                let mut count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) } as usize;
                if count > pids.len() {
                    pids.resize(count, 0);
                    count = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) } as usize;
                }
                assert!(count <= pids.len(), "console membership changed during fixture observation");
                let state = if !unsafe { GetConsoleWindow() }.is_null() { "fixture-console-window" }
                    else if pids[..count].contains(&parent) { "fixture-console-inherited" }
                    else { "fixture-console-isolated" };
                print!("{}", env::var("FIXTURE_RESPONSE").unwrap().replace("fixture-bearer", state));
                return;
            }
            let mut input = String::new();
            io::stdin().read_to_string(&mut input).unwrap();
            assert!(input.is_empty());
            if let Ok(expected) = env::var("EXPECTED_INFO") {
                assert_eq!(env::var("KUBERNETES_EXEC_INFO").unwrap(), expected);
            }
            if let Ok(name) = env::var("EXPECT_ABSENT") { assert!(env::var_os(name).is_none()); }
            print!("{}", env::var("FIXTURE_RESPONSE").unwrap());
        }
    }
}
