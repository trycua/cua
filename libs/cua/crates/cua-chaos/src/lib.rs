//! `cua-chaos`: generic chaos-testing tooling for anything that drives Cua
//! Spaces (the E2B, Daytona and Modal gateways, the Python shim, a script).
//!
//! - [`rng`] and [`plan`]: a seeded scheduler. A plan (timed client
//!   operations and injected faults) is a pure function of `(profile, seed)`,
//!   so every failure replays from its seed.
//! - [`inject`]: fault injection as data (kill, partition, disk-full).
//! - [`inventory`] and [`collect`]: leak-check inventories of a host or a
//!   cloud scope, and the diff that asserts nothing outlived its Space.
//! - [`runner`]: baseline, play, undo, settle, diff.
//!
//! Clients only supply a driver (how to perform an operation) and their
//! state sources; nothing here knows about a particular provider.

pub mod collect;
pub mod inject;
pub mod inventory;
pub mod plan;
pub mod rng;
pub mod runner;

pub use collect::{Source, Sources, collect};
pub use inject::{FaultCmd, FaultTable};
pub use inventory::{Inventory, Item, Leak, Report, Wanted, cross_check};
pub use plan::{Plan, Profile, Step, StepKind};
pub use rng::Rng;
