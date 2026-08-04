//! The wire between a server and its clients.
//!
//! The world itself never crosses it. A seed is a world — the `world` crate
//! generates the same ocean, bit for bit, on every machine — so a server has
//! nothing to say about terrain beyond the seed, and what remains is the
//! small talk of a session: who is in the world, and where they are. Both
//! sides of the conversation are defined here, engine-free, so the headless
//! server and any client — the Bevy game today, another renderer some day —
//! speak from one definition.
//!
//! Like the world's layout, the wire is a *format*: the bytes each message
//! encodes to are pinned by tests, because a server must understand clients
//! built from other checkouts. Changing any encoding means bumping
//! [`PROTOCOL_VERSION`], which is the first thing a client says and the one
//! thing a server may refuse.

use std::io::{self, Read, Write};

use glam::Vec2;

/// The dialect spoken here. A client leads with it in [`ToServer::Hello`],
/// and a server that speaks a different one answers [`ToClient::Refused`]
/// and hangs up — which is the whole of version negotiation.
pub const PROTOCOL_VERSION: u16 = 1;

/// The port a server listens on, and a client joins on, unless told
/// otherwise. Nothing else claims it, and it is easily remembered as the
/// powers of two run together.
pub const DEFAULT_PORT: u16 = 24816;

/// The longest frame either side will accept. Every message today fits in a
/// couple of dozen bytes; the ceiling exists so that a corrupt length prefix
/// reads as corruption instead of as a request to buffer megabytes.
const MAX_FRAME: u16 = 64;

/// A player, as the server counts them: dealt out in joining order, never
/// reused within a session, meaningless across sessions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayerId(pub u32);

impl std::fmt::Display for PlayerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "player {}", self.0)
    }
}

/// What a client may say.
///
/// Positions are metres on the world's ground plane, as everywhere else in
/// the workspace. Height is never sent: the ground is deterministic, so every
/// machine puts a player down on it locally.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToServer {
    /// The first message on any connection, and never sent again.
    Hello { version: u16 },
    /// Where the player now is.
    Move { position: Vec2 },
}

/// What a server may say.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToClient {
    /// The session, granted: which world this is, who the client is in it,
    /// and where the world is entered.
    Welcome {
        id: PlayerId,
        seed: u32,
        spawn: Vec2,
    },
    /// The version the server speaks, sent instead of a welcome when the
    /// client's is not it. The connection closes after.
    Refused {
        version: u16,
    },
    /// Someone is in the world: sent once for each player already present
    /// when a client joins, and to everyone else when one arrives.
    Joined {
        id: PlayerId,
        position: Vec2,
    },
    /// A player moved. Never echoed back to the player who did.
    Moved {
        id: PlayerId,
        position: Vec2,
    },
    Left {
        id: PlayerId,
    },
}

impl ToServer {
    /// Puts the message on the wire, whole: one frame, one write.
    pub fn write(&self, to: &mut impl Write) -> io::Result<()> {
        let mut payload = Vec::new();
        match self {
            Self::Hello { version } => {
                payload.push(0);
                put_u16(&mut payload, *version);
            }
            Self::Move { position } => {
                payload.push(1);
                put_vec2(&mut payload, *position);
            }
        }
        write_frame(to, &payload)
    }

    /// Reads the next message, blocking until a whole frame has arrived.
    pub fn read(from: &mut impl Read) -> io::Result<Self> {
        let frame = read_frame(from)?;
        let mut payload = Payload::over(&frame);
        let message = match payload.u8()? {
            0 => Self::Hello {
                version: payload.u16()?,
            },
            1 => Self::Move {
                position: payload.vec2()?,
            },
            tag => return Err(corrupt(format!("unknown client message tag {tag}"))),
        };
        payload.finish()?;
        Ok(message)
    }
}

