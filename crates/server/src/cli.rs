//! The server binary's command line: a port to listen on and a world to host.

use protocol::DEFAULT_PORT;

use crate::{random_seed, WorldConfig, OPENING};

/// What the command line asked for.
pub struct Args {
    pub port: u16,
    pub config: WorldConfig,
    /// What time of day the world opens at, as a phase of the day — see
    /// `protocol::ToClient::Daylight`.
    pub opening: f32,
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
  --seed <n>   the world to host [default: a new one every run, which the
               server names as it starts]
  --time <h>   the hour the world opens at, from 0 to 24 [default: {opens},
               a morning]

The world is generated here and handed out a chunk at a time. Clients need
know nothing about it — not the seed, not the layout — which is why the seed
is named on this side of the wire and nowhere else.

A day turns in {day:.0} seconds, and the hour is the server's: every client in
the world is under the same sun, however long the world has been up.
",
        opens = OPENING * 24.0,
        day = protocol::DAY_SECONDS,
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
        config: WorldConfig {
            seed: random_seed(),
        },
        opening: OPENING,
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
            "--time" => args.opening = hour(value)?,
            other => return Err(format!("unknown option `{other}`\n\n{}", usage())),
        }
    }
    Ok(args)
}

/// An hour of the world's day, as a phase of it: `0` and `24` are both
/// midnight, `6` dawn, `18` dusk. Hours rather than the fraction the wire
/// carries, because an hour is what somebody deciding when a world should
/// open thinks in — even where the day itself is ten minutes long.
///
/// Public because both command lines take a `--time` and it is the same
/// hour: a game opening a world for itself is opening the world a server
/// would have.
pub fn hour(value: &str) -> Result<f32, String> {
    let hours: f32 = value
        .parse()
        .map_err(|_| format!("`{value}` is not an hour"))?;
    if !(0.0..=24.0).contains(&hours) {
        return Err(format!("`{value}` is not an hour of the day"));
    }
    Ok((hours / 24.0).rem_euclid(1.0))
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
        assert_ne!(args.config.seed, again.config.seed);
    }

    #[test]
    fn takes_a_port_and_a_seed() {
        let args = parse_args("--port 4000 --seed 7").expect("should parse");
        assert_eq!(args.port, 4000);
        assert_eq!(args.config.seed, 7);
    }

    #[test]
    fn a_world_can_be_opened_at_any_hour() {
        // Midnight either end, and noon in the middle: the hour is a phase of
        // the day by the time anything else sees it.
        assert_eq!(parse_args("--time 0").expect("should parse").opening, 0.0);
        assert_eq!(parse_args("--time 12").expect("should parse").opening, 0.5);
        assert_eq!(parse_args("--time 24").expect("should parse").opening, 0.0);
        assert_eq!(parse_args("").expect("should parse").opening, OPENING);

        assert!(parse_args("--time midnight").is_err());
        assert!(parse_args("--time 25").is_err(), "not an hour of any day");
        assert!(parse_args("--time -1").is_err());
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
