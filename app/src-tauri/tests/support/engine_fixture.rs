use std::io::Write;
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["version"] {
        println!("kmx v0.9.0-rc.6 (SECRET_MARKER)\n  model SECRET_MARKER");
        eprintln!("warning SECRET_MARKER");
        return;
    }
    assert!(
        args.len() == 9
            && args[..3] == ["cluster", "orka", "list"]
            && args[3] == "--context"
            && args[5] == "--namespace"
            && args[7..] == ["-o", "json"]
    );
    match args[4].as_str() {
        "exit" => {
            println!("{{}}");
            eprintln!("SECRET_MARKER");
            std::process::exit(7);
        }
        "oversize" => {
            let _ = std::io::stdout().write_all(&vec![b'x'; 1048577]);
        }
        "oversize-stderr" => {
            let _ = std::io::stderr().write_all(&vec![b'x'; 1048577]);
        }
        "block" => {
            let pending = std::path::Path::new(&args[6]).with_extension("pending");
            std::fs::write(&pending, std::process::id().to_string()).unwrap();
            std::fs::rename(pending, &args[6]).unwrap();
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        _ => print!("{}", args.join("\n")),
    }
}
