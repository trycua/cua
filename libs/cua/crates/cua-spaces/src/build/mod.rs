//! Builds: an image from a Dockerfile or a list of steps, made in a Space
//! and kept as pinned checkpoints.
//!
//! One build feature for every surface (`cua build`, `Spaces.build`,
//! `SpaceCreateOptions.build`, the compat layers):
//!
//! 1. A [`BuildRecipe`] is a base image and ordered [`BuildStep`]s, from a
//!    Dockerfile ([`BuildRecipe::from_dockerfile`]) or built directly.
//! 2. Each step that changes the filesystem (`RUN`, `COPY`/`ADD`, `WORKDIR`)
//!    is a layer. A layer's cache key is `hash(previous key, step)`, where a
//!    `COPY` step carries the hashes of the files it copies; the first key
//!    covers the base image (its digest when this machine knows it), the
//!    machine shape and [`RECIPE_VERSION`].
//! 3. [`Spaces::build`] finds the deepest layer already built (checkpoint
//!    `build-<key12>`), forks it into a builder Space (or starts the base
//!    image's), applies the remaining layers through the Space's
//!    cua-spacesd, checkpoints (pinned) after each, and deletes the builder.
//!    The last layer's checkpoint is the result: forks of it start in well
//!    under a second and, on QEMU, share its memory copy-on-write.
//! 4. A tag is an alias (tag to build) so `fork`, `Spaces.create(build)` and
//!    `cua spaces create build:<tag>` find it.
//!
//! Environment, working directory and user from the Dockerfile are the
//! image's [`ImageConfig`], which commands run in a Space made from the
//! build get ([`crate::Space::shell_command`]).

mod base;
mod context;
mod engine;
mod host;
mod plan;
mod recipe;
mod store;

pub use base::{Base, classify_base, python_script, python_version};
pub use engine::{
    BuildEvent, BuildHost, BuildSink, BuilderSpec, ExecOutcome, ExecSpec, LineSink, as_user_in,
};
pub use host::BUILD_IMAGE_PREFIX;
pub use recipe::{
    BuildOptions, BuildRecipe, BuildResult, BuildStep, ImageConfig, RECIPE_VERSION, StepResult,
};
pub use store::{BuildInfo, BuildRecord};

#[cfg(test)]
mod tests;
