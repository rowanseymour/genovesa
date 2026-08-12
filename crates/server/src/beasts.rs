//! The beasts: the creatures the server *means*.
//!
//! The game's client-side wildlife — the eagles, the seabird lines — is
//! scenery a client raises for itself out of ground it was already sent, and
//! the server has never heard of it. That arrangement holds exactly as long
//! as a creature can never matter, and there are two ways to matter. The
//! shark's way is consequence: it will one day go for a player swimming
//! where it hunts, and a shark that one client could see and another could
//! not would make that moment a private hallucination. The whale's and the
//! dolphins' way is company: they are rare, pointable events, and two
//! players anchored side by side must be able to point at the same one.
//! Birds are texture nobody compares notes on, and stay each client's own.
//!
//! So a beast is run the way a player is: the server holds where it is and
//! tells everyone on a beat, and a client's whole part is to draw what it is
//! told — see [`ToClient::Beast`], one message that is both introduction and
//! movement, so joining a session mid-life needs no catching up beyond the
//! next beat. What a shark looks like, how deep a whale rides, how many
//! dolphins a pod is drawn as: drawing, and none of this module's business.
//!
//! Beasts are session state, not world state, and that is deliberate. Palms
//! travel with the ground because a seed *means* them; two visits to one
//! world must find the same trees on the same beach. A beast is not a fact
//! about the world but something the world is doing, and what it is doing is
//! *now* — so beasts are raised where the players are, live while anyone is
//! near, and are forgotten when the last of them sails away, exactly as the
//! weather is a function of the session's own clock rather than a recording.
//! Nothing here touches the world's digests.
//!
//! What passes for a mind today is a [`Habitat`] apiece: cruise the water
//! that is yours, and — for the kinds that yield — give way to boats. That
//! last one used to be client-side animation when these animals were
//! wildlife; a creature deciding to avoid a player is behaviour *about* a
//! player, so it moved here with them. Anything smarter grows here and
//! nowhere else — and the first real step, the shark's attack, is blocked on
//! the wire before it is blocked on any cleverness, because the server
//! cannot yet see the one thing an attack turns on: whether a player is *in
//! the water*. A client reports a position, not whether its player is afoot,
//! aboard, or wading — a boat has never crossed the wire at all. Teaching
//! [`protocol::ToServer::Move`] to say how a player travels is the next
//! piece of organisation, and it belongs to the wire, not to this file.

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use glam::Vec2;
use protocol::{BeastId, BeastKind, ToClient};

use crate::{broadcast_all, Shared};

/// How often the beasts are minded, which is also how often everyone is told
/// where they are. Slower than the players' own ten-a-second trickle: a
/// cruising animal is the steadiest mover in the world, and a client eases
/// and extrapolates between tellings — see [`ToClient::Beast`] — so four
/// beats a second read as one unbroken glide.
const BEAST_TICK: Duration = Duration::from_millis(250);

/// How far ahead a beast sounds the bottom before swimming there, in metres —
/// a few seconds of cruising, so a turn starts while there is still water to
/// turn in rather than at the sand.
const SOUNDING: f32 = 8.0;

/// How many spots are sounded, per beast wanted, before giving up until the
/// next beat. A coast offers shallows in abundance and open ocean offers
/// deep water without limit; this bounds what a player anchored somewhere
/// that offers neither costs.
const RAISE_ATTEMPTS: u32 = 12;

