//! The server binary: hosts one world, forever.
//!
//! `server --help` lists what little it takes. There is no window and no
//! rendering — clients generate the terrain for themselves from the seed the
//! welcome hands them.

use std::process::ExitCode;

use server::{cli, Server};

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args().skip(1).collect()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("server: {message}");
            return ExitCode::FAILURE;
        }
    };

    // The library keeps quiet about sessions; a server run from a terminal is
    // exactly the caller that wants to see them.
    let server = match Server::bind(("0.0.0.0", args.port), args.config) {
        Ok(server) => server.reporting_to(|line| println!("{line}")),
        Err(error) => {
            eprintln!("server: cannot listen on port {}: {error}", args.port);
            return ExitCode::FAILURE;
        }
    };
    println!(
        "hosting world {} on port {} — join with: game --join <this host>:{}",
        args.config.seed, args.port, args.port
    );
    server.run();
    ExitCode::SUCCESS
}