impl ToClient {
    /// Puts the message on the wire, whole: one frame, one write.
    pub fn write(&self, to: &mut impl Write) -> io::Result<()> {
        let mut payload = Vec::new();
        match self {
            Self::Welcome { id, seed, spawn } => {
                payload.push(0);
                put_u32(&mut payload, id.0);
                put_u32(&mut payload, *seed);
                put_vec2(&mut payload, *spawn);
            }
            Self::Refused { version } => {
                payload.push(1);
                put_u16(&mut payload, *version);
            }
            Self::Joined { id, position } => {
                payload.push(2);
                put_u32(&mut payload, id.0);
                put_vec2(&mut payload, *position);
            }
            Self::Moved { id, position } => {
                payload.push(3);
                put_u32(&mut payload, id.0);
                put_vec2(&mut payload, *position);
            }
            Self::Left { id } => {
                payload.push(4);
                put_u32(&mut payload, id.0);
            }
        }
        write_frame(to, &payload)
    }

    /// Reads the next message, blocking until a whole frame has arrived.
    pub fn read(from: &mut impl Read) -> io::Result<Self> {
        let frame = read_frame(from)?;
        let mut payload = Payload::over(&frame);
        let message = match payload.u8()? {
            0 => Self::Welcome {
                id: PlayerId(payload.u32()?),
                seed: payload.u32()?,
                spawn: payload.vec2()?,
            },
            1 => Self::Refused {
                version: payload.u16()?,
            },
            2 => Self::Joined {
                id: PlayerId(payload.u32()?),
                position: payload.vec2()?,
            },
            3 => Self::Moved {
                id: PlayerId(payload.u32()?),
                position: payload.vec2()?,
            },
            4 => Self::Left {
                id: PlayerId(payload.u32()?),
            },
            tag => return Err(corrupt(format!("unknown server message tag {tag}"))),
        };
        payload.finish()?;
        Ok(message)
    }
}

// --- The encoding ------------------------------------------------------------
//
// A frame is a little-endian u16 length followed by that many bytes: a tag
// byte, then the message's fields, little-endian, floats as their IEEE 754
// bits. Nothing self-describing — the tag says what fields follow, and both
// ends are built from this file.

/// Frames a payload and writes it in one call, so a frame reaches the socket
/// whole and small messages travel as single packets.
fn write_frame(to: &mut impl Write, payload: &[u8]) -> io::Result<()> {
    debug_assert!(
        payload.len() <= MAX_FRAME as usize,
        "message over MAX_FRAME"
    );
    let mut frame = Vec::with_capacity(2 + payload.len());
    put_u16(&mut frame, payload.len() as u16);
    frame.extend_from_slice(payload);
    to.write_all(&frame)
}

/// Reads one frame's payload, blocking until all of it has arrived.
fn read_frame(from: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 2];
    from.read_exact(&mut length)?;
    let length = u16::from_le_bytes(length);
    if length > MAX_FRAME {
        return Err(corrupt(format!(
            "a {length}-byte frame can only be garbage"
        )));
    }
    let mut payload = vec![0u8; length as usize];
    from.read_exact(&mut payload)?;
    Ok(payload)
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_vec2(out: &mut Vec<u8>, value: Vec2) {
    out.extend_from_slice(&value.x.to_le_bytes());
    out.extend_from_slice(&value.y.to_le_bytes());
}

fn corrupt(what: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

/// A payload being decoded: hands out fields front to back, and insists at
/// the end that nothing is left — a message with spare bytes is as corrupt as
/// one that runs short.
struct Payload<'a> {
    bytes: &'a [u8],
}