/// Where and how one kind of beast lives. Every number that differs between
/// a shark, a pod and a whale is here, so the differences can be read in one
/// place — and so the warden below is written once.
struct Habitat {
    kind: BeastKind,
    /// The seafloor this kind calls home, in metres of terrain height —
    /// how each kind claims its own water, exactly as the client-side
    /// wildlife's crossings did before these animals moved to the server.
    /// The shark's band is the sunlit strip where a beach shelves away,
    /// which is where the players are; dolphins ask only for depth; whales
    /// for real depth, which open floor at minus [`OCEAN_DEPTH`] satisfies.
    ///
    /// [`OCEAN_DEPTH`]: protocol::ground::OCEAN_DEPTH
    band: (f32, f32),
    /// Cruising pace, metres per second. A shark ambles; the dolphins' and
    /// the whale's paces are exactly the speeds their crossings kept as
    /// client-side wildlife, and not only for character: the client pitches
    /// a porpoising body by its leap *against this speed*, so the arcs were
    /// tuned at these numbers and a slower whale is a steeper, spy-hopping
    /// whale.
    cruise: f32,
    /// How many of this kind are kept in the waters around each player.
    /// Sharks come in twos — the second is what makes the first read as
    /// *sharks live here* rather than as a one-off — while a pod or a whale
    /// is an event, and two events at once read as an aquarium.
    about: usize,
    /// How far around a player counts as their waters, in metres: this kind
    /// is counted and raised inside this reach. Wider for the animals meant
    /// to be met seldom.
    waters: f32,
    /// The ring a newcomer is raised on, in metres from the player it is
    /// raised for: outside the view at any ordinary zoom, inside `waters`,
    /// so it is discovered rather than watched appearing.
    ring: (f32, f32),
    /// Beyond this far from every player, in metres, this kind is forgotten
    /// — see the module's opening on beasts being session state.
    /// Comfortably past `waters`, so a player tacking along a coast is not
    /// despawning and respawning the same animal with every leg.
    forgotten: f32,
    /// How near a player may come, in metres, before this kind steers away
    /// — zero for a kind that does not yield. The give-way the wildlife
    /// module promises on the client is served from here for the beasts:
    /// dolphins and whales bear away from boats, and the shark pointedly
    /// does not, which is the first thing a player learns about sharks.
    wary: f32,
}

const HABITATS: [Habitat; 3] = [
    Habitat {
        kind: BeastKind::Shark,
        // The top of the band is three wade-depths of water: a shark holds
        // a little off where a player can stand, close enough to circle a
        // swimmer the moment there is such a thing as swimming.
        band: (-6.0, -1.5),
        cruise: 1.4,
        about: 2,
        waters: 450.0,
        ring: (350.0, 450.0),
        forgotten: 700.0,
        wary: 0.0,
    },
    Habitat {
        kind: BeastKind::Dolphins,
        band: (f32::NEG_INFINITY, -4.0),
        cruise: 4.5,
        about: 1,
        waters: 700.0,
        ring: (400.0, 650.0),
        forgotten: 1_000.0,
        wary: 35.0,
    },
    Habitat {
        kind: BeastKind::Whale,
        band: (f32::NEG_INFINITY, -7.0),
        cruise: 2.2,
        about: 1,
        waters: 1_100.0,
        ring: (500.0, 900.0),
        forgotten: 1_500.0,
        wary: 60.0,
    },
];

/// One beast, as the server minds it. The kind is carried rather than
/// implied because the map holds every beast there is, whatever it is, and
/// the tick reads each one's [`Habitat`] off what it finds.
struct Beast {
    kind: BeastKind,
    position: Vec2,
    /// Which way it is swimming, as a unit vector. Pace is the habitat's
    /// business, so bearing is the whole of the state a cruise needs.
    heading: Vec2,
}

impl Beast {
    fn habitat(&self) -> &'static Habitat {
        HABITATS
            .iter()
            .find(|habitat| habitat.kind == self.kind)
            .expect("every kind of beast has a habitat")
    }
}

