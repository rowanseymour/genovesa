//! The server binary: hosts one world, forever.
//!
//! `server --help` lists what little it takes. There is no window and no
//! rendering — the world is generated here and handed to clients a chunk at
//! a time. Given `--world` the world outlives the process: killed and
//! started again on the same file, it is the same world, aged exactly as
//! much as it was up.

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

    let addr = ("0.0.0.0", args.port);
    let (server, begin_at) = match &args.world {
        // A world file that exists is a world that already has its seed, so
        // a `--seed` aimed at it is a contradiction to refuse rather than a
        // preference to ignore: one of the two named worlds was not going
        // to be the one hosted.
        Some(path) if path.exists() => {
            if args.seed_chosen {
                eprintln!(
                    "server: {} is already a world — its seed is not for choosing",
                    path.display()
                );
                return ExitCode::FAILURE;
            }
            (Server::reopen(addr, path), None)
        }
        Some(path) => (Server::bind(addr, args.config), Some(path.clone())),
        None => (Server::bind(addr, args.config), None),
    };
    let mut server = match server {
        Ok(server) => server,
        Err(error) => {
            eprintln!("server: cannot host on port {}: {error}", args.port);
            return ExitCode::FAILURE;
        }
    };

    // The library keeps quiet about sessions; a server run from a terminal is
    // exactly the caller that wants to see them.
    server = server.reporting_to(|line| println!("{line}"));
    if let Some(phase) = args.opening {
        server = server.opening_at(phase);
    }
    // The clock set before the world is first written, so a `--time` asked
    // for at a kept world's birth is in its file from the very first save —
    // killed thirty seconds in, it must still reopen at the hour it was
    // given.
    if let Some(path) = begin_at {
        server = match server.keeping_at(path) {
            Ok(server) => server,
            Err(error) => {
                eprintln!("server: cannot keep the world: {error}");
                return ExitCode::FAILURE;
            }
        };
    }

    println!(
        "hosting world {} on port {} — join with: game --join <this host>:{}",
        server.seed(),
        args.port,
        args.port
    );
    server.run();
    ExitCode::SUCCESS
}
