//! The wire between a server and its clients.
//!
//! The world crosses it. A server generates the ocean and hands it out a
//! chunk at a time; a client asks for chunks by coordinate and draws what
//! comes back, and it is told nothing else — not the seed, not the layout,
//! not which chunks are worth asking for. An answer is either open water,
//! which carries no data because the sea and its floor are two flat planes
//! anyone can draw, or ground, which arrives as a [`ground::ChunkPayload`]:
//! corner heights on a fixed grid, one palette entry per triangle, and — on
//! the minority of chunks that hold a lake — the level its water stands at.
//!
//! That is a deliberate inversion of how this started. A seed used to be a
//! world — every machine regenerated the same ocean, bit for bit, and terrain
//! never travelled — which made the client the second half of the generator
//! and made porting it to another language a promise to reproduce every noise
//! octave and every rounding. Sending the ground instead costs bandwidth and
//! buys a client that can be written by anyone who can read this file.
//!
//! Determinism did not stop mattering, it moved: a seed must still mean the
//! same world wherever it is *hosted*, or re-hosting one would land everybody
//! somewhere else. That promise now lives entirely in the `world` crate and
//! its digests, on one machine at a time.
//!
//! Like the world's layout, the wire is a *format*: the bytes each message
//! encodes to are pinned by tests, because a server must understand clients
//! built from other checkouts. Changing any encoding means bumping
//! [`PROTOCOL_VERSION`], which is the first thing a client says and the one
//! thing a server may refuse.

pub mod ground;

use std::io::{self, Read, Write};

use glam::{IVec2, Vec2};

pub use ground::{ChunkPayload, Shade, Surface, Tone};

/// The dialect spoken here. A client leads with it in [`ToServer::Hello`],
/// and a server that speaks a different one answers [`ToClient::Refused`]
/// and hangs up — which is the whole of version negotiation.
pub const PROTOCOL_VERSION: u16 = 7;

/// How long one turn of the world's day takes, in seconds — sunrise to
/// sunrise, ten minutes of it.
///
/// Part of the wire rather than each client's own idea, because a client
/// runs the clock itself between the server's tellings — see
/// [`ToClient::Daylight`] — and one running it at its own pace would drift
/// from the sky everyone else is under.
pub const DAY_SECONDS: f32 = 600.0;

/// The phase the last of the light has gone by, and [`DAYBREAK`] the phase
/// the first of it returns at — between them, the night.
///
/// Both ends need to agree on where the night is and neither can be talked
/// out of it: a server runs the clock fast through a night its players are
/// waiting out (see [`ToServer::WantDawn`]) and stops at [`DAYBREAK`], and a
/// client only offers to wait when there is a night to wait. Set a little
/// inside the dark on either side, so the night is the part with nothing to
/// look at rather than the whole of dusk and dawn.
pub const NIGHTFALL: f32 = 0.79;
/// Where the night ends — see [`NIGHTFALL`]. Ahead of the sunrise at 0.25,
/// so a night waited out ends looking at one.
pub const DAYBREAK: f32 = 0.22;

/// Whether a phase of the day (see [`ToClient::Daylight`]) falls in the
/// night — the span from [`NIGHTFALL`] round midnight to [`DAYBREAK`].
pub fn is_night(phase: f32) -> bool {
    !(DAYBREAK..NIGHTFALL).contains(&phase)
}

/// The port a server listens on, and a client joins on, unless told
/// otherwise. Nothing else claims it, and it is easily remembered as the
/// powers of two run together.
pub const DEFAULT_PORT: u16 = 24816;

/// The longest frame a server will accept from a client. Everything a client
/// says is a couple of dozen bytes — where it is, or which chunk it wants —
/// and the ceiling exists so that a corrupt length prefix reads as corruption
/// instead of as a request to buffer megabytes.
const MAX_CLIENT_FRAME: u16 = 64;

