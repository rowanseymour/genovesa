//! What the determinism tests are built out of.
//!
//! Both of this crate's "a seed is the same thing everywhere" tests — the
//! map's, over a height field and its colours, and the world's, over a layout
//! and an island's ground — fold their subject down to one number and pin it.
//! They had a digest apiece, spelled the same way twice; this is that function,
//! written once, so the two tests cannot drift into hashing differently and
//! quietly stop being comparable.

/// FNV-1a over a stream of bytes.
///
/// Dependency-free like the noise, and for the same reason: the digest has to
/// mean the same thing in every build there will ever be, so it cannot be
/// whatever hasher a library version happens to ship.
///
/// Taken as bytes rather than as values because what each test feeds it
/// differs — one a run of floats, the other a layout's integers — while the
/// folding is the same. The *order* the bytes arrive in is part of what the
/// recorded digests pin, so a caller that reorders its stream has changed the
/// answer, exactly as if it had changed the data.
pub fn digest(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01B3);
    }
    hash
}

/// The bytes a float digests as: its bit pattern, little-endian. Never its
/// decimal form, which would round away exactly the last-bit differences
/// between two machines that these digests exist to catch.
pub fn floats(values: impl IntoIterator<Item = f32>) -> impl Iterator<Item = u8> {
    values
        .into_iter()
        .flat_map(|value| value.to_bits().to_le_bytes())
}

/// The same for whole numbers — island origins, sizes and seeds.
pub fn ints(values: impl IntoIterator<Item = i64>) -> impl Iterator<Item = u8> {
    values.into_iter().flat_map(i64::to_le_bytes)
}
