//! Elastic placement and scaling: one solve decides where pending Spaces
//! go on the machines you have (your devices first: they cost nothing) and
//! which cloud machines to add, at the least cost within your limits; then
//! when idle cloud machines go away. Pure: no I/O, deterministic, so every
//! decision can be tested, replayed and shown as a plan before it runs.

pub mod detect;
pub mod geo;
pub mod model;
pub mod scale;
pub mod scenarios;
pub mod sim;
#[cfg(test)]
mod sim_tests;
pub mod solve;
#[cfg(test)]
mod tests;

pub use model::*;
pub use solve::{Opening, evaluate, greedy, greedy_with, lower_bound, plan_valid, solve};
