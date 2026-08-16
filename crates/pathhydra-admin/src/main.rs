use std::process::ExitCode;

fn main() -> ExitCode {
    match pathhydra_admin::run(std::env::args_os().skip(1)) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("pathhydra-admin: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}
