//! The world itself, free of any engine: terrain generation, and the plan
//! renderer that judges it from above.
//!
//! Everything here is deterministic — a seed is a map, whoever generates it,
//! on whatever machine — and free of Bevy, because it is a *server's* crate.
//! In a session exactly one process runs this, and hands out what it makes a
//! chunk at a time; the clients drawing that never see any of it. Determinism
//! still matters for the same reason it always did, one step further out: a
//! seed handed to another machine to host has to raise the same islands.
//!
//! The game's own crate holds everything that draws — meshes, materials,
//! cameras and menus — and depends on none of this.

pub mod archipelago;
pub mod bananas;
pub mod cacti;
pub mod mangroves;
pub mod noise;
pub mod palms;
pub mod plan;
pub mod plants;
pub mod sunlight;
pub mod terrain;
pub mod weather;

#[cfg(test)]
mod testing;