/// The longest frame a client will accept from a server, which is exactly one
/// chunk of ground and not a byte more: its tag, its coordinates, the flag
/// that says what kind of answer this is, the count of palms standing on it,
/// and the payload — the largest kind, which is ground with standing water on
/// it and a full complement of palms.
///
/// Derived rather than picked, so that a message which outgrew it fails to
/// send here instead of arriving as garbage — and so that "how much can one
/// answer cost" has one answer, written down.
const MAX_SERVER_FRAME: u16 =
    (1 + 8 + 1 + 1 + ground::payload_bytes(true, ground::MAX_PALMS)) as u16;

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
/// the workspace. Height is never sent: a player stands on the ground the
/// server sent them, so the server can put them back on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToServer {
    /// The first message on any connection, and never sent again.
    Hello { version: u16 },
    /// Where the player now is.
    Move { position: Vec2 },
    /// Ground, please — one chunk of it, named by its coordinate on the world
    /// grid of [`ground::CHUNK_METRES`] squares.
    ///
    /// A client asks for every chunk near its camera without knowing, or
    /// being able to know, which of them hold land. Answers come back as
    /// [`ToClient::Chunk`] and may arrive in any order: an island takes real
    /// time to generate and open water takes none, so a request for water
    /// posted after one for land will often be answered first.
    WantChunk { chunk: IVec2 },
    /// This player is lying to at anchor waiting the night out, and would
    /// like it over with.
    ///
    /// A standing request rather than an order: it says what this player
    /// wants *now*, and lapses within a beat of the client falling silent,
    /// so taking the helm again is simply a client that has stopped asking.
    /// What the server does with it is its own business — the night runs
    /// fast only while everyone in the world is asking, and a world where
    /// somebody is still sailing keeps its night. Repeat it while the wish
    /// stands; a client that sends it once and stops has changed its mind.
    WantDawn,
}

/// What a server may say.
#[derive(Clone, Debug, PartialEq)]
pub enum ToClient {
    /// The session, granted: who the client is, where the world is entered,
    /// and which way to look when it opens.
    ///
    /// The seed is not here, and that is the point — a client has no use for
    /// one, having nothing to generate. `facing` is a ground point the view
    /// opens towards, so that a player arrives looking at the island they
    /// were put down beside rather than out to sea; a server with nothing in
    /// particular to look at sends the spawn itself, which names no direction.
    Welcome {
        id: PlayerId,
        spawn: Vec2,
        facing: Vec2,
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
    /// The ground a client asked for, or the absence of it: `None` is open
    /// water, which needs no data — the sea and the ocean floor are flat
    /// planes, and a client draws them whether or not anything is on top.
    ///
    /// Most answers are `None`. An island is laid out as a rectangle with a
    /// skirt of open water round it, and between islands there is nothing at
    /// all, so a camera's neighbourhood is mostly sea.
    ///
    /// Ground that carries a lake costs half as much again — see
    /// [`ChunkPayload::water`] — and is a small minority of the ground.
    Chunk {
        chunk: IVec2,
        ground: Option<ChunkPayload>,
    },
    /// What the weather is doing: the wind over the whole world, as a
    /// velocity — direction and metres per second in one vector, so there is
    /// no bearing convention to agree on and a calm is simply a short one.
    ///
    /// Sent once directly after [`ToClient::Welcome`], and again to everyone
    /// whenever it has changed enough to matter. The server is the one
    /// authority on it, exactly as with the ground: weather moves over a
    /// session, every player in a world is under the same sky, and what a
    /// client does with it — how a sea wears this much wind — is drawing,
    /// not simulation.
    Weather {
        wind: Vec2,
    },
    /// Where the world's day stands: the fraction of it since midnight, so
    /// 0.0 is midnight, 0.25 sunrise, 0.5 noon and 0.75 sunset. Always in
    /// `0.0..1.0`.
    ///
    /// Sent with the welcome and on a beat of a second or so after that, and
    /// the server is the authority on it exactly as it is on the weather:
    /// every player in a world is under the same sun, wherever they are and
    /// whatever their machine thinks the time is.
    ///
    /// A client is expected to run the clock itself between tellings, at
    /// [`DAY_SECONDS`] to the turn, and to ease onto each new word rather
    /// than snapping to it — the tellings are a beat apart and a sun that
    /// jumped every second would be a stuttering one. It never runs
    /// backwards: the one thing that moves it other than the clock is a
    /// night being waited out (see [`ToServer::WantDawn`]), which the server
    /// serves by running the same clock fast, so the difference a client has
    /// to make up is always a small step forwards.
    Daylight {
        phase: f32,
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
            Self::WantChunk { chunk } => {
                payload.push(2);
                put_ivec2(&mut payload, *chunk);
            }
            Self::WantDawn => payload.push(3),
        }
        write_frame(to, &payload, MAX_CLIENT_FRAME)
    }

