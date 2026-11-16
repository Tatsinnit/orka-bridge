use std::{io::{Read, Write}, time::Duration};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let account = args == ["config", "get", "user", "--host", "github.com"];
    assert!(account || args == ["auth", "token", "--hostname", "github.com"]);
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    assert!(input.is_empty());
    let exe = std::env::current_exe().unwrap();
    let pending = exe.with_extension(if account { "account-pending" } else { "pending" });
    std::fs::write(&pending, std::process::id().to_string()).unwrap();
    std::fs::rename(pending, exe.with_extension(if account { "account-pid" } else { "pid" })).unwrap();
    let name = exe.file_stem().unwrap().to_str().unwrap();
    if account {
        match name {
            "valid" => return print!("fixture-login\r\n"),
            "account-fail" => std::process::exit(1),
            "account-invalid" => return print!("two words\n"),
            "account-hang" => std::thread::sleep(Duration::from_secs(60)),
            "account-switch" => {
                let count = exe.with_extension("count");
                let seen = std::fs::read(&count).map_or(0, |raw| raw.len());
                std::fs::write(&count, vec![b'x'; seen + 1]).unwrap();
                return print!("{}\n", if seen == 0 { "first-login" } else { "second-login" });
            }
            _ => return print!("fixture-login\n"),
        }
    }
    match name {
        "nonzero" => {
            print!("FIXTURE_ONLY_credential");
            eprintln!("FIXTURE_ONLY_stderr");
            std::process::exit(7);
        }
        "oversize" => {
            let _ = std::io::stdout().write_all(&vec![b'x'; 16385]);
            std::thread::sleep(Duration::from_secs(60));
        }
        "hang" => std::thread::sleep(Duration::from_secs(60)),
        "hang-exit" => {
            print!("FIXTURE_ONLY_credential");
            std::io::stdout().flush().unwrap();
            close_stdout();
            std::thread::sleep(Duration::from_secs(60));
        }
        "invalid" => { std::io::stdout().write_all(b"fixture\0token").unwrap(); }
        _ => {
            std::io::stderr().write_all(&vec![b'x'; 1024 * 1024]).unwrap();
            print!(" \tFIXTURE_ONLY_credential\r\n");
        }
    }
}

#[cfg(unix)]
fn close_stdout() {
    unsafe extern "C" { fn close(fd: i32) -> i32; }
    unsafe { close(1); }
}
#[cfg(windows)]
fn close_stdout() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    unsafe { CloseHandle(GetStdHandle((-11i32) as u32)); }
}
