use std::process::ExitCode;

fn main() -> ExitCode {
    match ruconmd::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ruconmd: {error}");
            ExitCode::FAILURE
        }
    }
}