    /// Reads the next message, blocking until a whole frame has arrived.
    pub fn read(from: &mut impl Read) -> io::Result<Self> {
        let frame = read_frame(from, MAX_CLIENT_FRAME)?;
        let mut payload = Payload::over(&frame);
        let message = match payload.u8()? {
            0 => Self::Hello {
                version: payload.u16()?,
            },
            1 => Self::Move {
                position: payload.vec2()?,
            },
            2 => Self::WantChunk {
                chunk: payload.ivec2()?,
            },
            3 => Self::WantDawn,
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
            Self::Welcome { id, spawn, facing } => {
                payload.push(0);
                put_u32(&mut payload, id.0);
                put_vec2(&mut payload, *spawn);
                put_vec2(&mut payload, *facing);
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
            Self::Weather { wind } => {
                payload.push(6);
                put_vec2(&mut payload, *wind);
            }
            Self::Daylight { phase } => {
                payload.push(7);
                put_f32(&mut payload, *phase);
            }
            Self::Chunk { chunk, ground } => {
                payload.push(5);
                put_ivec2(&mut payload, *chunk);
                match ground {
                    // A flag byte rather than three tags, so that what kind of
                    // answer this is is one thing a reader tests and the
                    // coordinates are in the same place whichever it is. It
                    // says how long the rest of the message is, which is why
                    // ground with water on it is a kind of answer here rather
                    // than a detail inside the payload.
                    None => payload.push(0),
                    Some(ground) => {
                        payload.push(if ground.water.is_some() { 2 } else { 1 });
                        // Alongside the flag rather than inside the payload,
                        // and for the same reason: between them they say how
                        // long the rest of the message is, which is what a
                        // reader needs before it can read any of it.
                        payload.push(ground.palms.len() as u8);
                        ground.put(&mut payload);
                    }
                }
            }
        }
        write_frame(to, &payload, MAX_SERVER_FRAME)
    }

    /// Reads the next message, blocking until a whole frame has arrived.
    pub fn read(from: &mut impl Read) -> io::Result<Self> {
        let frame = read_frame(from, MAX_SERVER_FRAME)?;
        let mut payload = Payload::over(&frame);
        let message = match payload.u8()? {
            0 => Self::Welcome {
                id: PlayerId(payload.u32()?),
                spawn: payload.vec2()?,
                facing: payload.vec2()?,
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
            5 => {
                let chunk = payload.ivec2()?;
                let ground = match payload.u8()? {
                    0 => None,
                    flag @ (1 | 2) => {
                        let palms = payload.u8()? as usize;
                        Some(payload.chunk_payload(flag == 2, palms)?)
                    }
                    flag => return Err(corrupt(format!("chunk {chunk} flagged {flag}"))),
                };
                Self::Chunk { chunk, ground }
            }
            6 => Self::Weather {
                wind: payload.vec2()?,
            },
            7 => Self::Daylight {
                phase: payload.f32()?,
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
fn write_frame(to: &mut impl Write, payload: &[u8], limit: u16) -> io::Result<()> {
    // Unreachable for the messages defined above, whose fields were counted
    // against their direction's ceiling; the check is for the message added
    // after this line was last read. Refusing to send beats a length prefix
    // that quietly wrapped in the `as u16` below, which would frame the whole
    // rest of the session as garbage — and only in release builds, where an
    // assertion is not there to say so.
    if payload.len() > limit as usize {
        return Err(corrupt(format!(
            "a {}-byte message does not fit a {limit}-byte frame",
            payload.len()
        )));
    }
    let mut frame = Vec::with_capacity(2 + payload.len());
    put_u16(&mut frame, payload.len() as u16);
    frame.extend_from_slice(payload);
    to.write_all(&frame)
}

/// Reads one frame's payload, blocking until all of it has arrived.
fn read_frame(from: &mut impl Read, limit: u16) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 2];
    from.read_exact(&mut length)?;
    let length = u16::from_le_bytes(length);
    if length > limit {
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

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_vec2(out: &mut Vec<u8>, value: Vec2) {
    out.extend_from_slice(&value.x.to_le_bytes());
    out.extend_from_slice(&value.y.to_le_bytes());
}

fn put_ivec2(out: &mut Vec<u8>, value: IVec2) {
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

    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(
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

    fn ivec2(&mut self) -> io::Result<IVec2> {
        Ok(IVec2::new(self.i32()?, self.i32()?))
    }

    fn chunk_payload(&mut self, water: bool, palms: usize) -> io::Result<ChunkPayload> {
        let bytes = self.take(ground::payload_bytes(water, palms))?;
        ChunkPayload::take(bytes, water, palms).ok_or_else(|| {
            corrupt("a chunk painted in colours this build has never heard of".into())
        })
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
    use super::ground::{Shade, Surface, Tone, FACET_TRIS, FACET_VERTS};
    use super::*;

    fn bytes_of_client(message: ToServer) -> Vec<u8> {
        let mut out = Vec::new();
        message.write(&mut out).expect("a Vec never fails to grow");
        out
    }

    fn bytes_of_server(message: &ToClient) -> Vec<u8> {
        let mut out = Vec::new();
        message.write(&mut out).expect("a Vec never fails to grow");
        out
    }

    /// A chunk of ground whose every byte is a function of where it is, so
    /// that anything which transposed, truncated or reordered the payload
    /// shows up rather than round-tripping perfectly.
    fn a_chunk() -> ChunkPayload {
        ChunkPayload {
            heights: (0..FACET_VERTS * FACET_VERTS)
                .map(|i| (i * 601 % 65_521) as u16)
                .collect(),
            surfaces: (0..FACET_TRIS)
                .map(|i| {
                    Surface::new(
                        [
                            Tone::Seabed,
                            Tone::Sand,
                            Tone::Forest,
                            Tone::Fell,
                            Tone::Snow,
                        ][i % 5],
                        [Shade::Dark, Shade::Plain, Shade::Light][i % 3],
                    )
                })
                .collect(),
            water: None,
            palms: Vec::new(),
        }
    }

    /// The same chunk with a lake on it, its levels a different function of
    /// position from the heights — so a payload that sent one grid twice, or
    /// read the water out of the heights, fails rather than agreeing with
    /// itself.
    fn a_chunk_with_a_lake() -> ChunkPayload {
        ChunkPayload {
            water: Some(
                (0..FACET_VERTS * FACET_VERTS)
                    .map(|i| (i * 907 % 65_519) as u16)
                    .collect(),
            ),
            ..a_chunk()
        }
    }

    #[test]
    fn every_message_survives_the_round_trip() {
        let at = Vec2::new(1234.5, -6789.25);
        for message in [
            ToServer::Hello {
                version: PROTOCOL_VERSION,
            },
            ToServer::Move { position: at },
            ToServer::WantChunk {
                chunk: IVec2::new(-9, 4),
            },
            ToServer::WantDawn,
        ] {
            let bytes = bytes_of_client(message);
            assert_eq!(ToServer::read(&mut bytes.as_slice()).unwrap(), message);
        }

        for message in [
            ToClient::Welcome {
                id: PlayerId(3),
                spawn: at,
                facing: Vec2::new(-1.0, 2.0),
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
            ToClient::Weather {
                wind: Vec2::new(-3.25, 8.5),
            },
            ToClient::Daylight { phase: 0.125 },
            ToClient::Chunk {
                chunk: IVec2::new(3, -8),
                ground: None,
            },
            ToClient::Chunk {
                chunk: IVec2::new(-2, 7),
                ground: Some(a_chunk()),
            },
            ToClient::Chunk {
                chunk: IVec2::new(6, -1),
                ground: Some(a_chunk_with_a_lake()),
            },
        ] {
            let bytes = bytes_of_server(&message);
            assert_eq!(ToClient::read(&mut bytes.as_slice()).unwrap(), message);
        }
    }

    #[test]
    fn the_wire_is_a_format() {
        // The exact bytes, pinned the way the world pins its digests: a server
        // must understand clients built from other checkouts, so changing any
        // of this means bumping PROTOCOL_VERSION, not re-recording the test.
        //
        // Every variant of both enums appears, because the round trip above
        // cannot see any of the ways this format could move while still
        // agreeing with itself: renumbered tags, fields swapped within a
        // message, a length counted wrong. All of those read back perfectly
        // and would still leave two builds unable to talk.
        //
        // The point (1.5, -2.0) is shared by every message that carries one:
        // both halves are exact in binary, and the two differ in every byte
        // that matters, so a pair of axes that swapped places would show.
        assert_eq!(
            bytes_of_client(ToServer::Hello { version: 3 }),
            [3, 0, 0, 3, 0],
            "hello: length 3, tag 0, version LE"
        );
        assert_eq!(
            bytes_of_client(ToServer::Move {
                position: Vec2::new(1.5, -2.0),
            }),
            [
                9, 0, // length
                1, // tag
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
            ],
        );
        assert_eq!(
            bytes_of_client(ToServer::WantChunk {
                chunk: IVec2::new(5, -3),
            }),
            [
                9, 0, // length
                2, // tag
                5, 0, 0, 0, // x = 5
                0xFD, 0xFF, 0xFF, 0xFF, // z = -3, two's complement LE
            ],
        );
        assert_eq!(
            bytes_of_client(ToServer::WantDawn),
            [1, 0, 3],
            "want dawn: length 1, tag 3, and nothing to say"
        );

        assert_eq!(
            bytes_of_server(&ToClient::Welcome {
                id: PlayerId(7),
                spawn: Vec2::new(1.5, -2.0),
                facing: Vec2::new(-2.0, 1.5),
            }),
            [
                21, 0, // length
                0, // tag
                7, 0, 0, 0, // id
                0, 0, 0xC0, 0x3F, // spawn x = 1.5
                0, 0, 0, 0xC0, // spawn z = -2.0
                0, 0, 0, 0xC0, // facing x = -2.0 — the spawn's axes swapped,
                0, 0, 0xC0, 0x3F, // facing z = 1.5, so a confused pair shows
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Refused { version: 9 }),
            [3, 0, 1, 9, 0],
            "refused: length 3, tag 1, version LE"
        );
        assert_eq!(
            bytes_of_server(&ToClient::Joined {
                id: PlayerId(7),
                position: Vec2::new(1.5, -2.0),
            }),
            [
                13, 0, // length
                2, // tag
                7, 0, 0, 0, // id
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Moved {
                id: PlayerId(7),
                position: Vec2::new(1.5, -2.0),
            }),
            [
                13, 0, // length
                3, // tag — Joined's twin, and only the tag tells them apart
                7, 0, 0, 0, // id
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Left { id: PlayerId(7) }),
            [
                5, 0, // length
                4, // tag
                7, 0, 0, 0, // id
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Weather {
                wind: Vec2::new(1.5, -2.0),
            }),
            [
                9, 0, // length
                6, // tag
                0, 0, 0xC0, 0x3F, // x = 1.5
                0, 0, 0, 0xC0, // y = -2.0
            ],
        );
        assert_eq!(
            bytes_of_server(&ToClient::Daylight { phase: 0.75 }),
            [
                5, 0, // length
                7, // tag
                0, 0, 0x40, 0x3F, // phase = 0.75, sunset
            ],
        );

        // Open water: the whole message, since there is nothing in it.
        assert_eq!(
            bytes_of_server(&ToClient::Chunk {
                chunk: IVec2::new(5, -3),
                ground: None,
            }),
            [
                10, 0, // length
                5, // tag
                5, 0, 0, 0, // x = 5
                0xFD, 0xFF, 0xFF, 0xFF, // z = -3
                0,    // no ground here
            ],
        );

        // Ground: too long to write out, so the head, the length and a
        // handful of interior bytes at known offsets. Between them they pin
        // the layout — where the heights start, that they are little-endian
        // pairs, where the surfaces start, and how a surface packs.
        let ground = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::new(5, -3),
            ground: Some(a_chunk()),
        });
        let framed = 1 + 8 + 1 + 1 + ground::PAYLOAD_BYTES;
        assert_eq!(ground.len(), 2 + framed);
        assert_eq!(
            ground[..11],
            [
                (framed & 0xFF) as u8,
                (framed >> 8) as u8, // length
                5,                   // tag
                5,
                0,
                0,
                0, // x = 5
                0xFD,
                0xFF,
                0xFF,
                0xFF, // z = -3
            ],
            "the head of a ground answer"
        );
        assert_eq!(ground[11], 1, "the flag says there is dry ground");
        assert_eq!(ground[12], 0, "and the count says no palms stand on it");

        // Heights start at 13. Corner 0 is 0, corner 1 is 601, corner 2 is
        // 1202 — little-endian pairs.
        assert_eq!(ground[13..19], [0, 0, 0x59, 0x02, 0xB2, 0x04]);

        // Surfaces start once the heights are done. The first is Seabed dark
        // — tone 0 in the high bits, shade 0 in the low two — and the second
        // Sand plain: tone 2, shade 1.
        let surfaces = 13 + FACET_VERTS * FACET_VERTS * 2;
        assert_eq!(
            ground[surfaces..surfaces + 3],
            [0b0000_0000, 0b0000_1001, 0b0001_0010],
            "seabed/dark, sand/plain, forest/light"
        );

        // And the same chunk with a lake on it. The heights and the surfaces
        // must land at exactly the offsets they land at above — water is
        // something a chunk carries in addition, not a rearrangement of what
        // it carried already — so the two answers agree byte for byte up to
        // the end of the surfaces and differ only in the flag and the tail.
        let lake = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::new(5, -3),
            ground: Some(a_chunk_with_a_lake()),
        });
        let wet = 1 + 8 + 1 + 1 + ground::payload_bytes(true, 0);
        assert_eq!(lake.len(), 2 + wet);
        assert_eq!(
            lake[..2],
            [(wet & 0xFF) as u8, (wet >> 8) as u8],
            "a watered chunk is longer by exactly its water grid"
        );
        assert_eq!(
            lake[2..11],
            ground[2..11],
            "the head is the same either way"
        );
        assert_eq!(lake[11], 2, "the flag says there is water on this ground");
        assert_eq!(
            lake[12..surfaces + FACET_TRIS],
            ground[12..surfaces + FACET_TRIS],
            "the water moved the heights or the surfaces"
        );

        // The water grid starts once the surfaces are done, little-endian
        // pairs like the heights: level 0 is 0, level 1 is 907, level 2 is
        // 1814.
        let water = surfaces + FACET_TRIS;
        assert_eq!(lake[water..water + 6], [0, 0, 0x8B, 0x03, 0x16, 0x07]);
    }

    #[test]
    fn a_message_too_big_to_frame_is_refused() {
        // The client's ceiling is far below any message defined here, so this
        // asks the framing directly. The length prefix is a u16 and the
        // ceiling is far below one, so an oversized payload written anyway
        // would arrive as a plausible short frame followed by the rest of it
        // read as messages.
        let mut wire = Vec::new();
        assert!(write_frame(
            &mut wire,
            &vec![0u8; MAX_CLIENT_FRAME as usize + 1],
            MAX_CLIENT_FRAME
        )
        .is_err());
        assert!(wire.is_empty(), "half a frame reached the wire");

        // And the server's ceiling is exactly the largest chunk there is —
        // ground with water on it and as many palms as one may carry — so
        // that answer fits with nothing to spare, and a plainer chunk fits
        // with the difference to spare.
        let mut most = a_chunk_with_a_lake();
        most.palms = vec![
            ground::Palm {
                at: Vec2::ZERO,
                yaw: 0.0,
                scale: ground::PALM_SCALE_MIN,
            };
            ground::MAX_PALMS
        ];
        let biggest = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::ZERO,
            ground: Some(most),
        });
        assert_eq!(
            biggest.len() - 2,
            MAX_SERVER_FRAME as usize,
            "a watered chunk under a full stand of palms is what the ceiling is for"
        );
        let lake = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::ZERO,
            ground: Some(a_chunk_with_a_lake()),
        });
        let ground = bytes_of_server(&ToClient::Chunk {
            chunk: IVec2::ZERO,
            ground: Some(a_chunk()),
        });
        assert_eq!(lake.len() - ground.len(), ground::WATER_BYTES);
        assert_eq!(
            biggest.len() - lake.len(),
            ground::MAX_PALMS * ground::PALM_BYTES
        );
    }

    #[test]
    fn a_client_cannot_be_asked_to_buffer_a_chunk() {
        // The two directions have different ceilings, and the small one is
        // what protects a server from a client claiming to have a great deal
        // to say. A length that would be perfectly legal coming the other way
        // is corruption coming this way.
        let mut wire = Vec::new();
        put_u16(&mut wire, MAX_CLIENT_FRAME + 1);
        wire.extend(std::iter::repeat_n(0, MAX_CLIENT_FRAME as usize + 1));
        assert!(ToServer::read(&mut wire.as_slice()).is_err());
    }

    #[test]
    fn messages_stream_back_to_back() {
        let mut wire = Vec::new();
        let first = ToServer::Hello { version: 2 };
        let second = ToServer::Move {
            position: Vec2::new(8.0, -4.0),
        };
        let third = ToServer::WantChunk {
            chunk: IVec2::new(1, 1),
        };
        first.write(&mut wire).unwrap();
        second.write(&mut wire).unwrap();
        third.write(&mut wire).unwrap();

        let mut reading = wire.as_slice();
        assert_eq!(ToServer::read(&mut reading).unwrap(), first);
        assert_eq!(ToServer::read(&mut reading).unwrap(), second);
        assert_eq!(ToServer::read(&mut reading).unwrap(), third);
        assert!(reading.is_empty());
    }

    /// Ground answers are the one thing on this wire big enough to be split
    /// across packets, so a reader that assumed a frame arrives whole would
    /// pass every test above and fail on a real socket.
    #[test]
    fn a_chunk_split_across_reads_still_arrives() {
        struct Dribble<'a> {
            bytes: &'a [u8],
        }
        impl Read for Dribble<'_> {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                let n = out.len().min(self.bytes.len()).min(1000);
                out[..n].copy_from_slice(&self.bytes[..n]);
                self.bytes = &self.bytes[n..];
                Ok(n)
            }
        }

        // The watered chunk, being the largest answer there is and so the
        // most split.
        let message = ToClient::Chunk {
            chunk: IVec2::new(-2, 7),
            ground: Some(a_chunk_with_a_lake()),
        };
        let wire = bytes_of_server(&message);
        let mut dribble = Dribble { bytes: &wire };
        assert_eq!(ToClient::read(&mut dribble).unwrap(), message);
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

        // A chunk whose flag byte is none of the three kinds of answer.
        let mut bad_flag = vec![10, 0, 5];
        bad_flag.extend([0; 8]);
        bad_flag.push(3);
        assert!(ToClient::read(&mut bad_flag.as_slice()).is_err());

        // And one that claims water and then ends where dry ground would
        // have: the flag is what says how long the payload is, so a frame
        // that disagrees with its own flag is corrupt rather than a chunk
        // with a short lake.
        let mut short_lake = Vec::new();
        put_u16(&mut short_lake, (1 + 8 + 1 + ground::PAYLOAD_BYTES) as u16);
        short_lake.push(5);
        short_lake.extend([0; 8]);
        short_lake.push(2);
        short_lake.extend(std::iter::repeat_n(0, ground::PAYLOAD_BYTES));
        assert!(ToClient::read(&mut short_lake.as_slice()).is_err());

        // And a wire that simply ends is an ordinary end-of-file error.
        assert!(ToServer::read(&mut [].as_slice()).is_err());
    }
}