/// Minds the beasts on a thread of its own: raises them where the players
/// are, swims them, forgets them when everyone has left, and tells the whole
/// roster where they stand on every beat.
///
/// The flock is owned by this thread outright rather than shared — nothing
/// else ever needs to read it, because [`ToClient::Beast`] is an upsert and a
/// newly joined client is caught up by the next beat with nobody having to
/// introduce anything. That is what keeps this file free of locks of its own:
/// the roster's lock is taken to read positions and again to broadcast, and
/// no lock outlives either.
///
/// The thread ends with the session, within a beat of [`Shared::stopping`]
/// being set, and is deliberately not joined — the same terms the sky thread
/// lives on.
pub(crate) fn mind_the_beasts(shared: &Arc<Shared>) {
    let shared = shared.clone();
    thread::spawn(move || {
        let mut beasts: HashMap<BeastId, Beast> = HashMap::new();
        let mut next_id = 0u32;
        // Stirred into every roll the beasts make, so two sessions on one
        // seed do not raise their animals on the same bearings. Decorative
        // randomness: nothing here ever needs to agree with another machine,
        // the tellings being the agreement.
        let mut entropy = scramble(shared.world.seed());

        loop {
            if shared.stopping.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            thread::sleep(BEAST_TICK);

            // Gathered before anything is decided, and the lock dropped
            // before any terrain is sounded: the tick works from one still
            // picture of where everyone is, and never holds the session up
            // while it thinks.
            let players: Vec<Vec2> = {
                let players = shared.players.lock().expect("no poisoned lock");
                players.values().map(|player| player.position).collect()
            };

            // A world with nobody in it keeps no beasts: there is nothing
            // for them to matter to, and nobody to tell. Quietly, with no
            // parting words — a departure emptied the roster, so there is
            // no one left to hear a `BeastGone`.
            if players.is_empty() {
                beasts.clear();
                continue;
            }

            let mut news: Vec<ToClient> = Vec::new();

            // Swim, then forget, then raise — so a beast that cruised out of
            // everyone's reach this very beat is let go rather than told
            // about once more, and one raised this beat is told about by the
            // loop at the bottom rather than trusted to a later one.
            for beast in beasts.values_mut() {
                entropy = scramble(entropy);
                cruise(beast, &shared, &players, entropy);
            }

            beasts.retain(|id, beast| {
                let minded = players
                    .iter()
                    .any(|player| player.distance(beast.position) <= beast.habitat().forgotten);
                if !minded {
                    news.push(ToClient::BeastGone { id: *id });
                }
                minded
            });

            for player in &players {
                for habitat in &HABITATS {
                    let about = beasts
                        .values()
                        .filter(|beast| beast.kind == habitat.kind)
                        .filter(|beast| beast.position.distance(*player) <= habitat.waters)
                        .count();
                    for _ in about..habitat.about {
                        entropy = scramble(entropy);
                        if let Some(beast) = raise(habitat, &shared, *player, entropy) {
                            beasts.insert(BeastId(next_id), beast);
                            next_id += 1;
                        }
                    }
                }
            }

            for (id, beast) in &beasts {
                news.push(ToClient::Beast {
                    id: *id,
                    kind: beast.kind,
                    position: beast.position,
                    velocity: beast.heading * beast.habitat().cruise,
                });
            }

            let players = shared.players.lock().expect("no poisoned lock");
            for word in news {
                broadcast_all(&players, word);
            }
        }
    });
}

