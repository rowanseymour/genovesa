//! The world itself, free of any engine: terrain generation, and the plan
//! renderer that judges it from above.
//!
//! Everything here is deterministic — a seed is a map, whoever generates it,
//! on whatever machine — and free of Bevy, so it can be built for a headless
//! server or for wasm without a renderer coming along. The game's own crate
//! holds everything that draws: meshes, materials, cameras and menus.

pub mod archipelago;
pub mod args;
pub mod noise;
pub mod plan;
pub mod terrain;

#[cfg(test)]
mod testing;
