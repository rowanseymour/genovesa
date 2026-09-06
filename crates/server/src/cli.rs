//! The server binary's command line: a port to listen on, a world to host,
//! and what to keep it as.

use std::path::PathBuf;

use protocol::DEFAULT_PORT;

use crate::random_seed;

/// What the command line asked for.
pub struct Args {
    pub port: u16,
    /// The world to host. A seed is a world — what a generator makes of one
    /// is not this binary's business.
    pub seed: u32,
    /// Whether `--seed` was actually said, as opposed to the random one
    /// every run fills in — what lets the binary refuse a seed aimed at a
    /// world that already has one.
    pub seed_chosen: bool,
    /// The file to keep the world in — reopened if it exists, begun if not.
    /// Without it the world lives exactly as long as the process.
    pub world: Option<PathBuf>,
    /// What to call the world on the screen that lists kept worlds — see
    /// `Server::named`. Only for a world being begun in a file: a world that
    /// exists has its name already, and one that lives as long as the process
    /// is never listed anywhere, so a name for it would be an option that
    /// quietly did nothing.
    pub name: Option<String>,
}

/// Built rather than written out so the defaults it quotes are read from the
/// code itself and cannot drift.
fn usage() -> String {
    format!(
        "\
Genovesa server — hosts a shared world for game clients to join.

Usage: server [options]

Options:
  --port <n>     port to listen on [default: {DEFAULT_PORT}]
  --seed <n>     the world to host [default: a new one every run, which the
                 server names as it starts]
  --world <file> keep the world in this file: reopened if it exists — the
                 same islands, the clock and the players where they were —
                 and begun if not [default: the world lasts as long as the
                 process]
  --name <text>  what a world begun with --world is called where kept worlds
                 are listed, up to {letters} letters [default: nameless]

The world is generated here and handed out a chunk at a time. Clients need
know nothing about it — not the seed, not the layout — so nothing they are
sent carries either, and the ground arrives already made. A client that wants
the number can ask for it: the console's `world seed` answers, that being a
debugging question rather than something a client draws with.

A day turns in {day:.0} seconds, and the hour is the server's: every client in
the world is under the same sun, however long the world has been up. Which
hour it is, is the console's `world time` — said from inside the world by
anyone in it, at any point, rather than fixed here before there is a world to
say it to.
",
        day = protocol::DAY_SECONDS,
        letters = protocol::NAME_LETTERS,
    )
}

/// Reads the arguments the binary was started with, printing usage and
/// quitting if that is all that was asked for.
pub fn parse(argv: Vec<String>) -> Result<Args, String> {
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        print!("{}", usage());
        std::process::exit(0);
    }

    // A server told nothing about which world to host hosts a new one, rather
    // than the same one every host that ever forgot to say. The line it prints
    // as it starts is what makes that world askable for again.
    let mut args = Args {
        port: DEFAULT_PORT,
        seed: random_seed(),
        seed_chosen: false,
        world: None,
        name: None,
    };

    let mut rest = argv.iter();
    while let Some(flag) = rest.next() {
        let value = rest
            .next()
            .ok_or_else(|| format!("`{flag}` needs a value"))?;
        match flag.as_str() {
            "--port" => {
                args.port = value
                    .parse()
                    .map_err(|_| format!("`{value}` is not a port"))?;
            }
            "--seed" => {
                args.seed = value
                    .parse()
                    .map_err(|_| format!("`{value}` is not a seed"))?;
                args.seed_chosen = true;
            }
            "--world" => args.world = Some(PathBuf::from(value)),
            // Held to the rules a name typed into the game is held to — the
            // wire's, and the field's cap on letters — so the file never
            // carries what the screen could not show.
            "--name" => {
                args.name = Some(
                    protocol::island_name(value)
                        .filter(|name| name.chars().count() <= protocol::NAME_LETTERS)
                        .ok_or_else(|| format!("`{value}` is not a name"))?,
                );
            }
            other => return Err(format!("unknown option `{other}`\n\n{}", usage())),
        }
    }
    if args.name.is_some() && args.world.is_none() {
        return Err("a name is for a world that is kept: say `--world` too".to_string());
    }
    Ok(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(line: &str) -> Result<Args, String> {
        parse(line.split_whitespace().map(str::to_string).collect())
    }

    #[test]
    fn defaults_host_a_new_world_on_the_default_port() {
        let args = parse_args("").expect("should parse");
        assert_eq!(args.port, DEFAULT_PORT);

        // Told nothing, two runs host different worlds rather than the same
        // one forever.
        let again = parse_args("").expect("should parse");
        assert_ne!(args.seed, again.seed);
    }

    #[test]
    fn takes_a_port_and_a_seed() {
        let args = parse_args("--port 4000 --seed 7").expect("should parse");
        assert_eq!(args.port, 4000);
        assert_eq!(args.seed, 7);
        assert!(args.seed_chosen, "a seed said out loud went unnoticed");
        assert!(
            !parse_args("").expect("should parse").seed_chosen,
            "a seed nobody chose claimed to be chosen"
        );
    }

    #[test]
    fn takes_a_world_file() {
        let args = parse_args("--world islands.world").expect("should parse");
        assert_eq!(args.world, Some(PathBuf::from("islands.world")));
        assert_eq!(parse_args("").expect("should parse").world, None);
    }

    #[test]
    fn takes_a_name_for_a_world_being_kept() {
        let args = parse(vec![
            "--world".into(),
            "islands.world".into(),
            "--name".into(),
            " Windward Reach ".into(),
        ])
        .expect("should parse");
        assert_eq!(args.name.as_deref(), Some("Windward Reach"));
        assert_eq!(
            parse_args("--world islands.world")
                .expect("should parse")
                .name,
            None
        );

        // A name for a world nobody will ever see listed is refused rather
        // than dropped, and so is a name the screen could not show.
        assert!(
            parse_args("--name Reach").is_err(),
            "named a world that is not kept"
        );
        assert!(parse(vec![
            "--world".into(),
            "w".into(),
            "--name".into(),
            "  ".into()
        ])
        .is_err());
        let long = "x".repeat(protocol::NAME_LETTERS + 1);
        assert!(parse_args(&format!("--world w --name {long}")).is_err());
    }

    /// The hour is the console's `world time`, said from inside the world,
    /// and an option that quietly did nothing would be worse than one that is
    /// refused.
    #[test]
    fn the_hour_is_not_asked_for_here() {
        for line in ["--time 6", "--time 6:30"] {
            assert!(
                parse_args(line).is_err(),
                "`{line}` should be refused — it is said down the console now"
            );
        }
    }

    #[test]
    fn rejects_what_it_cannot_make_sense_of() {
        assert!(parse_args("--nonsense 1").is_err());
        assert!(parse_args("--port").is_err(), "a value is required");
        assert!(parse_args("--port somewhere").is_err());
        assert!(parse_args("--port 90000").is_err(), "not a port");
        assert!(parse_args("--seed island").is_err());
    }
}