/// One beat of being a beast: keep swimming, keep to your water, and — for
/// the kinds that yield — bear away from boats.
///
/// Each beat it sounds the bottom a few seconds ahead. Water in the band
/// means bearing on with only an idle wander — nothing alive swims a ruled
/// line. Water out of the band means trying bearings further and further off
/// the current one, alternating sides so it has no favourite; a beast boxed
/// in on every sounding simply holds course this beat and tries again on the
/// next, a beat being a fraction of a metre.
///
/// The yielding comes first and the band correction after, in that order on
/// purpose: a whale bears away from a boat, but never onto ground that is
/// not a whale's — it will pass close by a hull sooner than beach itself
/// dodging one.
fn cruise(beast: &mut Beast, shared: &Shared, players: &[Vec2], entropy: u32) {
    let habitat = beast.habitat();

    // The wander: a few degrees a beat, either way, always applied — it is
    // what keeps a long straight reach from reading as a bearing being held.
    let wander = (unit(entropy, 0x5EA5) - 0.5) * 0.12;
    beast.heading = turned(beast.heading, wander);

    // Giving way: swing gently off any player inside the wary reach. Eased
    // by being a bounded turn per beat rather than a snap onto the escape
    // bearing, so a yielding animal reads as deciding, not as deflected.
    if habitat.wary > 0.0 {
        if let Some(nearest) = players
            .iter()
            .filter(|player| player.distance(beast.position) < habitat.wary)
            .min_by(|a, b| {
                a.distance(beast.position)
                    .total_cmp(&b.distance(beast.position))
            })
        {
            let away = (beast.position - *nearest).normalize_or(beast.heading);
            let off = beast.heading.angle_to(away);
            beast.heading = turned(beast.heading, off.clamp(-0.25, 0.25));
        }
    }

    if !swimmable(shared, habitat, beast.position + beast.heading * SOUNDING) {
        for step in 1..=8 {
            // 1, -1, 2, -2, … quarter-turns of an eighth each: the nearest
            // useful bearings first, both sides tried evenly.
            let side = if step % 2 == 1 { 1.0 } else { -1.0 };
            let off = side * (step as f32 + 1.0) / 2.0 * std::f32::consts::FRAC_PI_4;
            let tried = turned(beast.heading, off);
            if swimmable(shared, habitat, beast.position + tried * SOUNDING) {
                beast.heading = tried;
                break;
            }
        }
    }

    beast.position += beast.heading * habitat.cruise * BEAST_TICK.as_secs_f32();
}

/// Whether a point is water this kind would be in: seafloor inside the band,
/// and *known* to be. The sounding is [`Archipelago::ready_height`] rather
/// than the generating kind, because this thread ticks on a beat and an
/// island takes real milliseconds to raise — and ground nobody has been near
/// enough to generate is ground no beast needs to be swimming towards.
///
/// [`Archipelago::ready_height`]: world::archipelago::Archipelago::ready_height
fn swimmable(shared: &Shared, habitat: &Habitat, at: Vec2) -> bool {
    shared
        .world
        .ready_height(at.x, at.y)
        .is_some_and(|floor| (habitat.band.0..=habitat.band.1).contains(&floor))
}

/// Tries to raise one beast of a kind in a player's waters: a handful of
/// soundings on the habitat's ring, the first that lands in its band
/// winning. `None` when every try found the wrong water or unmade ground —
/// which costs nothing and is tried afresh next beat.
///
/// It arrives already swimming, on a bearing drawn with its spot: a beast is
/// never doing nothing, and the first telling anyone hears of it carries a
/// velocity like every other.
fn raise(habitat: &'static Habitat, shared: &Shared, player: Vec2, entropy: u32) -> Option<Beast> {
    for attempt in 0..RAISE_ATTEMPTS {
        let bearing = unit(entropy, attempt * 2 + 1) * std::f32::consts::TAU;
        let out =
            habitat.ring.0 + unit(entropy, attempt * 2 + 2) * (habitat.ring.1 - habitat.ring.0);
        let spot = player + Vec2::from_angle(bearing) * out;
        if swimmable(shared, habitat, spot) {
            return Some(Beast {
                kind: habitat.kind,
                position: spot,
                heading: Vec2::from_angle(unit(entropy, 0xF1_5B) * std::f32::consts::TAU),
            });
        }
    }
    None
}

/// A heading swung through `angle` radians — positive one way, negative the
/// other, nobody downstream caring which is which.
fn turned(heading: Vec2, angle: f32) -> Vec2 {
    Vec2::from_angle(angle).rotate(heading)
}

/// Stirs bits until they stop resembling what they were — SplitMix's mixing
/// rounds, without its sequence. Everything random the beasts do comes
/// through here; none of it ever needs to agree with another machine.
fn scramble(mut x: u32) -> u32 {
    x = x.wrapping_add(0x9E37_79B9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 13;
    x = x.wrapping_mul(0xC2B2_AE35);
    x ^ (x >> 16)
}

/// A decorative number in `0.0..1.0` from some bits and a salt.
fn unit(entropy: u32, salt: u32) -> f32 {
    scramble(entropy ^ salt) as f32 / u32::MAX as f32
}
