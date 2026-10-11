//! Cua Spaces: interactive sandboxes on any image, and everything a person
//! or an agent does with one. cua-spacesd, when the image runs it, adds
//! the desktop, shell, files, streaming and teleport primitives.
//!
//! This crate replaces `apps/cua-spaces/mcp/spaces_mcp.py` (and
//! `agent_harness.py`) as the Spaces core. It is plain async Rust over the cua
//! SDK crates: [`cua_spacesd_client`] for every guest call, [`cua_fleet`] for Fleet
//! claims, [`cua_sandbox_core`] for local sandboxes. `cua-daemon` hosts it
//! (and serves [`mcp`] as `cua daemon mcp`); `cua-sdk` wraps it for UniFFI;
//! the Tauri app links it directly.
//!
//! ```no_run
//! # async fn demo() -> cua_spaces::Result<()> {
//! use cua_spaces::Spaces;
//! let spaces = Spaces::builder().build();
//! let info = spaces.add("http://127.0.0.1:3211", Some("token".into()), None).await?;
//! let space = spaces.space(&info.id).await?;
//! println!("{}", space.bash("uname -a", std::time::Duration::from_secs(30)).await?.render());
//! # Ok(()) }
//! ```
//!
//! ## Model
//!
//! - A **Space** is any sandbox (any image), or a direct URL that answers
//!   either a cua-spacesd `GetCapabilities` handshake or MCP
//!   `initialize`. Lifecycle, declared services, tunnels, public URLs and
//!   generic MCP (`list_tools`/`call_tool` with `service=`) need no
//!   spacesd; without one the capability set is empty. Ids are sandbox refs:
//!   `local:<name>`, `cloud:<name>`, `direct:<host:port>`,
//!   `relay:<machine-id>` ([`SpaceId`]; legacy `space://…` ids still parse).
//! - [`Spaces`] is the registry (`~/.cua/spaces.json`, tokens in a separate
//!   0600 `spaces-credentials.json`) and the two ways to get a Space:
//!   [`Spaces::create`] (a new sandbox, local or in the cloud) and
//!   [`Spaces::add`] (an existing machine by address).
//! - [`Space`] is a connected handle. Every spacesd primitive checks the
//!   feature it needs first ([`Space::require`]) and fails fast with
//!   [`Error::CapabilityMissing`] naming it.
//!
//! ## Modules (Cargo features, all on by default)
//!
//! | Feature | Module | What |
//! |---|---|---|
//! | always | [`exec`], [`services`] | `bash`, `write`, driver tools |
//! | always | [`app_icon`] | the icon the guest desktop shows for a window's app |
//! | `spaces-files` | [`files`], [`walk`] | upload / download / `send_file` (ReceiveFiles, gitignore-aware) |
//! | `spaces-stream` | [`stream`] | targets, media tickets, [`stream::StreamSession`] |
//! | `spaces-presence` | [`presence`] | join, roster, cursors |
//! | `spaces-hotspot` | [`hotspot`] | reverse-SOCKS egress through this host |
//! | `spaces-volume` | [`volume`] | Cua Volume mounted in a Space's guest, served from this host |
//! | `spaces-agents` | [`agents`] | agent CLIs as detached, tagged env processes |
//! | `mcp` / `mcp-http` | [`mcp`] | the Spaces MCP server over stdio / streamable HTTP |
//! | `mcp-client` | [`client`] | the typed MCP client + `cua.control.session/1` conformance |
//! | always | [`routines`], [`groups`] | recurring Bot tasks and group chats (app model, no Space calls) |

// The MCP tool dispatcher (agents included) builds deep futures; their
// Send check needs more than the default recursion depth on newer rustc.
#![recursion_limit = "256"]

pub mod app_icon;
/// A hash-chained audit log (the share audit).
pub mod audit;
#[cfg(feature = "mcp-client")]
pub mod client;
/// Spaces in your own cloud account: a cloud sandbox (`cua_sandbox_core::byoc`)
/// shown as its relay machine.
pub mod cloud;
pub mod creating;
pub mod error;
pub mod exec;
pub mod fleet_runtime;
pub mod groups;
pub mod host_spaces;
pub mod id;
pub mod operator;
pub mod registry;
pub mod relay;
pub mod routines;
pub mod services;
mod space;
mod spaces;
pub mod thumbnails;

#[cfg(feature = "spaces-agents")]
pub mod agents;
/// Extension points for the capabilities that ship with Cua Spaces
/// (teleport, the Cua Volume, persistent agents).
#[cfg(feature = "mcp")]
pub mod extension;
#[cfg(feature = "spaces-files")]
pub mod files;
#[cfg(feature = "spaces-hotspot")]
pub mod hotspot;
#[cfg(feature = "spaces-agents")]
pub mod install_cache;
#[cfg(feature = "mcp")]
pub mod mcp;
#[cfg(feature = "spaces-presence")]
pub mod presence;
pub mod reattach;
/// Sharing a Space with other accounts, to watch or to edit, through the
/// relay.
#[cfg(feature = "spaces-agents")]
pub mod share;
/// The Keyvault seam and the whole `request_site_login` tool: sign in to a
/// site in a Space with a saved password, approved by the user.
pub mod site_login;
#[cfg(feature = "spaces-stream")]
pub mod stream;
/// The Keyvault broker seam the `teleport_app` MCP tool routes delivery
/// through (the daemon supplies the implementation).
pub mod teleport_broker;
#[cfg(feature = "spaces-volume")]
pub mod volume;
#[cfg(feature = "spaces-files")]
pub mod walk;

pub use app_icon::{AppIcon, IconRequest};
pub use creating::{CancelOutcome, CancelState};
pub use error::{Error, Result};
pub use id::{Provider, SpaceId};
pub use relay::{RelayAccount, RelayMachine};
pub use space::{SPACESD_FEATURE, ServiceSource, Space, SpaceInfo, SpacePower, SpaceService};
pub use spaces::{
    CreatePhase, CreateProgress, DEFAULT_PROBE_TIMEOUT, PendingSpace, ProgressSink,
    RecoveredCreate, RecoveryOutcome, SpaceCreate, SpaceCreated, Spaces, SpacesBuilder,
    expects_spacesd, pool_key, sanitize_label, sized_pool_key,
};

/// The Spaces tool contract (manifest, input types).
pub use cua_spaces_contract as contract;
/// The spacesd client.
pub use cua_spacesd_client;
