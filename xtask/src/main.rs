use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    match xtask::run(std::env::args_os(), &mut std::io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let _ = writeln!(std::io::stderr().lock(), "error: {e}");
            ExitCode::FAILURE
        }
    }
}
