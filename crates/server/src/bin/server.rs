//! The server binary: hosts one world until it is asked to stop.
//!
//! `server --help` lists what little it takes: a port, a world, a file to
//! keep it in and a name to keep it under. Nothing about what the world
//! *does* once it is up — the hour
//! included, which is the console's `time`, said from inside the world by
//! anyone in it. There is no window and no rendering — the world is generated
//! here and handed to clients a chunk at a time. Given `--world` the world
//! outlives the process: killed and started again on the same file, it is the
//! same world, aged exactly as much as it was up.
//!
//! Being asked to stop is a signal — Ctrl-C, or whatever an init system sends
//! — and it is the same end of a session a player leaving a world they hosted
//! gets: everybody still in the world is hung up on, and the world is written
//! down before the process goes. See `server::signals`.

use std::process::ExitCode;

use server::{cli, signals, Server};

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
        // A world file that exists is a world that already has its seed and
        // its name, so a `--seed` or `--name` aimed at it is a contradiction
        // to refuse rather than a preference to ignore: one of the two named
        // worlds was not going to be the one hosted.
        Some(path) if path.exists() => {
            if args.seed_chosen {
                eprintln!(
                    "server: {} is already a world — its seed is not for choosing",
                    path.display()
                );
                return ExitCode::FAILURE;
            }
            if args.name.is_some() {
                eprintln!(
                    "server: {} is already a world — its name is not for choosing",
                    path.display()
                );
                return ExitCode::FAILURE;
            }
            (Server::reopen(addr, path), None)
        }
        Some(path) => (
            Server::bind(addr, args.seed)
                .map(|server| server.named(args.name.as_deref().unwrap_or(""))),
            Some(path.clone()),
        ),
        None => (Server::bind(addr, args.seed), None),
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
    if let Some(path) = begin_at {
        server = match server.keeping_at(path) {
            Ok(server) => server,
            Err(error) => {
                eprintln!("server: cannot keep the world: {error}");
                return ExitCode::FAILURE;
            }
        };
    }

    // Before the door is opened rather than after: a world stopped in the
    // moment it started must still be stopped politely, and a signal arriving
    // between the two would otherwise be the default action — which is to say
    // the world's first save would also have been its last.
    signals::catch();

    let host = match server.spawn() {
        Ok(host) => host,
        Err(error) => {
            eprintln!("server: cannot serve: {error}");
            return ExitCode::FAILURE;
        }
    };
    println!(
        "hosting world {} on port {} — join with: game --join <this host>:{}",
        host.seed(),
        args.port,
        args.port
    );

    signals::wait_to_be_asked();

    // The drop is the whole of stopping: it hangs up on everyone still in the
    // world and writes it down, and does not return until both have happened.
    // Said before rather than after, because after is a line nobody waiting on
    // a slow save would see in time for it to mean anything.
    println!("closing the world");
    drop(host);
    ExitCode::SUCCESS
}
