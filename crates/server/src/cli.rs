//! The server binary's command line: a port to listen on and a world to host.

use protocol::DEFAULT_PORT;
use world::archipelago::WorldConfig;

/// What the command line asked for.
pub struct Args {
    pub port: u16,
    pub config: WorldConfig,
}

/// Built rather than written out so the defaults it quotes are read from the
/// code itself and cannot drift.
fn usage() -> String {
    format!(
        "\
Genovesa server — hosts a shared world for game clients to join.

Usage: server [options]

Options:
  --port <n>   port to listen on [default: {DEFAULT_PORT}]
  --seed <n>   the world to host [default: {}]

Terrain never crosses the wire: clients generate the same world from the
seed, and the server only keeps track of who is in it and where.
",
        WorldConfig::default().seed,
    )
}

/// Reads the arguments the binary was started with, printing usage and
/// quitting if that is all that was asked for.
pub fn parse(argv: Vec<String>) -> Result<Args, String> {
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        print!("{}", usage());
        std::process::exit(0);
    }

    let mut args = Args {
        port: DEFAULT_PORT,
        config: WorldConfig::default(),
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
                args.config.seed = value
                    .parse()
                    .map_err(|_| format!("`{value}` is not a seed"))?;
            }
            other => return Err(format!("unknown option `{other}`\n\n{}", usage())),
        }
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
    fn defaults_host_the_default_world_on_the_default_port() {
        let args = parse_args("").expect("should parse");
        assert_eq!(args.port, DEFAULT_PORT);
        assert_eq!(args.config.seed, WorldConfig::default().seed);
    }

    #[test]
    fn takes_a_port_and_a_seed() {
        let args = parse_args("--port 4000 --seed 7").expect("should parse");
        assert_eq!(args.port, 4000);
        assert_eq!(args.config.seed, 7);
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
