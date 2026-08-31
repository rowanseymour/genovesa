//! What grows on a chunk, and the one budget every kind of it spends.
//!
//! A kind knows where its own sort grows — [`crate::palms`] at the back of a
//! beach, [`crate::bananas`] on a wet valley floor, [`crate::mangroves`] in the
//! shallows of a lake — and this is where their answers are put together. The
//! gathering is here rather than in any of them because the wire carries one
//! list under one ceiling: whoever spends the last of it has to be somewhere
//! that can see every claim, not whichever module happened to be asked first.
//!
//! The hash the scatters are drawn from lives here too, for the same reason a
//! palette does: two kinds drawing from two hashes would be two answers to
//! the same question, and a seed has to mean one thing.

use glam::IVec2;
use protocol::ground::{Plant, MAX_PLANTS};

use crate::archipelago::Island;

/// Everything standing on one chunk of an island, of every kind.
///
/// The order is fixed — kind by kind, and each kind in its own lattice
/// order — because it is the order a truncated chunk would keep, and a
/// truncation that differed between machines would be two beaches.
pub fn plants(island: &Island, chunk: IVec2) -> Vec<Plant> {
    let mut found = crate::palms::palms(island, chunk);
    found.extend(crate::bananas::bananas(island, chunk));
    found.extend(crate::mangroves::mangroves(island, chunk));

    // The ceiling is what a chunk's count byte can say, and nothing here comes
    // anywhere near it: the fullest chunk yet measured is a mangrove thicket
    // and carries a third of it, where a beach carries single figures. So
    // this is a backstop against a rule gone wrong rather than a policy for
    // sharing anything out — and if it ever does start biting, the answer is
    // not to keep truncating in the order the kinds are listed above, which
    // would let a beach eat a valley's allowance by being asked first, but to
    // deal the budget between the kinds that want it.
    found.truncate(MAX_PLANTS);
    found
}

/// A cell and a seed, folded to one number.
///
/// Integer arithmetic throughout, and written out here rather than taken from
/// a crate, for the reason the noise gives: a seed has to mean the same thing
/// in every build there will ever be, and a hasher from the lockfile is not
/// that. Splitmix64's finaliser over the two coordinates and the seed packed
/// into one word.
pub(crate) fn mix(cell: IVec2, seed: u32) -> u64 {
    let packed = (cell.x as u32 as u64) | ((cell.y as u32 as u64) << 32);
    let mut z = packed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add((seed as u64).wrapping_mul(0xD1B5_4A32_D192_ED03));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The `nth` independent draw from a hash, in `0.0..1.0`.
///
/// The word is re-finalised per draw rather than sliced into fields, so two
/// draws off the same cell are uncorrelated — sliced bits of one splitmix
/// output are not, and a scatter built on them lines its trees up with its
/// jitter.
pub(crate) fn draw(seed: u64, nth: u32) -> f32 {
    let mut z = seed.wrapping_add((nth as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    // The top 24 bits over 2^24: every value is exactly representable, so the
    // same word gives the same float on any machine.
    (z >> 40) as f32 / (1u32 << 24) as f32
}
