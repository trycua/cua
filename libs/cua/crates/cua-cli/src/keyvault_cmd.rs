//! `cua keyvault`: status, setup, unlock and lock of the Cua Keyvault that
//! `cua daemon` hosts on `$CUA_HOME/keyvault.sock`.
//!
//! The command line is defined here; the Keyvault ships with Cua Spaces
//! (source-available): the Cua Spaces build of `cua` runs these commands,
//! and this build hands them to it (see [`crate::extension`]).
//!
//! Every subcommand is one broker request over the Keyvault IPC client, with
//! the normal server check (`ServerCheck::default_for_build`: a release CLI
//! only talks to the Cua-signed daemon). The broker checks this process's
//! signature in turn, so only a first-party `cua` can create, unlock or lock
//! the vault, and it asks for Touch ID itself where the design requires it.
//!
//! A passphrase is never an argument: `--passphrase` prompts on the terminal
//! with echo off, and `--passphrase-stdin` reads one line from stdin for
//! scripts. It is sent only to the broker, never logged or stored, never in
//! telemetry (which records `keyvault.init`, not arguments), and zeroized.

use clap::{Args, Subcommand};

/// `cua keyvault` subcommands.
#[derive(Subcommand, Debug, Clone)]
pub enum KeyvaultCmd {
    /// Whether the Keyvault exists and is unlocked, and which protectors
    /// (Touch ID with the OS key store, a passphrase) the daemon can use.
    #[command(after_help = "Examples:
  cua keyvault status
  cua keyvault status --json")]
    Status,
    /// Create the Keyvault and print its recovery key once. Uses the OS key
    /// store (Touch ID confirms) unless --passphrase or --passphrase-stdin.
    #[command(after_help = "Examples:
  # The OS key store (the signed Cua daemon only)
  cua keyvault init
  # A passphrase, typed twice at a prompt
  cua keyvault init --passphrase
  # Scripts: the passphrase is the first line of stdin
  cua keyvault init --passphrase-stdin")]
    Init(CredentialArgs),
    /// Unlock the Keyvault with the OS key store, or with its passphrase.
    #[command(after_help = "Examples:
  cua keyvault unlock
  cua keyvault unlock --passphrase")]
    Unlock(CredentialArgs),
    /// Lock the Keyvault (unlocking needs the OS key store or the
    /// passphrase again).
    #[command(after_help = "Examples:
  cua keyvault lock")]
    Lock,
    /// Import a browser's saved passwords into the Keyvault, one item per
    /// site. They stay sealed: agents sign in with them through
    /// `request_site_login` after you approve, and never see them. The
    /// daemon asks for Touch ID or your login password.
    #[command(after_help = "Examples:
  cua keyvault import-passwords --browser chrome
  cua keyvault import-passwords --browser chrome --site github.com --site example.com
  cua keyvault import-passwords --browser chrome --profile \"Profile 1\"")]
    ImportPasswords(ImportPasswordsArgs),
    /// Import a signed-in session (cookies, or a single-app session like
    /// Slack's) into the Keyvault, without delivering it anywhere. A later
    /// `cua teleport push --sandbox NAME` (or the review sheet's "Save to
    /// Keyvault") can deliver it; it stays sealed until then. The daemon
    /// asks for Touch ID or your login password.
    #[command(after_help = "Examples:
  cua keyvault import-session --app chrome
  cua keyvault import-session --app chrome --site github.com --site example.com
  cua keyvault import-session --app firefox --profile \"Profile 1\"
  cua keyvault import-session --app slack")]
    ImportSession(ImportSessionArgs),
    /// Requests waiting for your approval (site logins, teleports).
    #[command(after_help = "Examples:
  cua keyvault requests
  cua keyvault requests --json")]
    Requests,
    /// Approve a waiting request (the daemon asks for Touch ID or your login
    /// password). A site login is approved for one sign-in.
    #[command(after_help = "Examples:
  cua keyvault approve 5f2c9a0e")]
    Approve {
        /// The request id (`cua keyvault requests`).
        id: String,
    },
    /// Decline a waiting request.
    #[command(after_help = "Examples:
  cua keyvault deny 5f2c9a0e")]
    Deny {
        /// The request id.
        id: String,
    },
}

/// `cua keyvault import-passwords`.
#[derive(Args, Debug, Clone)]
pub struct ImportPasswordsArgs {
    /// The browser to import from.
    #[arg(long, default_value = "chrome", value_parser = ["chrome"])]
    pub browser: String,
    /// The browser profile (name or path); default: the default profile.
    #[arg(long)]
    pub profile: Option<String>,
    /// Only this site (a registrable domain such as github.com). Repeatable;
    /// default: every saved site.
    #[arg(long = "site")]
    pub sites: Vec<String>,
}

/// `cua keyvault import-session`.
#[derive(Args, Debug, Clone)]
pub struct ImportSessionArgs {
    /// The app to import from (a teleport provider id: `chrome`, `firefox`,
    /// `slack`, ...; `cua teleport providers` lists every one on this
    /// machine).
    #[arg(long)]
    pub app: String,
    /// The browser profile (name or path); default: the default profile.
    /// Ignored for a single-app provider (Slack, Discord, ...).
    #[arg(long)]
    pub profile: Option<String>,
    /// Only this site's cookies (a registrable domain such as
    /// github.com). Repeatable; a browser with none named imports every
    /// site's cookies. Ignored (the whole session is one item) for a
    /// single-app provider.
    #[arg(long = "site")]
    pub sites: Vec<String>,
    /// Also carry each named site's localStorage / IndexedDB origins.
    #[arg(long)]
    pub include_storage: bool,
    /// Also carry each named site's saved passwords, as part of this same
    /// session item (`cua keyvault import-passwords` keeps them as their
    /// own items instead, one per site).
    #[arg(long)]
    pub include_passwords: bool,
    /// Only session cookies: drop every persistent cookie.
    #[arg(long)]
    pub session_only: bool,
    /// Drop persistent cookies that expire more than 30 days out
    /// (long-lived refresh tokens), keeping short sessions.
    #[arg(long)]
    pub drop_long_lived: bool,
}

/// How to supply the passphrase. Never as an argument value.
#[derive(Args, Debug, Clone, Default)]
pub struct CredentialArgs {
    /// Use a passphrase, typed at a terminal prompt with echo off.
    #[arg(long, conflicts_with = "passphrase_stdin")]
    pub passphrase: bool,
    /// Use a passphrase read from the first line of stdin (scripts).
    #[arg(long)]
    pub passphrase_stdin: bool,
}