impl<'a> Payload<'a> {
    fn over(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    fn take(&mut self, count: usize) -> io::Result<&'a [u8]> {
        if self.bytes.len() < count {
            return Err(corrupt("message ended mid-field".into()));
        }
        let (taken, rest) = self.bytes.split_at(count);
        self.bytes = rest;
        Ok(taken)
    }

    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }

    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }

    fn vec2(&mut self) -> io::Result<Vec2> {
        Ok(Vec2::new(self.f32()?, self.f32()?))
    }

    fn finish(self) -> io::Result<()> {
        if !self.bytes.is_empty() {
            return Err(corrupt(format!(
                "{} bytes left after the message ended",
                self.bytes.len()
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_of_client(message: ToServer) -> Vec<u8> {
        let mut out = Vec::new();
        message.write(&mut out).expect("a Vec never fails to grow");
        out
    }

    fn bytes_of_server(message: ToClient) -> Vec<u8> {
        let mut out = Vec::new();
        message.write(&mut out).expect("a Vec never fails to grow");
        out
    }

    #[test]
    fn every_message_survives_the_round_trip() {
        let at = Vec2::new(1234.5, -6789.25);
        for message in [
            ToServer::Hello {
                version: PROTOCOL_VERSION,
            },
            ToServer::Move { position: at },
        ] {
            let bytes = bytes_of_client(message);
            assert_eq!(ToServer::read(&mut bytes.as_slice()).unwrap(), message);
        }

        for message in [
            ToClient::Welcome {
                id: PlayerId(3),
                seed: 20_040_112,
                spawn: at,
            },
            ToClient::Refused { version: 9 },
            ToClient::Joined {
                id: PlayerId(1),
                position: at,
            },
            ToClient::Moved {
                id: PlayerId(2),
                position: at,
            },
            ToClient::Left { id: PlayerId(4) },
        ] {
            let bytes = bytes_of_server(message);
            assert_eq!(ToClient::read(&mut bytes.as_slice()).unwrap(), message);
        }
    }

    #[test]
    fn the_wire_is_a_format() {
        // The exact bytes, pinned the way the world pins its digests: a server
        // must understand clients built from other checkouts, so changing any
        // of this means bumping PROTOCOL_VERSION, not re-recording the test.
        assert_eq!(
            bytes_of_client(ToServer::Hello { version: 1 }),
            [3, 0, 0, 1, 0],
            "hello: length 3, tag 0, version LE"
        );
        assert_eq!(
            bytes_of_server(ToClient::Welcome {
                id: PlayerId(7),
                seed: 20_040_112,
                spawn: Vec2::new(1.5, -2.0),
            }),
            [
                17, 0, // length
                0, // tag
                7, 0, 0, 0, // id
                0xB0, 0xC9, 0x31, 0x01, // seed 20 040 112
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
            ],
        );
    }

    #[test]
    fn messages_stream_back_to_back() {
        let mut wire = Vec::new();
        let first = ToServer::Hello { version: 1 };
        let second = ToServer::Move {
            position: Vec2::new(8.0, -4.0),
        };
        first.write(&mut wire).unwrap();
        second.write(&mut wire).unwrap();

        let mut reading = wire.as_slice();
        assert_eq!(ToServer::read(&mut reading).unwrap(), first);
        assert_eq!(ToServer::read(&mut reading).unwrap(), second);
        assert!(reading.is_empty());
    }

    #[test]
    fn garbage_is_refused_not_believed() {
        // A length past the ceiling is corruption, however patient the reader.
        let oversized = [0xFF, 0xFF];
        assert!(ToServer::read(&mut oversized.as_slice()).is_err());

        // An unknown tag, in an otherwise well-formed frame.
        let unknown = [1, 0, 200];
        assert!(ToServer::read(&mut unknown.as_slice()).is_err());
        assert!(ToClient::read(&mut unknown.as_slice()).is_err());

        // A frame that ends mid-field, and one that runs past its meaning.
        let short = [2, 0, 0, 1];
        assert!(ToServer::read(&mut short.as_slice()).is_err());
        let long = [4, 0, 0, 1, 0, 99];
        assert!(ToServer::read(&mut long.as_slice()).is_err());

        // And a wire that simply ends is an ordinary end-of-file error.
        assert!(ToServer::read(&mut [].as_slice()).is_err());
    }
}
