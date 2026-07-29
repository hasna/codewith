use std::ffi::OsStr;
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [command, help] if command == OsStr::new("acp") && help == OsStr::new("--help") => {
            ExitCode::SUCCESS
        }
        [command] if command == OsStr::new("acp") => {
            std::thread::sleep(Duration::from_secs(30));
            ExitCode::SUCCESS
        }
        _ => ExitCode::FAILURE,
    }
}
