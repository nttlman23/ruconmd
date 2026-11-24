use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    match ruconmd::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Терминала уже может не быть, а eprintln! тогда паникует.
            let _ = writeln!(std::io::stderr(), "ruconmd: {error}");
            ExitCode::FAILURE
        }
    }
}
