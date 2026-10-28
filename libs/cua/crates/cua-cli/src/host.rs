//! `cua host …` (set this machine up for unattended access, over cua-host)
//! and `cua spaces` (`ls`: registered Spaces plus the account's relay
//! machines; `add` / `rm`: register or forget a Space).

use crate::auth;
use crate::util::{hint_line, line};
use clap::Subcommand;
use cua_host::{
    AccountTokens, DeviceAuth, Host, HostStatus, MachinePatch, RelayClient, RunnerKind,
    ServiceManager, SetupOptions,
};
use cua_sdk::CuaError;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// What `cua host setup --share` shares, the app's sharing switch.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostShare {
    /// Share this computer's own desktop only.
    Desktop,
    /// Provide Spaces for your devices; keep this desktop private.
    Spaces,
    /// Share this desktop and provide Spaces.
    Both,
    /// Share nothing (setup refuses: there is nothing to set up).
    None,
}

impl HostShare {
    /// `(share_desktop, provide_spaces)` the value asks for.
    pub fn settings(self) -> (bool, bool) {
        match self {
            Self::Desktop => (true, false),
            Self::Spaces => (false, true),
            Self::Both => (true, true),
            Self::None => (false, false),
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum HostCmd {
    /// Set this computer up for unattended access: install cua-spacesd as
    /// a service that joins the cua.ai relay as your account (default) or
    /// serves on a direct ip:port. By default it
    /// shares this computer's desktop and, on a computer set up for the
    /// first time through the relay, provides Spaces to your devices (a
    /// host in your pool: `cua run` jobs and `--on auto` Spaces can run
    /// here, within its limits). `--share` picks what it shares: `desktop`,
    /// `spaces` (its own desktop stays private), `both` or `none`;
    /// `cua host config` changes it later. A computer set up before keeps
    /// what it provides. Running it again with the same options changes
    /// nothing (a stopped service starts again).
    #[command(after_help = "Examples:
  # Join the cua.ai relay as your account (shares this desktop and
  # provides Spaces to your devices)
  cua host setup
  # Share this desktop only
  cua host setup --share desktop
  # A spare computer: do not share its desktop, run Spaces for your devices
  cua host setup --share spaces --name \"Mac mini (spare)\"
  # Add this computer with a one-time code from the Cua Spaces app
  # (Add a computer) or `cua host pair`: no sign-in needed here
  cua host setup --pair K7QM-4XPB-2RHD
  # Keep this computer off the end-to-end encrypted mesh (relay HTTPS only):
  # run setup with CUA_MESH=off in the environment
  # Self-hosting teams: cua help advanced")]
    Setup {
        /// Relay URL (default $CUA_RELAY_URL, else the CUA_ENV relay, https://relay.cua.ai in production).
        #[arg(long, hide = true, conflicts_with = "direct")]
        relay: Option<String>,
        /// Join with a one-time pairing code (or add-machine link) made in
        /// the Cua Spaces app or with `cua host pair`, instead of signing
        /// in here. The computer joins the account (or team) that made it.
        #[arg(long, value_name = "CODE", conflicts_with_all = ["direct", "remote"])]
        pair: Option<String>,
        /// When sign-in is needed, use a device code to approve it in a
        /// browser on another computer. The account session is not saved.
        #[arg(long, conflicts_with = "direct")]
        remote: bool,
        /// Serve directly on ip:port with a local env token (LAN /
        /// port-forwarded) instead of joining a relay.
        #[arg(long, hide = true)]
        direct: Option<String>,
        /// Display name (default: the host name).
        #[arg(long)]
        name: Option<String>,
        /// Also allow this account (id or email). Repeatable.
        #[arg(long = "allow")]
        allow: Vec<String>,
        /// cua-spacesd binary (default $CUA_SPACESD_BIN, a bundled
        /// one, or the release download).
        #[arg(long)]
        driver_bin: Option<PathBuf>,
        /// auto | systemd | launchd | app-helper | windows-task | process.
        /// On a Mac with the Cua Spaces app, auto is app-helper: the
        /// service runs as the app's own login item ("Cua Spaces").
        #[arg(long, default_value = "auto")]
        runner: String,
        /// What this computer shares: its own desktop (screen, input,
        /// shell, files), the Spaces it creates for your enrolled devices
        /// (Lume macOS VMs, Docker Linux containers), both, or neither
        /// (refused). Default: the desktop, plus Spaces on a new relay
        /// setup; a computer set up before keeps what it provides.
        #[arg(
            long,
            value_enum,
            value_name = "WHAT",
            conflicts_with_all = ["profile", "desktop", "no_desktop", "provide_spaces", "no_provide_spaces"]
        )]
        share: Option<HostShare>,
        /// Hidden alias: `--share spaces` is `--profile spare`.
        #[arg(long, hide = true, value_parser = ["desktop", "spare"])]
        profile: Option<String>,
        /// Hidden alias of the desktop half of `--share`.
        #[arg(long, hide = true, overrides_with = "no_desktop")]
        desktop: bool,
        /// Hidden alias of `--share spaces` with `--provide-spaces`.
        #[arg(long, hide = true)]
        no_desktop: bool,
        /// Hidden alias: with the default profile, `--share both`.
        #[arg(long, hide = true, overrides_with = "no_provide_spaces")]
        provide_spaces: bool,
        /// Hidden alias: with the default profile, `--share desktop`.
        #[arg(long, hide = true)]
        no_provide_spaces: bool,
        /// Provided Spaces at once (default 4; 0: no limit).
        #[arg(long)]
        max_spaces: Option<u32>,
        /// With --direct and Spaces: take host calls, and connections to
        /// the Spaces this computer forwards, from any address (and allow a
        /// public --direct address). By default only loopback, Tailscale
        /// and private LAN addresses may. The direct listener is plaintext
        /// and its token is a shell on this computer: prefer Tailscale.
        #[arg(long, hide = true, requires = "direct")]
        allow_any_address: bool,
        /// Serve on this machine's tailnet address (Tailscale or Headscale;
        /// port 3211 unless --direct says otherwise), and let the tailnet
        /// identify callers: you (this machine's tailnet user) and the
        /// tailnet users you --allow need no env token. Your tailnet's ACLs
        /// decide who can reach it at all.
        #[arg(long, hide = true, conflicts_with = "relay")]
        tailnet: bool,
        /// This computer exists only for Cua work (a cloud instance made for
        /// it; `cua cloud add aws` sets its hosts up this way): jobs may run in its
        /// own Space (`kind = "machine"`), with its GPU and drivers as they
        /// are. Never on a computer you also use: there jobs run in macOS VMs
        /// or containers.
        #[arg(long)]
        dedicated: bool,
    },
    /// Make a one-time code (with a link and the install command) that adds
    /// another computer to your account: run the command it prints on that
    /// computer. Codes work once and expire (30 minutes by default). Run
    /// from an enrolled device.
    #[command(after_help = "Examples:
  cua host pair
  cua host pair --name build-box --expires 60
  cua host pair --list
  cua host pair --revoke pc_0123456789abcdef
  # Add the computer to a team: waits for it to join, then adds it
  cua host pair --team AIGIS")]
    Pair {
        /// Add the computer to this team (id, alias or name; owner or admin):
        /// waits until the computer joins, then makes it a team computer.
        #[arg(long)]
        team: Option<String>,
        /// With --team: print the code and exit instead of waiting; add the
        /// computer later with `cua team add-machine <team> relay:<id>`.
        #[arg(long, requires = "team")]
        no_wait: bool,
        /// Name the computer gets (default: its own host name).
        #[arg(long)]
        name: Option<String>,
        /// Minutes until the code expires (1 to 1440).
        #[arg(long, value_name = "MINUTES", default_value_t = 30)]
        expires: u64,
        /// List your codes (used, unused, revoked, expired) instead.
        #[arg(long, conflicts_with_all = ["revoke", "team", "name"])]
        list: bool,
        /// Revoke an unused code by id (`pc_…`).
        #[arg(long, value_name = "ID", conflicts_with_all = ["team", "name"])]
        revoke: Option<String>,
        /// Relay URL (default $CUA_RELAY_URL, else the CUA_ENV relay, https://relay.cua.ai in production).
        #[arg(long, hide = true)]
        relay: Option<String>,
    },
    /// Show or change what this computer shares: its own desktop, and
    /// whether it creates Spaces for your other devices.
    #[command(after_help = "Examples:
  cua host config
  # Stop sharing this desktop; keep providing Spaces
  cua host config --desktop off --provide-spaces on
  cua host config --max-spaces 6")]
    Config {
        /// Share this computer's own desktop: on | off.
        #[arg(long, value_parser = ["on", "off"])]
        desktop: Option<String>,
        /// Create Spaces for your enrolled devices: on | off.
        #[arg(long, value_parser = ["on", "off"])]
        provide_spaces: Option<String>,
        /// Provided Spaces at once (0: no limit).
        #[arg(long)]
        max_spaces: Option<u32>,
        /// macOS VMs at once, 0 to 2 (Apple's license allows two per Mac).
        #[arg(long)]
        max_macos_vms: Option<u32>,
    },
    /// Show whether this computer is set up, the service, and who is
    /// connected.
    #[command(after_help = "Examples:
  cua host status")]
    Status,
    /// Stop sharing: disconnect everyone and refuse new connections.
    #[command(after_help = "Examples:
  cua host stop")]
    Stop,
    /// Start sharing again after `stop`.
    #[command(after_help = "Examples:
  cua host start")]
    Start,
    /// Share a relay computer (this one, or `--machine`) with people in your
    /// team (id or verified email). Run from an enrolled device; every
    /// change is in the owner's audit log (`cua devices audit`).
    #[command(after_help = "Examples:
  cua host share friend@example.com
  cua host share friend@example.com --machine 0123abcd4567ef89 --yes")]
    Share {
        /// Accounts to add. Repeatable.
        #[arg(required = true)]
        who: Vec<String>,
        /// Computer id (default: this computer).
        #[arg(long, visible_alias = "computer", value_name = "COMPUTER")]
        machine: Option<String>,
        /// Relay URL (default: the one this computer joined, else
        /// $CUA_RELAY_URL / https://relay.cua.ai).
        #[arg(long, hide = true)]
        relay: Option<String>,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Stop sharing a relay computer with accounts; their open connections
    /// are cut.
    #[command(after_help = "Examples:
  cua host unshare friend@example.com")]
    Unshare {
        /// Accounts to remove. Repeatable.
        #[arg(required = true)]
        who: Vec<String>,
        /// Computer id (default: this computer).
        #[arg(long, visible_alias = "computer", value_name = "COMPUTER")]
        machine: Option<String>,
        /// Relay URL.
        #[arg(long, hide = true)]
        relay: Option<String>,
    },
    /// macOS: move the host service into the Cua Spaces app, as its "Cua
    /// Spaces Helper" login item, from the LaunchAgent that older versions
    /// installed. This machine keeps its id, registration and Spaces; if
    /// the helper does not take over, the old service is put back. macOS
    /// then asks once for Screen Recording and Accessibility for Cua Spaces
    /// Helper.
    #[command(after_help = "Examples:
  cua host migrate
  # Try again after a move that was rolled back
  cua host migrate --force")]
    Migrate {
        /// Try again even if a move failed before with this version.
        #[arg(long)]
        force: bool,
    },
    /// Unregister from the relay, uninstall the service and delete the host
    /// configuration (asks first).
    #[command(after_help = "Examples:
  cua host remove
  # Without the prompt (required in scripts and agents)
  cua host remove --force")]
    Remove {
        /// Skip the confirmation prompt. Required when there is no terminal
        /// to ask on (scripts, agents): without it nothing is removed.
        #[arg(long, short)]
        force: bool,
        /// For an uninstall of Cua Spaces: turns off the Spaces this computer
        /// provides and takes them off the relay too, instead of refusing
        /// while it provides any. The Spaces and their data stay on this
        /// computer.
        #[arg(long)]
        uninstalling: bool,
    },
}

/// `cua machines`: your computers on the relay.
#[derive(Subcommand, Debug)]
pub enum MachinesCmd {
    /// Remove a computer that is gone: its row leaves your relay directory,
    /// then the signed-in device with exactly its name is revoked (never this
    /// device; nothing when several match). A computer that is connected now
    /// is refused: run `cua host remove` on it.
    #[command(after_help = "Examples:
  cua machines rm old-laptop
  cua machines rm relay:0123abcd4567ef89 --yes --json")]
    Rm {
        /// The computer's name, machine id or `relay:<id>` (`cua spaces ls`).
        #[arg(value_name = "NAME")]
        name: String,
        /// Remove without asking.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SpacesCmd {
    /// Registered Spaces and your computers on the relay.
    #[command(
        visible_alias = "list",
        after_help = "CPU, MEMORY and DISK ON THIS MAC show what each Space was given and what it
uses now (the numbers the Cua Spaces app shows); for another of your
computers they show what it last reported. Numbers only: no file names.

Examples:
  cua spaces ls
  cua spaces ls --no-size
  cua spaces ls --json
  # Only the Spaces created with these labels
  cua spaces ls --label team=research --label run=7"
    )]
    Ls {
        /// Relay URL (default $CUA_RELAY_URL, else the CUA_ENV relay, https://relay.cua.ai in production).
        #[arg(long, hide = true)]
        relay: Option<String>,
        /// Only Spaces with this label (`key=value`; repeat to require
        /// several).
        #[arg(long, value_name = "KEY=VALUE")]
        label: Vec<String>,
        /// Do not measure what each Space takes on this computer's disk
        /// (that walk can take a moment with many Spaces).
        #[arg(long)]
        no_size: bool,
    },
    /// Register a Space after a capabilities handshake with its cua-spacesd
    /// (or, for another URL, an MCP initialize). Re-adding an address
    /// updates it. The token is stored apart from the registry, 0600.
    #[command(after_help = "Examples:
  cua spaces add 10.0.0.5:3211 --token \"$TOKEN\" --name studio
  cua spaces add local:my-sandbox
  cua spaces add http://127.0.0.1:8765/mcp --service tools
  # A computer that provides Spaces over Tailscale, without the relay
  # (`cua host setup --direct ... --share both` on it prints this)
  cua spaces add 100.101.102.103:3211 --host --name \"Mac mini (spare)\" --token \"$TOKEN\"")]
    Add {
        /// The Space's cua-spacesd (`http(s)://host:port`, `host:port`,
        /// default port 3211), a Space id (`local:<name>`),
        /// or any MCP endpoint URL.
        url: String,
        /// spacesd token (or the bearer an MCP endpoint needs).
        #[arg(long, env = "CUA_ENV_TOKEN", hide_env_values = true)]
        token: Option<String>,
        /// Display name (default: the guest's hostname).
        #[arg(long)]
        name: Option<String>,
        /// For an MCP endpoint URL: the service name to register it under
        /// (default `mcp`).
        #[arg(long)]
        service: Option<String>,
        /// It is a computer that provides Spaces over its direct address
        /// (Tailscale or LAN): `--on host:<name>` finds it without the
        /// relay. Its token is stored like any direct Space's.
        #[arg(long, conflicts_with = "service")]
        host: bool,
    },
    /// Create Spaces: on this computer, on one of your computers that
    /// provides Spaces (`cua host setup --share spaces` there),
    /// or in your own cloud account.
    #[command(after_help = "Examples:
  cua spaces create linux
  # On the least-loaded of your machines
  cua spaces create linux --on auto
  # Two macOS VMs on your spare Mac mini (matched by name)
  cua spaces create macos:26 --on \"spare mac mini\" --count 2
  cua spaces create --on host:0123abcd4567ef89 linux --json
  # Kept on your machines: created again on another one if its machine goes away
  cua spaces create linux --on auto --durable --name research
  # In your AWS account, on a pool host of exactly this type and region
  cua spaces create linux --on aws --instance-type c7g.xlarge --region us-west-2
  # Labels are kept with the Space; `cua spaces ls --label team=research` filters
  cua spaces create linux --label team=research --label run=7")]
    Create {
        /// Image: an alias (`linux`, `macos`, `macos:26`) or a registry
        /// reference. Default: the canonical Linux image.
        image: Option<String>,
        /// Where: `local`, `auto` (the least-loaded of your computers),
        /// `aws`, `gcp`, `modal`, `host:<computer>`, or a computer name
        /// (`mac-mini`, "spare mac mini"). Default: your default location
        /// (`local` unless set).
        #[arg(long)]
        on: Option<String>,
        /// How many (1 to 8).
        #[arg(long, default_value_t = 1)]
        count: u32,
        /// Name (with --count, `-2`, `-3`, ... are appended).
        #[arg(long)]
        name: Option<String>,
        /// auto | container | vm | microvm.
        #[arg(long)]
        kind: Option<String>,
        /// auto, or an engine the location offers (lume, runc, gvisor, qemu).
        #[arg(long)]
        runtime: Option<String>,
        /// vCPUs.
        #[arg(long)]
        cpus: Option<u32>,
        /// Memory in MiB.
        #[arg(long)]
        memory_mb: Option<u64>,
        /// Grow a VM's disk to this many GiB.
        #[arg(long)]
        disk_gb: Option<u32>,
        /// A GPU: the runtime's own (`--gpu`), or an option `cua spaces
        /// gpus` lists. macOS VMs: GPU acceleration (experimental, Apple
        /// silicon; https://cua.ai/docs/lume/guides/gpu-passthrough).
        #[arg(long, num_args = 0..=1, default_missing_value = "auto", value_name = "OPTION")]
        gpu: Option<String>,
        /// Keep it on your machines (with `--on auto`): when its machine
        /// goes away it is created again on another one, with the same name
        /// (its disk starts fresh). `cua spaces delete <name>` deletes it
        /// for good.
        #[arg(long)]
        durable: bool,
        /// A fork source (a "bake" Space): a local QEMU VM's guest RAM is a
        /// shared file, so `cua sb checkpoint` takes milliseconds and forks
        /// of its checkpoints share its pages. Other kinds ignore it.
        #[arg(long)]
        forkable: bool,
        /// The cloud machine type of the pool host it runs on (with `--on
        /// <cloud>` or `--on auto`; default: the pool picks).
        #[arg(long)]
        instance_type: Option<String>,
        /// The cloud region or location of that host (default: the pool
        /// picks, near you).
        #[arg(long)]
        region: Option<String>,
        /// Mount a named volume of Cua Volume in the Space:
        /// `NAME:PATH[:ro]`, or `NAME/SUBFOLDER:PATH[:ro]` for a folder of
        /// it. Repeat for several. Make one with `cua volume named create`.
        #[arg(long = "volume", value_name = "NAME:PATH[:ro]")]
        volumes: Vec<String>,
        /// A label to keep with the Space (`key=value`; repeat for more; up
        /// to 64, keys up to 63 bytes, values up to 256, none empty). Not
        /// with `--durable`.
        #[arg(long, value_name = "KEY=VALUE")]
        label: Vec<String>,
        /// Block this destination from the guest's first instruction (IP,
        /// CIDR or `all`; repeat for more). Enforced by the host, outside
        /// the guest. See `cua spaces egress`.
        #[arg(long, value_name = "ADDR|CIDR|all")]
        egress_deny: Vec<String>,
        /// Keep this destination reachable where --egress-deny blocks it.
        #[arg(long, value_name = "ADDR|CIDR|all")]
        egress_allow: Vec<String>,
    },
    /// Keep Spaces of one image ready where the elastic pool puts them, so
    /// the next creates of that shape start in a fraction of a second
    /// (`--list` shows the prewarms, `--remove ID` ends one).
    #[command(after_help = "Examples:
  cua spaces prewarm ghcr.io/trycua/linux:24.04-slim --count 16 --on aws --ttl 1h
  cua spaces prewarm linux --count 4 --kind container --cpus 1 --memory-mb 512
  cua spaces prewarm --list
  cua spaces prewarm --remove pw-1a2b3c")]
    Prewarm {
        /// Image (an alias or a registry reference).
        #[arg(required_unless_present_any = ["list", "remove"])]
        image: Option<String>,
        /// How many to keep ready (1 to 256).
        #[arg(long, default_value_t = 1)]
        count: u32,
        /// Where: `auto` (default), `local`, a linked cloud or
        /// `host:<machine>`.
        #[arg(long)]
        on: Option<String>,
        /// How long to keep them (`30m`, `2h`; default 1h, at most 7d).
        #[arg(long)]
        ttl: Option<String>,
        /// auto | container | vm | microvm.
        #[arg(long)]
        kind: Option<String>,
        /// vCPUs each.
        #[arg(long)]
        cpus: Option<u32>,
        /// Memory each, MiB.
        #[arg(long)]
        memory_mb: Option<u64>,
        /// Only this cloud machine type.
        #[arg(long)]
        instance_type: Option<String>,
        /// Only this cloud region.
        #[arg(long)]
        region: Option<String>,
        /// List the prewarms and how many each keeps ready.
        #[arg(long, conflicts_with_all = ["image", "remove"])]
        list: bool,
        /// End a prewarm (its ready Spaces go).
        #[arg(long, value_name = "ID", conflicts_with = "image")]
        remove: Option<String>,
    },
    /// Choose which destinations a Space may reach: block some, or block
    /// everything except a few. The machine the Space runs on enforces it,
    /// outside the guest (nothing in the guest, root included, can change
    /// it). Replaces the whole policy at once; `--clear` removes it. By IP
    /// address or CIDR (`all` is every address), not by hostname. DNS works
    /// only while the resolver the guest uses is not blocked.
    ///
    /// Everything is allowed unless a rule says otherwise: every --deny
    /// entry blocks that destination, and every --allow entry is an
    /// exception that wins over a --deny. --allow alone blocks nothing.
    /// Loopback, replies to connections made to the Space and IPv6 neighbor
    /// discovery are never filtered. Containers (runc, gVisor), and QEMU and
    /// Firecracker VMs on Linux take a policy; macOS VMs, Apple
    /// Containerization, Cua Cloud and your own cloud accounts refuse one
    /// and say why.
    #[command(after_help = "Examples:
  # Block one network
  cua spaces egress studio --deny 10.0.0.0/8
  # Only 1.1.1.1 and one network stay reachable
  cua spaces egress studio --deny all --allow 1.1.1.1 --allow 203.0.113.0/24
  # Back to unrestricted
  cua spaces egress studio --clear")]
    Egress {
        /// Space id or name.
        space: String,
        /// A destination to block (IPv4/IPv6 address, CIDR or `all`; repeat
        /// for more).
        #[arg(long, value_name = "ADDR|CIDR|all")]
        deny: Vec<String>,
        /// A destination that stays reachable where --deny blocks it
        /// (repeat for more).
        #[arg(long, value_name = "ADDR|CIDR|all")]
        allow: Vec<String>,
        /// Remove the policy.
        #[arg(long, conflicts_with_all = ["deny", "allow"])]
        clear: bool,
    },
    /// Cancel a create that is still running (here, in the daemon, or in
    /// another terminal): it stops and what it made is removed. Finished
    /// image downloads stay cached. Ctrl-C during `cua spaces create` does
    /// the same.
    #[command(after_help = "Examples:
  cua spaces cancel local:space-1a2b3c
  cua spaces cancel studio")]
    Cancel {
        /// The id the Space will have (`local:<name>`, printed when the
        /// create starts) or its name.
        space: String,
    },
    /// The GPU options each runtime offers on this computer (or in your
    /// own cloud account).
    #[command(after_help = "Examples:
  cua spaces gpus
  cua spaces gpus --json")]
    Gpus {
        /// `local` (default), `aws`, `gcp` or `modal`.
        #[arg(long)]
        on: Option<String>,
    },
    /// Delete a Space's sandbox (on this computer, on the computer that
    /// provides it, or in your own cloud account) and forget it (asks first).
    #[command(after_help = "Examples:
  cua spaces delete relay:space-0123456789abcdef
  cua spaces delete local:studio --force")]
    Delete {
        /// Space id or name.
        space: String,
        /// Skip the confirmation (required without a terminal).
        #[arg(long, short)]
        force: bool,
    },
    /// Turn a Space off: a local VM (QEMU or Lume) or a Space one of your
    /// computers provides is stopped, keeping its disk and freeing its
    /// memory, and so is a Space in your own cloud (`--on aws`, `gcp`:
    /// compute billing stops; `hetzner`: still billed). A local container
    /// is stopped or suspended as its runtime can. `cua spaces suspend`
    /// keeps a local Space's memory instead. Modal sandboxes and Spaces
    /// added by address cannot be turned off.
    #[command(after_help = "Examples:
  cua spaces stop local:studio
  cua spaces stop relay:space-0123456789abcdef --json")]
    Stop {
        /// Space id or name.
        space: String,
    },
    /// Suspend a local Space (a container or QEMU VM): its memory is kept,
    /// and stays in use, so it resumes where it was with `cua spaces
    /// start`. `cua spaces stop` frees the memory instead.
    #[command(after_help = "Examples:
  cua spaces suspend local:studio")]
    Suspend {
        /// Space id or name.
        space: String,
    },
    /// Give a local VM Space more or fewer vCPUs, more or less memory, or a
    /// bigger disk. It shuts down and boots again from its own disk (a
    /// stopped one boots): its files stay, its running processes do not. A
    /// disk never shrinks.
    #[command(after_help = "Examples:
  cua spaces resize local:builder --memory-mb 24576
  cua spaces resize builder --cpus 8 --disk-gb 200 --json")]
    Resize {
        /// Space id or name.
        space: String,
        /// vCPUs.
        #[arg(long)]
        cpus: Option<u32>,
        /// Memory in MiB.
        #[arg(long)]
        memory_mb: Option<u64>,
        /// Grow the disk to this many GiB.
        #[arg(long)]
        disk_gb: Option<u32>,
    },
    /// Turn a Space back on: resume a suspended one or boot a stopped one.
    #[command(after_help = "Examples:
  cua spaces start local:studio
  cua spaces start studio --json")]
    Start {
        /// Space id or name.
        space: String,
    },
    /// Forget a registered Space and its stored token. The sandbox keeps
    /// running. A Space cua created (on this computer or in a cloud) is
    /// refused while its sandbox exists: use `cua spaces delete` (or `stop`).
    /// The `relay:<computer>` record of a Space that registered
    /// itself and is gone (not connected) also leaves your relay directory.
    #[command(
        visible_alias = "remove",
        after_help = "Examples:
  cua spaces rm direct:10.0.0.5:3211
  cua spaces rm studio"
    )]
    Rm {
        /// Space id, address or display name (as `cua spaces ls` shows).
        space: String,
    },
    /// Give a Space a new name; its id stays the same. Lowercase letters,
    /// digits and dashes, as at create. A Space you shared, or one of your
    /// computers provides, is renamed on the relay too, so your other
    /// devices and the people it is shared with see the new name. Only its
    /// owner can rename a Space.
    #[command(after_help = "Examples:
  cua spaces rename local:studio design-review
  cua spaces rename relay:space-0123456789abcdef ci-runner")]
    Rename {
        /// Space id or name (as `cua spaces ls` shows).
        space: String,
        /// The new name.
        name: String,
    },
    /// Let someone in your team watch (viewer) or use (editor) a Space,
    /// through the relay. They open it as `relay:<computer>`. Sharing is for
    /// teams: you need a team, and they must be in it (or share with the
    /// whole team as `team:<id>`).
    #[command(after_help = "Examples:
  cua spaces share local:studio bob@example.com
  cua spaces share local:studio bob@example.com --role editor
  cua spaces share relay:0123abcd4567ef89 acct-42 --role viewer
  # The whole team, to watch
  cua spaces share local:studio team:acme --role viewer
  # From a script or agent: confirm with Touch ID or your password here
  cua spaces share local:studio bob@example.com --yes")]
    Share {
        /// Space id, address or display name.
        space: String,
        /// Their email (verified by cua.ai) or account id, in your team; or
        /// `team:<id>` for the whole team.
        who: String,
        /// `viewer`: presence and a view-only stream. `editor`: full use.
        #[arg(long, default_value = "viewer", value_parser = ["viewer", "editor"])]
        role: String,
        /// Confirm without a terminal prompt. Without a daemon the share
        /// happens at once. A running daemon still asks with Touch ID or
        /// your password on this computer; without a terminal, `--yes`
        /// says someone will answer there (otherwise the command fails
        /// at once instead of waiting for a prompt nobody sees).
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Stop sharing a Space with one account at once, or with everyone.
    #[command(after_help = "Examples:
  cua spaces unshare local:studio bob@example.com
  cua spaces unshare local:studio --all")]
    Unshare {
        /// Space id, address or display name.
        space: String,
        /// Their email or account id.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        who: Option<String>,
        /// Stop sharing with everyone (the Space leaves the relay).
        #[arg(long)]
        all: bool,
    },
    /// Who a Space is shared with, and who of them is connected now.
    #[command(after_help = "Examples:
  cua spaces shares local:studio
  cua spaces shares local:studio --json")]
    Shares {
        /// Space id, address or display name.
        space: String,
    },
    /// Publish a Space on the relay for your other devices (a phone off
    /// this network): its own driver dials out, as `cua host setup` does
    /// for this computer. Nobody else gets in until you share it.
    #[command(
        name = "relay-register",
        after_help = "Examples:
  cua spaces relay-register local:studio"
    )]
    RelayRegister {
        /// Space id, address or display name.
        space: String,
    },
    /// Take a Space off the relay (every share with it ends). With a
    /// `relay:<computer>` id, removes that computer's record from your relay
    /// directory: a Space that registered itself (also after it was
    /// deleted), or a computer of yours that is gone. Only its owner can.
    #[command(
        name = "relay-unregister",
        after_help = "Examples:
  cua spaces relay-unregister local:studio
  cua spaces relay-unregister relay:space-0123abcd4567ef89"
    )]
    RelayUnregister {
        /// Space id, address or display name.
        space: String,
    },
}

/// Checks a `cua spaces share` before it goes to a running daemon. The
/// daemon confirms every share with user presence (Touch ID or the login
/// password on this computer) and `--yes` cannot skip that. Without a
/// terminal the caller is likely a script or agent with nobody at the
/// screen, and the OS prompt would wait about two minutes before it
/// fails, so this fails at once unless `--yes` says someone will answer.
pub fn daemon_share_gate(yes: bool, terminal: bool) -> std::result::Result<(), CuaError> {
    if yes || terminal {
        return Ok(());
    }
    Err(CuaError::InvalidArgument(
        "not shared: stdin is not a terminal, and the running cua daemon confirms every share \
         with Touch ID or your password on this computer. Pass --yes (-y) when someone there \
         will confirm it; sharing through the daemon cannot skip that confirmation"
            .into(),
    ))
}

/// What the daemon asks the person at this computer for, in this OS's words
/// (it was always "Touch ID", also on a Linux server that has none).
pub fn share_confirm_prompt() -> &'static str {
    if cfg!(target_os = "macos") {
        "Confirm the share on this computer (Touch ID or your password)."
    } else if cfg!(target_os = "windows") {
        "Confirm the share on this computer (Windows Hello or your password)."
    } else {
        "Confirm the share on this computer (your password, asked by the desktop's polkit agent; a machine with no desktop session cannot confirm)."
    }
}

/// Confirms a share on the terminal (the `cua` CLI without a daemon).
pub struct TerminalConsent {
    /// `--yes`: confirmed up front.
    pub yes: bool,
}

impl cua_spaces::share::ShareConsent for TerminalConsent {
    fn confirm(&self, reason: &str) -> std::result::Result<(), String> {
        use std::io::{BufRead as _, IsTerminal as _};
        if self.yes {
            return Ok(());
        }
        if !std::io::stdin().is_terminal() {
            return Err("no terminal to confirm on; pass --yes".into());
        }
        eprint!("{reason}? [y/N] ");
        let mut answer = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut answer)
            .map_err(|e| e.to_string())?;
        if matches!(answer.trim(), "y" | "Y" | "yes") {
            Ok(())
        } else {
            Err("you declined".into())
        }
    }
}

fn print_shares(s: &cua_sdk::SpaceShares, json: bool, out: &mut dyn Write) {
    if json {
        let shares: Vec<serde_json::Value> = s
            .shares
            .iter()
            .map(|e| serde_json::json!({"who": e.who, "role": e.role, "connected": e.connected}))
            .collect();
        line(
            out,
            serde_json::json!({
                "space": s.space, "machine": s.machine, "invitee_space": s.invitee_space,
                "url": s.url, "online": s.online, "shares": shares,
            })
            .to_string(),
        );
        return;
    }
    if s.machine.is_empty() || s.shares.is_empty() {
        line(out, format!("{} is not shared.", s.space));
        return;
    }
    line(
        out,
        format!("{} is shared as {}:", s.space, s.invitee_space),
    );
    for e in &s.shares {
        line(
            out,
            format!(
                "  {:<32} {:<7}{}",
                e.who,
                e.role,
                if e.connected { " connected" } else { "" }
            ),
        );
    }
}

/// What `cua spaces rm` says about a Space whose sandbox still exists on
/// this machine: `rm` only forgets, so it refuses rather than orphan it.
fn rm_refused_for_local(id: &str) -> CuaError {
    CuaError::InvalidArgument(format!(
        "{id} is a Space created on this machine and its sandbox still exists: `cua spaces rm` \
         would only forget it and leave it running. Use `cua spaces delete {id}` to delete it \
         (or `cua spaces stop {id}` to keep it)"
    ))
}

/// What `cua spaces rm` says about a Space in the cloud: its sandbox is
/// billed until it is deleted, and forgetting the Space would leave it
/// running with nothing pointing at it.
fn rm_refused_for_cloud(id: &str) -> CuaError {
    CuaError::InvalidArgument(format!(
        "{id} is a Space in a cloud and its sandbox is billed until it is deleted: `cua spaces \
         rm` would only forget it and leave it running. Use `cua spaces delete {id}` to delete \
         it"
    ))
}

/// Whether `cua spaces rm` must refuse this Space: a Space whose sandbox cua
/// created and that still exists (a local one, a Cua Cloud claim, or one in
/// your own cloud, which `cloud_delete` marks).
fn rm_refusal(id: &str, cloud_delete: &str, local_sandbox_exists: bool) -> Option<CuaError> {
    if id.starts_with("cloud:") || !cloud_delete.is_empty() {
        return Some(rm_refused_for_cloud(id));
    }
    (id.starts_with("local:") && local_sandbox_exists).then(|| rm_refused_for_local(id))
}

fn space_json(s: &cua_sdk::SpaceInfo) -> serde_json::Value {
    serde_json::json!({
        "id": s.id,
        "name": s.name,
        "display_name": s.display_name,
        "provider": s.provider,
        "spacesd_version": s.spacesd_version,
        "features": s.features,
        "os": s.os,
        "services": s.services,
        "added_at": s.added_at,
        "host": s.host,
        "host_name": s.host_name,
        "power": s.power,
        "power_state": s.power_state,
        "durable": s.durable,
        "labels": s.labels,
        "expires_at_unix": s.expires_at_unix,
        "on_expire": s.on_expire,
        "created_at_unix": s.created_at_unix,
        "egress": {"allow": s.egress.allow, "deny": s.egress.deny},
        "egress_enforced": s.egress_enforced.map(|_| "host"),
        "stop_reason": s.stop_reason,
    })
}

/// The name people see: the one it was renamed to, else its name.
fn shown_name(s: &cua_sdk::SpaceInfo) -> &str {
    if s.display_name.is_empty() {
        &s.name
    } else {
        &s.display_name
    }
}

/// `--label key=value` words as the labels they set.
fn parse_labels(words: &[String]) -> Result<std::collections::HashMap<String, String>, CuaError> {
    words
        .iter()
        .map(|w| cua_spaces::registry::parse_label(w))
        .collect::<Result<_, _>>()
        .map_err(|e| match e {
            cua_spaces::Error::InvalidArgument(m) => CuaError::InvalidArgument(m),
            other => CuaError::InvalidArgument(other.to_string()),
        })
}

/// The Spaces that carry every label of `want`.
fn with_labels(
    list: &mut Vec<cua_spaces::SpaceInfo>,
    want: &std::collections::HashMap<String, String>,
) {
    list.retain(|s| want.iter().all(|(k, v)| s.labels.get(k) == Some(v)));
}

/// ` [k=v, k2=v2]` for a Space with labels, else nothing.
fn label_suffix(s: &cua_spaces::SpaceInfo) -> String {
    if s.labels.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = s.labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!(" [{}]", pairs.join(", "))
}

/// What `cua spaces stop` / `start` did: the message, or one JSON line.
/// The line that says when Spaces landed in your cloud ("3 of 16 Spaces
/// placed in your cloud: Hetzner pool · hetzner-2 (cpx11, hil) ·
/// $0.0338/h"); `None` when none did.
fn cloud_summary(placed: &[(String, String)], total: usize) -> Option<String> {
    if placed.is_empty() {
        return None;
    }
    let mut places: Vec<&str> = placed.iter().map(|(_, p)| p.as_str()).collect();
    places.sort_unstable();
    places.dedup();
    Some(format!(
        "{} of {total} Space{} placed in your cloud: {}",
        placed.len(),
        if total == 1 { "" } else { "s" },
        places.join("; ")
    ))
}

fn power_line(r: cua_sdk::SpacePowerReport, json: bool, out: &mut dyn Write) {
    if json {
        line(
            out,
            serde_json::json!({"space": r.space, "state": r.state, "power": r.power,
                "message": r.message})
            .to_string(),
        );
    } else {
        line(out, r.message);
    }
}

/// `cua spaces add` / `cua spaces rm`, through the SDK handle (the daemon
/// when one runs, else embedded), so they share the registry the daemon's
/// Spaces runtime and MCP use.
pub async fn run_spaces_sdk(
    cua: &Arc<cua_sdk::Cua>,
    cmd: SpacesCmd,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let spaces = cua.spaces();
    match cmd {
        SpacesCmd::Add {
            url,
            token,
            name,
            service,
            host: _,
        } => {
            let info = spaces
                .add_with_service(url, token.filter(|t| !t.is_empty()), name, service)
                .await?;
            if json {
                line(out, space_json(&info).to_string());
            } else {
                line(
                    out,
                    format!(
                        "Added {} ({}, {}{})",
                        info.id,
                        info.name,
                        info.provider,
                        if info.spacesd_version.is_empty() {
                            ", no cua-spacesd".to_string()
                        } else {
                            format!(", cua-spacesd {}", info.spacesd_version)
                        }
                    ),
                );
            }
        }
        SpacesCmd::Rm { space } => {
            let info = spaces.resolve(space.clone()).await?;
            // A Space cua created owns a sandbox (billed, for a cloud one):
            // forgetting it would leave that sandbox running with nothing
            // pointing at it.
            let local_exists = match info.id.strip_prefix("local:") {
                Some(name) => cua.sandboxes().get(name.to_string()).await.is_ok(),
                None => false,
            };
            if let Some(e) = rm_refusal(&info.id, &info.cloud_delete, local_exists) {
                return Err(e);
            }
            spaces.remove(space).await?;
            if json {
                line(
                    out,
                    serde_json::json!({ "removed": space_json(&info) }).to_string(),
                );
            } else {
                line(out, format!("Removed {} ({})", info.id, info.name));
            }
        }
        SpacesCmd::Rename { space, name } => {
            let info = spaces.rename(space.clone(), name.clone()).await?;
            if json {
                line(
                    out,
                    serde_json::json!({ "renamed": space_json(&info) }).to_string(),
                );
            } else {
                line(out, format!("Renamed {} to {}", info.id, shown_name(&info)));
            }
        }
        SpacesCmd::Share {
            space, who, role, ..
        } => {
            let who = team_who(&who).await?;
            if who.starts_with("team:") && !json {
                eprintln!(
                    "Adding {space} to the team… (right after a Teams checkout this can take up to a minute while the plan is set up)"
                );
            }
            let s = spaces.space(space).await?;
            print_shares(&s.share(who, Some(role)).await?, json, out);
        }
        SpacesCmd::Unshare { space, who, all } => {
            let s = spaces.space(space).await?;
            let who = match who {
                Some(w) if !all => Some(team_who(&w).await?),
                _ => None,
            };
            print_shares(&s.unshare(who).await?, json, out);
        }
        SpacesCmd::Shares { space } => {
            let s = spaces.space(space).await?;
            print_shares(&s.shares().await?, json, out);
        }
        SpacesCmd::RelayRegister { space } => {
            let r = spaces.relay_register(space).await?;
            if json {
                line(
                    out,
                    serde_json::json!({"space": r.space, "relay_space": r.relay_space,
                        "machine": r.machine, "url": r.url, "online": r.online})
                    .to_string(),
                );
            } else {
                line(
                    out,
                    format!(
                        "{} is on the relay as {}{}",
                        r.space,
                        r.relay_space,
                        if r.online { "" } else { " (not connected yet)" }
                    ),
                );
            }
        }
        SpacesCmd::RelayUnregister { space } => {
            let gone = spaces.relay_unregister(space.clone()).await?;
            if json {
                line(
                    out,
                    serde_json::json!({"space": space, "unregistered": gone}).to_string(),
                );
            } else if gone {
                line(out, format!("{space} is off the relay."));
            } else {
                line(out, format!("{space} was not on the relay."));
            }
        }
        SpacesCmd::Stop { space } => power_line(spaces.stop(space).await?, json, out),
        SpacesCmd::Suspend { space } => power_line(spaces.suspend(space).await?, json, out),
        SpacesCmd::Resize {
            space,
            cpus,
            memory_mb,
            disk_gb,
        } => {
            let r = spaces.resize(space, cpus, memory_mb, disk_gb).await?;
            if json {
                line(
                    out,
                    serde_json::json!({"space": r.space, "cpus": r.cpus,
                        "memory_mb": r.memory_mb, "disk_gb": r.disk_gb, "message": r.message})
                    .to_string(),
                );
            } else {
                line(out, r.message);
            }
        }
        SpacesCmd::Start { space } => power_line(spaces.start(space).await?, json, out),
        SpacesCmd::Prewarm {
            image,
            count,
            on,
            ttl,
            kind,
            cpus,
            memory_mb,
            instance_type,
            region,
            list,
            remove,
        } => {
            let fmt = |p: &cua_sdk::Prewarm| {
                let at: Vec<String> = p
                    .placements
                    .iter()
                    .map(|pl| format!("{} {}/{}", pl.host, pl.ready, pl.count))
                    .collect();
                format!(
                    "{}  {}  {}/{} ready  {} vCPU {} MiB  on {}  until {}",
                    p.id,
                    p.image,
                    p.ready,
                    p.count,
                    p.cpus,
                    p.memory_mb,
                    if at.is_empty() {
                        "-".into()
                    } else {
                        at.join(", ")
                    },
                    humantime::format_rfc3339_seconds(
                        std::time::UNIX_EPOCH + std::time::Duration::from_secs(p.expires_at)
                    )
                )
            };
            let row = |p: &cua_sdk::Prewarm| {
                serde_json::json!({"id": p.id, "image": p.image, "kind": p.kind, "cpus": p.cpus,
                    "memory_mb": p.memory_mb, "count": p.count, "ready": p.ready,
                    "expires_at": p.expires_at, "new_hosts": p.new_hosts,
                    "placements": p.placements.iter().map(|pl| serde_json::json!({
                        "host": pl.host, "machine": pl.machine, "count": pl.count, "ready": pl.ready})).collect::<Vec<_>>()})
            };
            if list {
                let all = spaces.prewarms().await?;
                if json {
                    line(
                        out,
                        serde_json::Value::Array(all.iter().map(row).collect()).to_string(),
                    );
                } else if all.is_empty() {
                    line(out, "No prewarms.");
                } else {
                    for p in &all {
                        line(out, fmt(p));
                    }
                }
            } else if let Some(id) = remove {
                let removed = spaces.remove_prewarm(id.clone()).await?;
                if json {
                    line(out, serde_json::json!({"removed": removed}).to_string());
                } else if removed {
                    line(out, format!("Ended {id}: its ready Spaces go."));
                } else {
                    line(out, format!("No prewarm {id}."));
                }
            } else {
                let ttl_secs = ttl
                    .map(|t| {
                        humantime::parse_duration(t.trim())
                            .map(|d| d.as_secs())
                            .map_err(|e| CuaError::InvalidArgument(format!("--ttl {t}: {e}")))
                    })
                    .transpose()?;
                let p = spaces
                    .prewarm(
                        image.unwrap_or_default(),
                        cua_sdk::PrewarmOptions {
                            count: Some(count),
                            on,
                            kind,
                            cpus,
                            memory_mb,
                            ttl_secs,
                            instance_type,
                            region,
                        },
                    )
                    .await?;
                if json {
                    line(out, row(&p).to_string());
                } else {
                    line(out, fmt(&p));
                }
            }
        }
        SpacesCmd::Egress {
            space,
            deny,
            allow,
            clear,
        } => {
            if deny.is_empty() && allow.is_empty() && !clear {
                return Err(CuaError::InvalidArgument(
                    "name what to block (--deny) or keep reachable (--allow), or --clear to \
                     remove the policy"
                        .into(),
                ));
            }
            let policy = cua_sdk::EgressPolicy { allow, deny };
            spaces.set_egress(space.clone(), policy.clone()).await?;
            if json {
                line(
                    out,
                    serde_json::json!({"space": space,
                        "egress": {"allow": policy.allow, "deny": policy.deny},
                        "egress_enforced": (!clear).then_some("host")})
                    .to_string(),
                );
            } else if clear {
                line(out, format!("Removed the egress policy of {space}."));
            } else {
                line(
                    out,
                    format!(
                        "Set the egress policy of {space}: {} blocked, {} kept reachable \
                         (enforced by its host, outside the guest).",
                        policy.deny.len(),
                        policy.allow.len()
                    ),
                );
            }
        }
        SpacesCmd::Cancel { space } => {
            let o = spaces.cancel_create(space).await?;
            if json {
                line(
                    out,
                    serde_json::json!({"id": o.id, "state": o.state, "message": o.message})
                        .to_string(),
                );
            } else {
                line(out, o.message);
            }
        }
        SpacesCmd::Gpus { on } => {
            let all = spaces.gpu_support(on).await?;
            if json {
                let rows: Vec<serde_json::Value> = all
                    .iter()
                    .map(|g| {
                        serde_json::json!({
                            "runtime": g.runtime,
                            "reason": g.reason,
                            "options": g.options.iter().map(|o| serde_json::json!({
                                "id": o.id, "label": o.label, "experimental": o.experimental,
                                "supported": o.supported, "reason": o.reason,
                                "learn_more": o.learn_more, "usd_per_hour": o.usd_per_hour,
                            })).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                line(out, serde_json::json!({ "runtimes": rows }).to_string());
            } else {
                for g in &all {
                    if g.options.is_empty() {
                        line(out, format!("{}: none ({})", g.runtime, g.reason));
                    }
                    for o in &g.options {
                        let state = if o.supported {
                            "available".to_string()
                        } else {
                            format!("unavailable: {}", o.reason)
                        };
                        line(
                            out,
                            format!(
                                "{}: {} ({}{}), {state}",
                                g.runtime,
                                o.id,
                                o.label,
                                if o.experimental { ", experimental" } else { "" }
                            ),
                        );
                    }
                }
            }
        }
        SpacesCmd::Create {
            image,
            on,
            count,
            name,
            kind,
            runtime,
            cpus,
            memory_mb,
            disk_gb,
            gpu,
            durable,
            forkable,
            instance_type,
            region,
            volumes,
            label,
            egress_deny,
            egress_allow,
        } => {
            let volumes = parse_volumes(&volumes)?;
            let labels = parse_labels(&label)?;
            let egress = (!egress_deny.is_empty() || !egress_allow.is_empty()).then(|| {
                cua_sdk::EgressPolicy {
                    allow: egress_allow.clone(),
                    deny: egress_deny.clone(),
                }
            });
            if !(1..=8).contains(&count) {
                return Err(CuaError::InvalidArgument(format!(
                    "--count {count}: create 1 to 8 Spaces at a time"
                )));
            }
            // A plain word that names no location is one of your machines.
            let on = on
                .map(|o| {
                    cua_sandbox_core::placement::On::parse_or_host(&o)
                        .map(|on| on.to_string())
                        .map_err(|e| CuaError::InvalidPlacement(e.message))
                })
                .transpose()?;
            // Each create has its own key, so Ctrl-C can cancel it.
            let run = std::process::id();
            let one = |n: u32, name: Option<String>| cua_sdk::SpaceCreateOptions {
                image: image.clone(),
                on: on.clone(),
                kind: kind.clone(),
                runtime: runtime.clone(),
                name,
                cpus,
                memory_mb,
                disk_gb,
                gpu: gpu.clone(),
                create_id: Some(format!("cli:{run}:{n}")),
                durable,
                forkable: forkable.then_some(true),
                instance_type: instance_type.clone(),
                region: region.clone(),
                volumes: volumes.clone(),
                labels: labels.clone(),
                egress: egress.clone(),
                ..Default::default()
            };
            // Durable Spaces are kept by the coordinator of your machines,
            // which the cua daemon of Cua Spaces runs: go through it.
            let spaces = if durable && cua.mode() == cua_sdk::CuaMode::Embedded {
                cua_daemon::client::ensure_daemon()
                    .await
                    .map_err(|e| CuaError::Internal(format!("the cua daemon: {e}")))?;
                cua_sdk::Cua::connect(None, None)?.spaces()
            } else {
                spaces
            };
            let names: Vec<Option<String>> = (1..=count)
                .map(|n| {
                    name.as_ref().map(|b| {
                        if n == 1 {
                            b.clone()
                        } else {
                            format!("{b}-{n}")
                        }
                    })
                })
                .collect();
            use std::io::IsTerminal as _;
            // What each create is doing, on one updating line of the
            // terminal (never in --json output).
            let show = !json && std::io::stderr().is_terminal() && count == 1;
            let steps = Arc::new(crate::util::StepLines::new(
                crate::util::CreateWords::for_image(
                    image.as_deref().unwrap_or_default(),
                    kind.as_deref().unwrap_or_default(),
                    true,
                ),
            ));
            let creates = names.into_iter().enumerate().map(|(i, n)| {
                let options = one(i as u32 + 1, n);
                let spaces = spaces.clone();
                let steps = steps.clone();
                async move {
                    if show {
                        Box::pin(
                            spaces.create_with_progress(options, Arc::new(StepListener(steps))),
                        )
                        .await
                    } else {
                        Box::pin(spaces.create(options)).await
                    }
                }
            });
            let mut all = Box::pin(futures_util::future::join_all(creates));
            let results = tokio::select! {
                r = &mut all => r,
                _ = tokio::signal::ctrl_c() => {
                    // Ctrl-C: stop every create and remove what it made.
                    eprintln!("\nCancelling...");
                    let mut messages = Vec::new();
                    for n in 1..=count {
                        let o = spaces.cancel_create(format!("cli:{run}:{n}")).await?;
                        if o.state != "not_creating" {
                            messages.push(o.message);
                        }
                    }
                    // The creates end with `Cancelled` once cleaned up.
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(120), all).await;
                    let message = if messages.is_empty() {
                        "Cancelled.".to_string()
                    } else {
                        messages.join(" ")
                    };
                    return Err(CuaError::Cancelled(message));
                }
            };
            crate::util::exit_on_ctrl_c();
            if show {
                steps.finish();
            }
            let mut failed = 0;
            let mut first_failure = 1;
            let mut rows = Vec::new();
            let mut in_cloud: Vec<(String, String)> = Vec::new();
            for r in results {
                match r {
                    Ok(created) => {
                        let Some(info) = created.space else {
                            continue;
                        };
                        if !info.cloud.is_empty() {
                            in_cloud.push((info.cloud.clone(), info.cloud_place.clone()));
                        }
                        if !json {
                            line(
                                out,
                                format!(
                                    "Created {} ({}{})",
                                    info.id,
                                    info.name,
                                    if info.host_name.is_empty() {
                                        String::new()
                                    } else {
                                        format!(", on {}", info.host_name)
                                    }
                                ),
                            );
                        }
                        rows.push(space_json(&info));
                    }
                    Err(e) => {
                        failed += 1;
                        if failed == 1 {
                            first_failure = crate::exit_code(&e);
                        }
                        if !json {
                            eprintln!("cua: {e}");
                        }
                        // `code` is the SDK error's name (`HostCapacity`),
                        // for a script that branches on it.
                        rows.push(serde_json::json!({
                            "error": e.to_string(),
                            "code": e.variant(),
                        }));
                    }
                }
            }
            if json {
                line(out, serde_json::json!({ "spaces": rows }).to_string());
            } else if let Some(line_) = cloud_summary(&in_cloud, count as usize) {
                line(out, line_);
            }
            if failed == count as usize {
                // The same exit code `cua` gives that error elsewhere (3 not
                // found, 8 no room on the machine).
                return Ok(first_failure);
            }
        }
        SpacesCmd::Delete { space, force } => {
            use crate::sandbox::{DeleteConfirmation, confirm_delete};
            use std::io::IsTerminal as _;
            match confirm_delete(force, std::io::stdin().is_terminal(), || {
                crate::util::confirm(&format!("Delete {space} and everything in it?"), false)
            }) {
                DeleteConfirmation::Proceed => {}
                DeleteConfirmation::Declined => {
                    line(out, "Aborted.");
                    return Ok(0);
                }
                DeleteConfirmation::NeedsForce => {
                    return Err(crate::sandbox::needs_force(&format!("delete {space}")));
                }
            }
            // A durable Space goes for good, through the daemon that keeps
            // it (deleting only its current copy would have it created
            // again).
            let durable = durable_spaces().await.into_iter().find(|d| {
                d["name"].as_str() == Some(space.as_str())
                    || d["space"].as_str() == Some(space.as_str())
            });
            let message = match durable.as_ref().and_then(|d| d["name"].as_str()) {
                Some(name) if cua.mode() == cua_sdk::CuaMode::Embedded => {
                    crate::runs::request(
                        "pool",
                        "durable_delete",
                        serde_json::json!({"name": name}),
                    )
                    .await?;
                    format!("Deleted the durable Space {name} (it is not created again).")
                }
                _ => spaces.delete(space.clone()).await?,
            };
            if json {
                line(
                    out,
                    serde_json::json!({"deleted": space, "message": message}).to_string(),
                );
            } else {
                line(out, message);
            }
        }
        SpacesCmd::Ls { .. } => {
            return Err(CuaError::Internal(
                "`cua spaces ls` runs over the registry directly".into(),
            ));
        }
    }
    Ok(0)
}

/// Account tokens for `cua host setup`: the `cua auth login` session when
/// this machine has one; otherwise a sign-in held in memory for this command
/// only and never stored, so a dedicated host keeps no account session (just
/// its machine token).
pub struct SetupTokens {
    stored: Arc<dyn AccountTokens>,
    login: Box<dyn Fn() -> LoginFuture + Send + Sync>,
    cached: tokio::sync::Mutex<Option<String>>,
    ephemeral: std::sync::atomic::AtomicBool,
}

/// A sign-in returning an access token.
pub type LoginFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = cua_host::Result<String>> + Send>>;

impl SetupTokens {
    /// `stored` first, else `login` (once per command).
    pub fn new(
        stored: Arc<dyn AccountTokens>,
        login: impl Fn() -> LoginFuture + Send + Sync + 'static,
    ) -> Self {
        Self {
            stored,
            login: Box::new(login),
            cached: tokio::sync::Mutex::new(None),
            ephemeral: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// The stored session, else an interactive sign-in that is not saved.
    pub fn session_or_sign_in(flow: cua_auth::Flow) -> Self {
        Self::new(Arc::new(SessionTokens), move || {
            Box::pin(sign_in_for_setup(flow))
        })
    }

    /// Whether the sign-in was held in memory only.
    pub fn used_ephemeral(&self) -> bool {
        self.ephemeral.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl AccountTokens for SetupTokens {
    async fn access_token(&self) -> cua_host::Result<String> {
        let mut cached = self.cached.lock().await;
        if let Some(t) = cached.as_ref() {
            return Ok(t.clone());
        }
        match self.stored.access_token().await {
            Ok(t) => Ok(t),
            Err(cua_host::Error::Unauthenticated(_)) => {
                let t = (self.login)().await?;
                self.ephemeral
                    .store(true, std::sync::atomic::Ordering::SeqCst);
                *cached = Some(t.clone());
                Ok(t)
            }
            Err(e) => Err(e),
        }
    }

    async fn signed_in(&self) -> bool {
        self.cached.lock().await.is_some() || self.stored.signed_in().await
    }
}

/// Signs in for `cua host setup` without storing the session.
async fn sign_in_for_setup(flow: cua_auth::Flow) -> cua_host::Result<String> {
    let unauth = |e: cua_auth::Error| cua_host::Error::Unauthenticated(e.to_string());
    let mut pending = auth::session().begin_login(flow).await.map_err(unauth)?;
    pending.return_to = Some(cua_auth::ReturnTo::Terminal);
    eprintln!("Sign in to cua.ai to register this computer (the session is not stored here):");
    eprintln!("  {}", pending.url);
    if let Some(code) = &pending.user_code {
        eprintln!("Enter this code if prompted: {code}");
    }
    let creds = pending.complete().await.map_err(unauth)?;
    Ok(creds.access_token)
}

/// The `cua auth login` session as account tokens (refreshed on use).
pub struct SessionTokens;

#[async_trait::async_trait]
impl AccountTokens for SessionTokens {
    async fn access_token(&self) -> cua_host::Result<String> {
        match auth::session_token(&auth::Store::from_env()).await {
            Ok(Some(t)) => Ok(t),
            Ok(None) => Err(cua_host::Error::Unauthenticated(
                "not signed in to cua.ai; run `cua auth login` first".into(),
            )),
            Err(e) => Err(cua_host::Error::Unauthenticated(e.to_string())),
        }
    }

    async fn signed_in(&self) -> bool {
        !matches!(auth::session().credentials(), Ok(None))
    }
}

/// `cua machines rm`: removes a computer that is gone (its relay row, then
/// the one signed-in device named like it). The caller folds `--yes` into
/// `confirm`.
pub async fn remove_machine(
    cua: &cua_sdk::Cua,
    auth: &DeviceAuth,
    tokens: &dyn AccountTokens,
    name: &str,
    confirm: &dyn Fn(&str) -> bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    use cua_host::DeviceState;
    // The row is removed through `cua`, the lookup and the device through
    // `auth`: they must be one relay (a daemon keeps its own).
    let bound = cua
        .embedded_runtime()
        .and_then(|r| r.spaces().relay_account())
        .map(|a| a.url);
    let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
    if !bound.is_some_and(|u| same(&u, auth.relay_url())) {
        return Err(CuaError::InvalidArgument(format!(
            "this runtime is not bound to the relay {} that your account uses; run it with --embedded",
            auth.relay_url()
        )));
    }
    // Fresh from the relay: the Space resolver reads a possibly cold cache.
    let relay = RelayClient::new(auth.relay_url()).map_err(host_err)?;
    let token = tokens.access_token().await.map_err(host_err)?;
    let machines = auth.machines_for(&relay, &token).await.map_err(host_err)?;
    let mut hits: Vec<_> = machines
        .iter()
        .filter(|m| m.id == name || format!("relay:{}", m.id) == name || m.name == name)
        .collect();
    hits.sort_by(|a, b| a.id.cmp(&b.id));
    let m = match hits.as_slice() {
        [] => {
            return Err(CuaError::NotFound(format!(
                "no computer named {name:?} in your relay directory (`cua spaces ls` lists them)"
            )));
        }
        [m] => *m,
        many => {
            let ids: Vec<_> = many.iter().map(|m| format!("relay:{}", m.id)).collect();
            return Err(CuaError::InvalidArgument(format!(
                "several computers are named {name:?}: pass one of {}",
                ids.join(", ")
            )));
        }
    };
    let rid = format!("relay:{}", m.id);
    if m.role != "owner" {
        return Err(CuaError::PermissionDenied(format!(
            "{rid} is shared with you; only its owner can remove it"
        )));
    }
    let hosted = m.host.as_deref().is_some_and(|h| !h.is_empty());
    let cloud = m
        .meta
        .get(cua_sandbox_core::byoc::meta::PROVIDER)
        .is_some_and(|p| !p.is_empty());
    let registered = !hosted
        && !m.meta.contains_key(cua_host::META_PROVIDES_SPACES)
        && m.id.starts_with("space-");
    if hosted || cloud || registered {
        return Err(CuaError::InvalidArgument(format!(
            "{rid} is a Space, not a computer; manage it with `cua spaces`"
        )));
    }
    if m.online {
        return Err(CuaError::InvalidArgument(format!(
            "{rid} ({}) is connected now; run `cua host remove` on that computer",
            m.name
        )));
    }
    if !confirm(&format!(
        "Remove {} ({rid}) from your relay directory and revoke the signed-in device named {}?",
        m.name, m.name
    )) {
        return Err(CuaError::PermissionDenied(
            "not removed (pass --yes to remove without a prompt)".into(),
        ));
    }
    // Refuses again if the computer connected meanwhile.
    let removed = cua.spaces().relay_unregister(rid.clone()).await?;
    let report = |out: &mut dyn Write, status: &str, ids: &[String], text: String| {
        if json {
            let v = serde_json::json!({
                "machine": { "id": rid, "name": m.name },
                "removed": removed,
                "device": { "status": status, "ids": ids, "message": text },
            });
            line(out, v.to_string());
        } else {
            if removed {
                line(
                    out,
                    format!("Removed {} ({rid}) from your relay directory.", m.name),
                );
            } else {
                line(
                    out,
                    format!("{rid} was already gone from your relay directory."),
                );
            }
            line(out, text);
        }
    };
    if !removed {
        report(out, "not_attempted", &[], "No device was touched.".into());
        return Ok(0);
    }
    let this = auth.device_id().ok().flatten();
    let found = auth.devices().await.map(|all| {
        let mut ids: Vec<_> = all
            .into_iter()
            .filter(|d| {
                !m.name.is_empty()
                    && d.name == m.name
                    && !d.current
                    && Some(&d.id) != this.as_ref()
                    && d.state != DeviceState::Revoked
            })
            .map(|d| d.id)
            .collect();
        ids.sort();
        ids
    });
    let fail = |out: &mut dyn Write, e: cua_host::Error| {
        report(
            out,
            "failed",
            &[],
            "Finish with `cua devices ls` and `cua devices revoke <id>`.".into(),
        );
        host_err(e)
    };
    let ids = match found {
        Ok(ids) => ids,
        Err(e) => return Err(fail(out, e)),
    };
    match ids.as_slice() {
        [] => report(
            out,
            "not_found",
            &[],
            format!(
                "No signed-in device named {}; no device was revoked.",
                m.name
            ),
        ),
        [id] => match crate::devices_cmd::revoke_device(auth, id).await {
            Ok(d) => report(
                out,
                "revoked",
                &ids,
                format!("Revoked {} ({}).", d.name, d.id),
            ),
            Err(e) => {
                report(
                    out,
                    "failed",
                    &ids,
                    "Finish with `cua devices ls` and `cua devices revoke <id>`.".into(),
                );
                return Err(e);
            }
        },
        many => report(
            out,
            "ambiguous",
            many,
            format!(
                "Several devices are named {} ({}): revoke the right one with `cua devices revoke <id>`.",
                m.name,
                many.join(", ")
            ),
        ),
    }
    Ok(0)
}

pub fn host_err(e: cua_host::Error) -> CuaError {
    let message = e.to_string();
    host_error(e, message)
}

fn host_error(e: cua_host::Error, m: String) -> CuaError {
    use cua_host::Error as E;
    match e {
        E::InvalidArgument(_) | E::Conflict(_) => CuaError::InvalidArgument(m),
        E::Unauthenticated(_) => CuaError::Unauthenticated(m),
        E::PermissionDenied(_) => CuaError::PermissionDenied(m),
        E::NotFound(_) => CuaError::NotFound(m),
        E::Relay(_) | E::Download(_) | E::Throttled { .. } => CuaError::Http(m),
        // The plain sentence, then where to upgrade (`?from=cli_limit`).
        E::PlanLimit(_) => {
            let url = cua_home::plan_limit::PlanLimit::parse(&m)
                .map(|l| l.upgrade_url_from(&cua_home::env::account_app_url(), "cli_limit"));
            CuaError::Http(match url {
                Some(url) => format!("{m} Upgrade: {url}"),
                None => m,
            })
        }
        E::Service(_) => CuaError::Runtime(m),
        E::Io(_) | E::Internal(_) => CuaError::Internal(m),
    }
}

fn render(s: &HostStatus, out: &mut dyn Write) {
    if !s.configured {
        hint_line(
            out,
            "This computer is not set up for access (run `cua host setup`).",
        );
        return;
    }
    let name = s.name.clone().unwrap_or_default();
    if s.paused_signed_out {
        hint_line(
            out,
            "Relay sharing is paused: the Cua app on this computer is signed out. \
             Sign in to it again (or run `cua host start`) to share it.",
        );
    }
    match s.mode.as_deref() {
        Some("relay") => line(
            out,
            format!(
                "{name}: relay {} as computer {}",
                s.relay_url.clone().unwrap_or_default(),
                s.machine_id.clone().unwrap_or_default()
            ),
        ),
        _ => line(
            out,
            format!(
                "{name}: direct {} (env token in {})",
                s.direct_url.clone().unwrap_or_default(),
                s.env_token_path.clone().unwrap_or_default()
            ),
        ),
    }
    line(
        out,
        format!(
            "  runner: {} {}{}",
            s.service.kind,
            if s.service.running {
                "running"
            } else if s.service.installed {
                "installed, not running"
            } else {
                "not installed"
            },
            if s.service.detail.is_empty() {
                String::new()
            } else {
                format!(" ({})", s.service.detail)
            }
        ),
    );
    if s.service.kind == "process" {
        hint_line(out, cua_host::service::PROCESS_RUNNER_NOTE);
    }
    if s.service.kind == "app-helper" && s.legacy_service {
        hint_line(
            out,
            "  the old LaunchAgent (com.trycua.spacesd.host) is still there: run `cua host migrate` to remove it",
        );
    } else if s.service.kind == "launchd" && s.helper_available {
        hint_line(
            out,
            "  tip: `cua host migrate` moves this service into the Cua Spaces app (its login item)",
        );
    }
    line(
        out,
        format!(
            "  sharing: {}{}",
            if s.sharing { "on" } else { "off" },
            match s.online {
                Some(true) => ", online",
                Some(false) => ", offline",
                None => "",
            }
        ),
    );
    render_settings(s, out);
    if !s.allow.is_empty() {
        line(out, format!("  also allowed: {}", s.allow.join(", ")));
    }
    if s.clients.is_empty() {
        if s.mode.as_deref() == Some("relay") {
            line(out, "  connected: nobody");
        }
    } else {
        for c in &s.clients {
            let who = c.name.clone().or(c.email.clone()).unwrap_or(c.id.clone());
            line(out, format!("  connected: {who} ({} stream(s))", c.streams));
        }
    }
    if let Some(e) = &s.error {
        line(out, format!("  relay: {e}"));
    }
    render_access(s, now_ms(), out);
    render_provided(s, now_ms(), out);
    if !s.permissions.is_empty() && s.share_desktop {
        line(
            out,
            "  macOS permissions (grant them yourself; Cua never changes them):",
        );
        for p in &s.permissions {
            line(out, format!("    - {}: {}", p.title, p.instructions));
            line(out, format!("      open {}", p.settings_url));
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn ago(now_ms: u64, at_ms: u64) -> String {
    let s = now_ms.saturating_sub(at_ms) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

/// Whether this machine's desktop is shared, as `cua host setup` and
/// `cua host status` say it.
fn desktop_line(share_desktop: bool) -> String {
    format!(
        "  desktop: {}",
        if share_desktop {
            "shared (this computer is a Space)"
        } else {
            "not shared (its screen, input, shell and files are off limits)"
        }
    )
}

/// The two settings: the desktop, and Spaces for your other devices.
fn render_settings(s: &HostStatus, out: &mut dyn Write) {
    line(out, desktop_line(s.share_desktop));
    if s.provide_spaces {
        let limit = |n: u32| {
            if n == 0 {
                "no limit".to_string()
            } else {
                format!("at most {n}")
            }
        };
        line(
            out,
            format!(
                "  provides Spaces: on ({} now, {}{})",
                s.provided_spaces.len(),
                limit(s.max_spaces),
                if cfg!(target_os = "macos") {
                    format!("; macOS VMs {}", limit(s.max_macos_vms))
                } else {
                    String::new()
                }
            ),
        );
    } else {
        line(out, "  provides Spaces: off");
    }
}

/// The Spaces this machine provides and the audit of remote creates and
/// deletes.
fn render_provided(s: &HostStatus, now_ms: u64, out: &mut dyn Write) {
    for p in &s.provided_spaces {
        if p.is_direct() {
            line(
                out,
                format!(
                    "    {} on port {} ({}, {}) for {}",
                    p.local_space, p.direct_port, p.image, p.kind, p.created_by
                ),
            );
            continue;
        }
        line(
            out,
            format!(
                "    relay:{} {} ({}, {}) for {}",
                p.relay_machine, p.local_space, p.image, p.kind, p.created_by
            ),
        );
    }
    if let Some(e) = &s.spaces_audit_error {
        line(
            out,
            format!("  spaces audit: does not verify ({e}); it may have been altered"),
        );
    }
    if s.spaces_audit.is_empty() {
        return;
    }
    line(out, "  spaces audit:");
    for r in s.spaces_audit.iter().take(8) {
        line(
            out,
            format!(
                "    {:<9} {} {} {}{}",
                ago(now_ms, r.at_ms),
                r.who,
                r.action,
                r.space,
                if r.detail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", r.detail)
                }
            ),
        );
    }
}

/// Who reached this machine recently (the driver's access log).
fn render_access(s: &HostStatus, now_ms: u64, out: &mut dyn Write) {
    if let Some(e) = &s.access_log_error {
        line(
            out,
            format!("  access log: does not verify ({e}); it may have been altered"),
        );
    }
    // Background probes and the owner's own thumbnails stay in the log
    // (and `--json`), not in this list.
    use cua_host::access::AccessKind;
    let shown: Vec<_> = s
        .recent_access
        .iter()
        .filter(|r| {
            !matches!(
                r.kind(),
                AccessKind::Background | AccessKind::OwnerThumbnail
            )
        })
        .take(8)
        .collect();
    if shown.is_empty() {
        return;
    }
    line(out, "  recent access:");
    for r in shown {
        line(
            out,
            format!(
                "    {:<9} {} via {}: {}",
                ago(now_ms, r.at_ms),
                r.who,
                r.via,
                r.what
            ),
        );
    }
}

fn emit(s: &HostStatus, json: bool, out: &mut dyn Write) -> Result<(), CuaError> {
    if json {
        line(
            out,
            serde_json::to_string(s).map_err(|e| CuaError::Internal(e.to_string()))?,
        );
    } else {
        render(s, out);
    }
    Ok(())
}

/// Runs a host command against `home` with `tokens` (`manager` overrides the
/// OS service runner; tests pass a fake).
pub async fn run_host(
    cmd: HostCmd,
    home: &Path,
    tokens: &dyn AccountTokens,
    device: Option<&DeviceAuth>,
    manager: Option<Arc<dyn ServiceManager>>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    run_host_to(
        cmd,
        home,
        tokens,
        device,
        manager,
        json,
        out,
        &mut std::io::stderr(),
        RunnerKind::detect(),
    )
    .await
}

/// [`run_host`] with an explicit stderr stream (warnings under `--json`) and
/// the runner `auto` detects on this machine.
#[allow(clippy::too_many_arguments)]
pub async fn run_host_to(
    cmd: HostCmd,
    home: &Path,
    tokens: &dyn AccountTokens,
    device: Option<&DeviceAuth>,
    manager: Option<Arc<dyn ServiceManager>>,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
    detected: RunnerKind,
) -> Result<i32, CuaError> {
    let mut host = Host::new(home);
    if let Some(m) = manager {
        host = host.with_service_manager(m);
    }
    match cmd {
        HostCmd::Setup {
            relay,
            pair,
            remote: _, // Selected by the command's SetupTokens.
            direct,
            name,
            allow,
            driver_bin,
            runner,
            share,
            profile,
            desktop,
            no_desktop,
            provide_spaces,
            no_provide_spaces,
            max_spaces,
            allow_any_address,
            tailnet,
            dedicated,
        } => {
            let direct = match (direct, tailnet) {
                (Some(addr), _) => Some(addr),
                (None, true) => {
                    let ip = cua_host::direct::tailscale_ip().ok_or_else(|| {
                        CuaError::InvalidArgument(
                            "--tailnet: no tailnet address on this machine (is Tailscale, or a \
                             Headscale-coordinated client, up?)"
                                .into(),
                        )
                    })?;
                    Some(cua_host::direct::authority(
                        ip,
                        cua_home::channel::CuaChannel::current().ports().tcp,
                    ))
                }
                (None, false) => None,
            };
            // A pasted add-machine link names its relay (`?relay=`) when the
            // code came from another relay than the default.
            let relay = relay.or_else(|| pair.as_deref().and_then(cua_host::pairing_link_relay));
            let mut opts = match direct {
                Some(addr) => SetupOptions::direct(addr.parse().map_err(|_| {
                    CuaError::InvalidArgument(format!("--direct {addr:?} is not ip:port"))
                })?),
                None => SetupOptions::relay(relay.unwrap_or_else(cua_host::relay_url_from_env)),
            };
            opts.tailnet = tailnet;
            // The code is single use: install the driver first, so a failed
            // download does not spend it.
            opts.download_first = pair.is_some();
            opts.name = name;
            opts.allow = allow;
            opts.driver_bin = driver_bin;
            opts.runner = RunnerKind::parse(&runner).map_err(host_err)?;
            let requested_runner = opts.runner;
            opts = opts.profile(
                cua_host::HostProfile::parse(profile.as_deref().unwrap_or("desktop"))
                    .map_err(host_err)?,
            );
            if desktop {
                opts.share_desktop = true;
            }
            if no_desktop {
                opts.share_desktop = false;
            }
            if provide_spaces {
                opts.provide_spaces = Some(true);
            }
            if no_provide_spaces {
                opts.provide_spaces = Some(false);
            }
            if let Some(share) = share {
                let (desktop, spaces) = share.settings();
                opts.share_desktop = desktop;
                opts.provide_spaces = Some(spaces);
            }
            opts.max_spaces = max_spaces;
            opts.allow_any_address = allow_any_address;
            opts.dedicated = dedicated;
            let spaces_only = !opts.share_desktop && host.resolve_provide_spaces(&opts);
            // Running it again changes nothing when this machine already
            // is that host; a host whose service stopped is started again
            // (no sign-in, the code is not spent).
            if let cua_host::HostMode::Relay { url } = &opts.mode
                && !opts.tailnet
                && opts.driver_bin.is_none()
                && opts.allow.is_empty()
                && opts.max_spaces.is_none()
                // `--share none` is always refused, even by a computer that
                // `host config` set to share nothing.
                && share != Some(HostShare::None)
            {
                // Only what this command line says: a re-run that leaves out
                // the profile keeps what the machine shares.
                let want = crate::onboard::Want {
                    relay: url.clone(),
                    share_desktop: (share.is_some() || profile.is_some() || desktop || no_desktop)
                        .then_some(opts.share_desktop),
                    // Providing Spaces: what the flags or a spare profile
                    // say; the desktop profile keeps the machine's own.
                    provide_spaces: opts.provide_spaces,
                    dedicated,
                    name: opts.name.clone(),
                    runner: (runner != "auto").then(|| runner.clone()),
                };
                let state = crate::onboard::HostState::read(&host).await;
                if state.is_up(&want) || state.is_down(&want) {
                    let code =
                        setup_unchanged(&host, state, &want, pair.is_some(), json, out).await?;
                    warn_setup_runner(&host, requested_runner, detected, json, out, err)?;
                    return Ok(code);
                }
            }
            // A pairing code stands in for the account: nothing to sign in.
            let paired = pair
                .as_deref()
                .map(cua_host::PairingCodeToken::parse)
                .transpose()
                .map_err(host_err)?;
            let tokens: &dyn AccountTokens = match &paired {
                Some(p) => p,
                None => tokens,
            };
            if paired.is_none()
                && let cua_host::HostMode::Relay { url } = &opts.mode
                && let Ok(device) = crate::devices_cmd::device_auth(url)
            {
                opts.device_session = device.try_session().await;
            }
            let s = host
                .setup_staged(opts, tokens)
                .await
                .map_err(|(stage, e)| {
                    let message = format!("host setup failed at {}: {e}", stage.as_str());
                    host_error(e, message)
                })?;
            emit(&s, json, out)?;
            warn_setup_runner(&host, requested_runner, detected, json, out, err)?;
            if s.provide_spaces && !json {
                line(
                    out,
                    crate::onboard::jobs_line(dedicated, &cua_cluster::sysinfo::accelerators()),
                );
            }
            // A spare Mac that only provides Spaces gives its Linux runtime
            // VM most of itself (every local Linux Space shares that VM),
            // unless the owner already sized it.
            if spaces_only
                && cfg!(target_os = "macos")
                && let Ok(Some((cpus, mem))) =
                    cua_vmm::managed::dedicate_to_spaces(&cua_sandbox_core::settings::config_path())
            {
                let note = crate::util::hint(&format!(
                    "Linux Spaces here share a {cpus}-CPU, {mem} GiB runtime VM \
                     (cua config set runtime.linux_memory_gib N to change it)."
                ));
                if json {
                    eprintln!("{note}");
                } else {
                    line(out, note);
                }
            }
            // macOS Spaces here need Local Network access, which only this
            // Mac's own screen can grant: say exactly what to allow.
            if let Ok(Some(config)) = host.config()
                && let Some(hint) = cua_host::local_network_hint(
                    std::env::consts::OS,
                    s.provide_spaces,
                    &config.driver_bin,
                )
            {
                let l = format!("Local Network: {}", hint.instructions);
                if json {
                    eprintln!("{l}");
                } else {
                    line(out, l);
                }
            }
            if allow_any_address && s.provide_spaces {
                eprintln!(
                    "warning: --allow-any-address: host calls are accepted from any address; {}.",
                    cua_host::direct::PLAINTEXT_WARNING
                );
            }
            // Direct mode: how a laptop adds this machine (with the token,
            // so to stderr when the output is JSON for a program).
            if let Some(pair) = host.pairing_command().map_err(host_err)? {
                let lines = [
                    if tailnet {
                        "On a device of yours or of a teammate you allowed (on this tailnet), add \
                         this computer:"
                            .to_string()
                    } else {
                        "On your laptop, add this computer (the token is a shell on this computer: \
                         keep it private):"
                            .to_string()
                    },
                    format!("  {pair}"),
                ];
                for l in lines {
                    if json {
                        eprintln!("{l}");
                    } else {
                        line(out, l);
                    }
                }
            }
        }
        HostCmd::Pair {
            team,
            no_wait,
            name,
            expires,
            list,
            revoke,
            relay,
        } => {
            // Only the stored `cua auth login` session: making a code is
            // no reason to start a sign-in for "this machine".
            pair_codes(
                home,
                &SessionTokens,
                device,
                relay,
                team,
                no_wait,
                name,
                expires,
                list,
                revoke,
                json,
                out,
            )
            .await
            .map_err(crate::team_cmd::err)?;
        }
        HostCmd::Migrate { force } => {
            let m = host
                .migrate_service(force, env!("CARGO_PKG_VERSION"))
                .await
                .map_err(host_err)?;
            if json {
                writeln!(
                    out,
                    "{}",
                    serde_json::to_string_pretty(&m)
                        .map_err(|e| CuaError::Internal(e.to_string()))?
                )
                .map_err(|e| CuaError::Internal(e.to_string()))?;
            } else {
                line(out, format!("{}: {}", m.outcome, m.detail));
                if m.permissions_needed {
                    hint_line(
                        out,
                        "macOS will ask you once more for Screen Recording and Accessibility, \
                         now for Cua Spaces: allow both in System Settings > Privacy & \
                         Security (`cua host status` lists them). The old \"cua-spacesd\" \
                         entries there can be removed with the minus button.",
                    );
                }
            }
            return Ok(match m.outcome.as_str() {
                "rolled-back" | "kept" => 1,
                _ => 0,
            });
        }
        HostCmd::Status => {
            let s = host.status().await.map_err(host_err)?;
            // This Mac's processor, memory and disk, with or without setup.
            let resources = crate::resources_view::fetch(
                &cua_spaces::Spaces::builder().home(home).build(),
                true,
            )
            .await;
            if json {
                let status =
                    serde_json::to_value(&s).map_err(|e| CuaError::Internal(e.to_string()))?;
                line(
                    out,
                    crate::resources_view::status_json(status, &resources).to_string(),
                );
            } else {
                render(&s, out);
                for l in crate::resources_view::this_mac_lines(&resources) {
                    line(out, l);
                }
            }
            if !s.configured {
                return Ok(1);
            }
        }
        HostCmd::Config {
            desktop,
            provide_spaces,
            max_spaces,
            max_macos_vms,
        } => {
            let on = |v: Option<String>| v.map(|v| v == "on");
            let change = cua_host::HostSettingsChange {
                share_desktop: on(desktop),
                provide_spaces: on(provide_spaces),
                max_spaces,
                max_macos_vms,
                cua_bin: None,
            };
            let s = if change == cua_host::HostSettingsChange::default() {
                host.status().await.map_err(host_err)?
            } else {
                host.configure(change).await.map_err(host_err)?
            };
            if json {
                emit(&s, true, out)?;
            } else if !s.configured {
                render(&s, out);
                return Ok(1);
            } else {
                render_settings(&s, out);
                render_provided(&s, now_ms(), out);
            }
        }
        HostCmd::Stop => emit(&host.stop_sharing().await.map_err(host_err)?, json, out)?,
        HostCmd::Start => emit(&host.start_sharing().await.map_err(host_err)?, json, out)?,
        HostCmd::Share {
            who,
            machine,
            relay,
            yes,
        } => {
            // Sharing widens who reaches the machine: ask.
            if !yes
                && !(crate::util::interactive()
                    && crate::util::confirm(
                        &format!("Let {} reach this computer?", who.join(", ")),
                        false,
                    ))
            {
                return Err(CuaError::PermissionDenied(
                    "not shared (pass --yes to share without a prompt)".into(),
                ));
            }
            let allow = change_allow(&host, home, tokens, device, machine, relay, &who, true)
                .await
                .map_err(host_err)?;
            print_allow(&allow, json, out);
        }
        HostCmd::Unshare {
            who,
            machine,
            relay,
        } => {
            let allow = change_allow(&host, home, tokens, device, machine, relay, &who, false)
                .await
                .map_err(host_err)?;
            print_allow(&allow, json, out);
        }
        HostCmd::Remove {
            force,
            uninstalling,
        } => {
            use crate::sandbox::{DeleteConfirmation, confirm_delete};
            use std::io::IsTerminal as _;
            match confirm_delete(force, std::io::stdin().is_terminal(), || {
                crate::util::confirm(
                    "Remove this computer's access setup (unregister it and delete its tokens)?",
                    false,
                )
            }) {
                DeleteConfirmation::Proceed => {}
                DeleteConfirmation::Declined => {
                    line(out, "Aborted.");
                    return Ok(0);
                }
                DeleteConfirmation::NeedsForce => {
                    return Err(crate::sandbox::needs_force(
                        "remove this computer's access setup",
                    ));
                }
            }
            if uninstalling {
                host.remove_for_uninstall().await.map_err(host_err)?;
            } else {
                host.remove().await.map_err(host_err)?;
            }
            if json {
                line(out, r#"{"removed":true}"#);
            } else {
                line(out, "This computer is no longer set up for access.");
            }
        }
    }
    Ok(0)
}

/// Warns after `cua host setup --runner auto` fell back to a plain process:
/// to `out`, or to `err` with `--json` so stdout stays JSON. Reads the
/// stored runner (the resolved name), not the live service kind.
fn warn_setup_runner(
    host: &Host,
    requested: RunnerKind,
    detected: RunnerKind,
    json: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<(), CuaError> {
    // Warn only when auto detection here really finds no systemd: a process
    // runner kept from an explicit choice (or another OS) is not a fallback.
    if requested != RunnerKind::Auto || detected != RunnerKind::Process {
        return Ok(());
    }
    let Some(config) = host.config().map_err(host_err)? else {
        return Ok(());
    };
    if let Some(text) = cua_host::service::auto_fallback_warning(requested, &config.runner) {
        hint_line(if json { err } else { out }, text);
    }
    Ok(())
}

/// `cua host setup` on a machine that already is the host asked for: says
/// so (with the accelerators found), or starts its stopped service again.
async fn setup_unchanged(
    host: &Host,
    state: crate::onboard::HostState,
    want: &crate::onboard::Want,
    paired: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let started = !state.is_up(want);
    let state = if started {
        let status = host.start_sharing().await.map_err(host_err)?;
        crate::onboard::HostState {
            status: serde_json::to_value(status).unwrap_or_default(),
            dedicated: state.dedicated,
        }
    } else {
        state
    };
    if json {
        line(
            out,
            serde_json::json!({"changed": started, "started": started, "host": state.status})
                .to_string(),
        );
        return Ok(0);
    }
    if started {
        line(
            out,
            format!(
                "Started the host service again: {} is back on {}.",
                state.status["name"].as_str().unwrap_or("this computer"),
                state.status["relayUrl"].as_str().unwrap_or("the relay"),
            ),
        );
    } else {
        line(out, state.summary());
    }
    line(
        out,
        desktop_line(state.status["shareDesktop"].as_bool() == Some(true)),
    );
    if paired {
        line(
            out,
            "The pairing code was not needed (this computer has joined already); if it was not used before, it still works on another computer.",
        );
    }
    if state.status["provideSpaces"].as_bool() == Some(true) {
        line(
            out,
            crate::onboard::jobs_line(state.dedicated, &cua_cluster::sysinfo::accelerators()),
        );
    }
    Ok(0)
}

/// How long an uninstall waits for one Space to turn off.
const UNINSTALL_STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(45);

/// `cua host remove --uninstalling`: turns off (keeping its data) each Space
/// this machine provides, so nothing of this machine stays reachable
/// through the relay. Best effort, each reported on stderr.
pub async fn turn_off_provided_spaces<F, Fut>(home: &Path, open: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Arc<cua_sdk::Cua>, CuaError>>,
{
    let provided = Host::new(home).provided_spaces().unwrap_or_default();
    if provided.is_empty() {
        return;
    }
    let cua = match open().await {
        Ok(cua) => cua,
        Err(e) => {
            eprintln!("Could not turn off the Spaces this computer provides: {e}");
            return;
        }
    };
    for space in provided {
        let id = space.local_space;
        match tokio::time::timeout(UNINSTALL_STOP_WAIT, cua.spaces().stop(id.clone())).await {
            Ok(Ok(_)) => eprintln!("Turned off {id}."),
            Ok(Err(e)) => eprintln!("Could not turn off {id}: {e}"),
            Err(_) => eprintln!("Timed out turning off {id}."),
        }
    }
}

fn print_allow(allow: &[String], json: bool, out: &mut dyn Write) {
    if json {
        line(out, serde_json::json!({ "allow": allow }).to_string());
    } else if allow.is_empty() {
        line(out, "Shared with: nobody besides the owner.");
    } else {
        line(out, format!("Shared with: {}", allow.join(", ")));
    }
}

/// `who` as it is shared with: a `team:<name>` becomes `team:<organization
/// id>` (the Spaces API resolves names and aliases); anything else as is.
async fn team_who(who: &str) -> Result<String, CuaError> {
    match who.trim().strip_prefix("team:") {
        Some(t)
            if !cua_host::spaces_api::is_org_ref(t)
                && !cua_host::spaces_api::is_legacy_team_id(t) =>
        {
            Ok(format!(
                "team:{}",
                crate::team_cmd::resolve_team(&crate::util::cua_home(), t).await?
            ))
        }
        _ => Ok(who.trim().to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
async fn pair_codes(
    home: &Path,
    tokens: &dyn AccountTokens,
    device: Option<&DeviceAuth>,
    relay: Option<String>,
    team: Option<String>,
    no_wait: bool,
    name: Option<String>,
    expires: u64,
    list: bool,
    revoke: Option<String>,
    json: bool,
    out: &mut dyn Write,
) -> cua_host::Result<()> {
    let url = relay_url(relay.clone(), home);
    // The relay the other machine is told to join: --relay, else a custom
    // `$CUA_RELAY_URL` (CUA_ENV, passed along, picks the rest).
    let relay_arg = crate::onboard::relay_for_line(
        relay.as_deref(),
        std::env::var("CUA_RELAY_URL").ok().as_deref(),
    );
    // The device session is per relay: with --relay, this device's
    // enrollment there.
    let other = match relay {
        Some(_) => crate::devices_cmd::device_auth(&url).ok(),
        None => None,
    };
    let device = other.as_deref().or(device);
    let session = match device {
        Some(d) => d.try_session().await,
        None => None,
    };
    let client = RelayClient::new(&url)?.with_device_session(session);
    let token = tokens.access_token().await?;
    if let Some(id) = revoke {
        client.revoke_pairing_code(&token, &id).await?;
        if json {
            line(out, serde_json::json!({ "revoked": id }).to_string());
        } else {
            line(
                out,
                format!("Revoked {id}: it can no longer add a computer."),
            );
        }
        return Ok(());
    }
    if list {
        let codes = client.pairing_codes(&token).await?;
        if json {
            line(out, serde_json::json!({ "codes": codes }).to_string());
        } else if codes.is_empty() {
            line(out, "No pairing codes.");
        } else {
            for c in codes {
                let what = match (&c.state[..], &c.machine) {
                    ("used", Some(m)) => format!("used by {m}"),
                    (state, _) => state.to_string(),
                };
                let team = c.team.map(|t| format!(" team {t}")).unwrap_or_default();
                line(out, format!("{}  {what}{team}", c.id));
            }
        }
        return Ok(());
    }
    if !(1..=1440).contains(&expires) {
        return Err(cua_host::Error::InvalidArgument(
            "--expires is 1 to 1440 minutes".into(),
        ));
    }
    // A team is an organization of the Spaces API: the relay code carries
    // no team (the relay's own teams are retired), and the machine becomes
    // a team machine once it joins (below).
    let api = cua_host::spaces_api::SpacesApiClient::current()?;
    let org = match &team {
        Some(t) => {
            let org = api.resolve_org(&token, t).await?;
            if !org.manages() {
                return Err(cua_host::Error::PermissionDenied(format!(
                    "only an owner or admin of {} can add computers to it",
                    org.label()
                )));
            }
            if !org.has("team_machines") {
                eprintln!(
                    "Note: {} has no team computers on its plan yet (buy Teams at app.cua.ai); a plan bought a moment ago is waited for.",
                    org.label()
                );
            }
            Some(org)
        }
        None => None,
    };
    let minted_name = name.clone();
    let minted = client
        .create_pairing_code(
            &token,
            &cua_host::PairingCodeRequest {
                name,
                team: None,
                ttl_secs: Some(expires * 60),
            },
        )
        .await?;
    let code = minted
        .code
        .clone()
        .filter(|c| cua_host::normalize_pairing_code(c).is_some())
        .ok_or_else(|| cua_host::Error::Relay("the relay returned no pairing code".into()))?;
    let link = cua_host::add_machine_url(&code, &url);
    // The other machine joins as a host that provides Spaces (so jobs and
    // Spaces can run there), on this relay when it is not the default one.
    let shown = cua_host::display_pairing_code(&code);
    let name_arg = minted_name.as_deref();
    let setup = format!(
        "cua {}",
        crate::onboard::pair_setup_args(&shown, name_arg, relay_arg.as_deref())
    );
    let env = crate::onboard::env_prefix();
    let sh = crate::onboard::add_line(
        &shown,
        &crate::onboard::InstallSource::Release,
        &env,
        relay_arg.as_deref(),
        name_arg,
    );
    // `CUA_INSTALL_URL`: a staging build of the Linux `cua` and
    // `cua-spacesd` instead of the release.
    let staging = match crate::onboard::InstallSource::from_env()
        .map_err(|e| cua_host::Error::InvalidArgument(e.to_string()))?
    {
        src @ crate::onboard::InstallSource::Tarball { .. } => Some(crate::onboard::add_line(
            &shown,
            &src,
            &env,
            relay_arg.as_deref(),
            name_arg,
        )),
        crate::onboard::InstallSource::Release => None,
    };
    let ps = cua_host::install_command(&code, &url, "windows");
    if json {
        line(
            out,
            serde_json::json!({
                "id": minted.id, "code": code, "url": link, "expires_at": minted.expires_at,
                "team": org.as_ref().map(|o| o.id.clone()),
                "install": {"sh": sh, "powershell": ps, "setup": setup, "staging": staging},
            })
            .to_string(),
        );
    } else {
        let mut lines = vec![
            format!(
                "Code {code} (works once, expires in {expires} min; id {})",
                minted.id
            ),
            format!("Open on the other computer: {link}"),
            "Or run there:".to_string(),
            format!("  cua installed:  {env}{setup}"),
            format!("  macOS / Linux:  {sh}"),
        ];
        if let Some(st) = &staging {
            lines.push(format!("  Staging build (Linux):  {st}"));
        }
        lines.push(format!("  Windows:        {ps}"));
        lines.push(crate::util::hint(
            "It joins your pool as a host that provides Spaces; see it with `cua computers`.",
        ));
        for l in lines {
            line(out, l);
        }
    }
    let Some(org) = org else {
        return Ok(());
    };
    let later = crate::util::hint(&format!(
        "cua team add-machine {} relay:<computer id>",
        org.id
    ));
    if no_wait {
        if !json {
            line(
                out,
                format!("When it has joined, add it to {}: {later}", org.label()),
            );
        }
        return Ok(());
    }
    if !json {
        line(
            out,
            format!(
                "Waiting for the computer to join, to add it to {} (Ctrl-C to stop; then: {later})…",
                org.label()
            ),
        );
    }
    let mut said = false;
    let machine = client
        .add_paired_machine_to_team(
            &api,
            &token,
            &minted.id,
            &org.id,
            minted.expires_at.max(1),
            std::time::Duration::from_secs(3),
            |_| {
                if !said && !json {
                    eprintln!(
                        "{} (the team's plan is on its way)",
                        cua_host::spaces_api::FINISHING_SETUP
                    );
                    said = true;
                }
            },
        )
        .await?;
    if json {
        line(
            out,
            serde_json::json!({ "team": org.id, "machine": machine }).to_string(),
        );
    } else {
        line(
            out,
            format!(
                "relay:{machine} joined and is a computer of {}.",
                org.label()
            ),
        );
    }
    Ok(())
}

/// Adds (`add`) or removes `who` on the allowlist of `machine` (default:
/// this machine) through the relay, as the account from this device, and
/// mirrors it into this machine's driver policy when it is the target.
#[allow(clippy::too_many_arguments)]
async fn change_allow(
    host: &Host,
    home: &Path,
    tokens: &dyn AccountTokens,
    device: Option<&DeviceAuth>,
    machine: Option<String>,
    relay: Option<String>,
    who: &[String],
    add: bool,
) -> cua_host::Result<Vec<String>> {
    let config = host.config()?;
    let local_id = config.as_ref().and_then(|c| c.machine_id.clone());
    let id = machine.or(local_id.clone()).ok_or_else(|| {
        cua_host::Error::InvalidArgument(
            "this computer is not on a relay; pass --machine <id> (`cua spaces ls`)".into(),
        )
    })?;
    let url = relay
        .or_else(|| config.as_ref().and_then(|c| c.relay_url.clone()))
        .unwrap_or_else(|| relay_url(None, home));
    let session = match device {
        Some(d) => d.try_session().await,
        None => None,
    };
    let client = RelayClient::new(&url)?.with_device_session(session);
    let token = tokens.access_token().await?;
    let current = client.machine(&token, &id).await?;
    let mut allow = current.allow.clone();
    for w in who {
        let w = w.trim().to_string();
        if add {
            if !allow.iter().any(|a| a.eq_ignore_ascii_case(&w)) {
                allow.push(w);
            }
        } else {
            allow.retain(|a| !a.eq_ignore_ascii_case(&w));
        }
    }
    let updated = client
        .patch(
            &token,
            &id,
            &MachinePatch {
                allow: Some(allow),
                ..Default::default()
            },
        )
        .await?;
    if local_id.as_deref() == Some(id.as_str()) {
        host.sync_policy_allow(&updated.allow)?;
    }
    Ok(updated.allow)
}

/// The relay account `cua daemon` serves Spaces with: the `cua auth login`
/// session (read per call, so signing in later works without a restart) on
/// the configured relay, and this device's enrollment.
pub fn daemon_relay_account(home: &Path) -> cua_spaces::RelayAccount {
    let url = relay_url(None, home);
    let account = cua_spaces::RelayAccount::new(url.clone(), Arc::new(SessionTokens));
    match crate::devices_cmd::device_auth(&url) {
        Ok(device) => account.with_device(device),
        Err(_) => account,
    }
}

/// Gives an embedded `cua` (no daemon running, or `--embedded`) the
/// account's relay machines, as `cua daemon` does for its own runtime:
/// without it `cua mcp` lists no relay machines and refuses `relay:<id>`
/// Spaces. A daemon-backed `cua` is left alone (the daemon lists them), as
/// is a runtime that already has an account. `CUA_DAEMON_NO_RELAY=1` turns
/// it off here too. Returns whether an account was attached.
pub fn attach_relay_account(
    cua: &cua_sdk::Cua,
    account: impl FnOnce() -> cua_spaces::RelayAccount,
) -> bool {
    let Some(runtime) = cua.embedded_runtime() else {
        return false;
    };
    if std::env::var_os("CUA_DAEMON_NO_RELAY").is_some()
        || runtime.spaces().relay_account().is_some()
    {
        return false;
    }
    runtime.spaces().set_relay(Some(account()));
    true
}

/// `--relay`, else `CUA_RELAY_URL`, else the relay this machine is set up
/// with, else the default relay.
pub fn relay_url(flag: Option<String>, home: &Path) -> String {
    cua_host::relay_url_for(flag.as_deref(), home)
}

/// How loaded one of your machines that provide Spaces is, for `cua spaces
/// ls`: "3 running · 40% CPU · 55% memory", or why it takes nothing now.
pub fn load_text(h: &cua_spaces::host_spaces::HostOffer) -> String {
    if !h.online {
        return String::new();
    }
    let Some(n) = &h.node else {
        return "provides Spaces".into();
    };
    if !n.problem.is_empty() {
        return format!("takes nothing now: {}", n.problem);
    }
    let pct = |free: u64, total: u64| {
        (100 * total.saturating_sub(free))
            .checked_div(total)
            .unwrap_or(0)
    };
    format!(
        "{} running · {}% CPU · {}% memory",
        n.running.values().sum::<u32>(),
        pct(u64::from(n.millicpus_free), u64::from(n.millicpus_total)),
        pct(n.memory_free_mb, n.memory_total_mb)
    )
}

/// A Space's name as the apps show it: the name it was renamed to, else
/// the name typed at create (`local:<name>`, `cloud:<name>`), not the
/// container hostname (12 hex
/// digits) its cua-spacesd reported. The same rule as the app core's
/// `spaces::name_of`, kept here because this crate is MIT and the app core
/// is not.
fn display_name(s: &cua_spaces::SpaceInfo) -> String {
    // Renamed (`cua spaces rename`): the name it was given.
    if !s.display_name.is_empty() {
        return s.display_name.clone();
    }
    let container_hostname = s.name.len() == 12
        && s.name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let from_id =
        s.id.strip_prefix("local:")
            .or_else(|| s.id.strip_prefix("cloud:"))
            .filter(|rest| !rest.is_empty());
    match from_id {
        Some(from_id) if s.name.is_empty() || container_hostname => from_id.to_string(),
        _ if s.name.is_empty() => s.id.clone(),
        _ => s.name.clone(),
    }
}

/// `cua spaces ls --json`: each Space as the registry has it, plus
/// `display_name` (the name the list and the apps show).
/// `--volume NAME[/SUBFOLDER]:PATH[:ro|:rw]` arguments as volume mounts.
pub(crate) fn parse_volumes(args: &[String]) -> Result<Vec<cua_sdk::VolumeMount>, CuaError> {
    let mut out = Vec::new();
    for a in args {
        let bad = |why: &str| {
            CuaError::InvalidArgument(format!(
                "--volume {a:?}: {why}; write it as NAME:PATH[:ro] (for example shared:/mnt/shared:ro)"
            ))
        };
        let parts: Vec<&str> = a.split(':').collect();
        if !(2..=3).contains(&parts.len()) {
            return Err(bad("expected a name and a path"));
        }
        let read_only = match parts.get(2).copied() {
            None | Some("rw") => false,
            Some("ro") => true,
            Some(other) => return Err(bad(&format!("{other:?} is not ro or rw"))),
        };
        let (name, subpath) = match parts[0].split_once('/') {
            Some((n, s)) => (n, s),
            None => (parts[0], ""),
        };
        out.push(cua_sdk::VolumeMount {
            name: name.to_string(),
            path: parts[1].to_string(),
            read_only,
            subpath: subpath.to_string(),
        });
    }
    let core: Vec<cua_spaces::named_volumes::VolumeMount> =
        out.iter().cloned().map(Into::into).collect();
    cua_spaces::named_volumes::validate(&core)
        .map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
    Ok(out)
}

fn ls_json(list: &[cua_spaces::SpaceInfo]) -> Vec<serde_json::Value> {
    list.iter()
        .map(|s| {
            let mut v = serde_json::to_value(s).unwrap_or_default();
            if let Some(o) = v.as_object_mut() {
                o.insert("display_name".into(), display_name(s).into());
            }
            v
        })
        .collect()
}

/// `cua spaces ls` rows: every Space (its id, then its name as the apps
/// show it), with the Spaces a machine provides indented under it (or,
/// when that machine is not listed, marked with its name), and how loaded
/// each of your computers is (`load`, by machine). This computer's own
/// row says "(this computer)".
/// [`grouped_lines_with`] for the tests of `resources_view`.
#[cfg(test)]
pub(crate) fn grouped_lines_for_tests(
    list: &[cua_spaces::SpaceInfo],
    columns: Option<&crate::resources_view::Columns<'_>>,
) -> Vec<String> {
    grouped_lines_with(list, columns, &Default::default())
}

#[cfg(test)]
fn grouped_lines(
    list: &[cua_spaces::SpaceInfo],
    load: &std::collections::HashMap<String, String>,
) -> Vec<String> {
    grouped_lines_with(list, None, load)
}

/// [`grouped_lines`] with the CPU, MEMORY and DISK ON THIS MAC columns (and
/// their header) when `columns` is given; their `own_machine` (this
/// install's relay machine id) is the row tagged "(this computer)".
fn grouped_lines_with(
    list: &[cua_spaces::SpaceInfo],
    columns: Option<&crate::resources_view::Columns<'_>>,
    load: &std::collections::HashMap<String, String>,
) -> Vec<String> {
    use crate::resources_view::{CPU_WIDTH, MEMORY_WIDTH};
    let this_computer = columns.and_then(|c| c.own_machine.as_deref());
    let row = |s: &cua_spaces::SpaceInfo, indent: &str| {
        let key = s.id.strip_prefix("relay:").unwrap_or(&s.name);
        let s_load = load.get(key).filter(|l| !l.is_empty());
        let width = 48usize.saturating_sub(indent.len());
        let line = match columns {
            None => format!(
                "{indent}{:<width$} {:<24} {:<7} {}",
                s.id,
                display_name(s),
                s.provider.as_str(),
                s.spacesd_version,
            ),
            Some(c) => {
                let cells = c.cells(s);
                format!(
                    "{indent}{:<width$} {:<24} {:<8} {:<9} {:<CPU_WIDTH$} {:<MEMORY_WIDTH$} {}",
                    s.id,
                    display_name(s),
                    s.provider.as_str(),
                    s.spacesd_version,
                    cells.cpu,
                    cells.memory,
                    cells.disk,
                )
            }
        };
        // The computer this runs on says so, not a Space on it.
        let line = if s.host.is_empty()
            && this_computer.is_some_and(|m| s.id.strip_prefix("relay:") == Some(m))
        {
            format!("{} (this computer)", line.trim_end())
        } else {
            line
        };
        // A durable Space says so (it moves when its machine goes away).
        let line = if s.durable.is_empty() {
            line
        } else {
            format!("{} (durable)", line.trim_end())
        };
        let line = format!("{}{}", line.trim_end(), label_suffix(s));
        // A Space turned off says so (`cua spaces start` turns it on), as
        // does a machine the relay cannot reach now (offline, not sharing).
        let line = match (s.power_state.as_str(), s.status.as_str()) {
            // One that stopped on its own says why ("stopped: out of memory").
            ("stopped", _) if !s.stop_reason.is_empty() => {
                format!("{} (stopped: {})", line.trim_end(), s.stop_reason)
            }
            ("suspended" | "stopped", _) => format!("{} ({})", line.trim_end(), s.power_state),
            (_, status) if !status.is_empty() => format!("{} ({status})", line.trim_end()),
            _ => line,
        };
        match s_load {
            Some(l) => format!("{} · {l}", line.trim_end()),
            None => line,
        }
    };
    let mut out = Vec::new();
    if columns.is_some() && !list.is_empty() {
        out.push(crate::resources_view::header(48));
    }
    for s in list.iter().filter(|s| s.host.is_empty()) {
        // A Space in your cloud names its cloud ("AWS · us-west-2").
        if s.host_name.is_empty() {
            out.push(row(s, ""));
        } else {
            out.push(format!("{} (on {})", row(s, ""), s.host_name));
        }
        // A relay host's Spaces name its machine id; a direct host's name
        // its Space id.
        let machine = s.id.strip_prefix("relay:").unwrap_or("");
        for c in list.iter().filter(|c| {
            !c.host.is_empty() && ((!machine.is_empty() && c.host == machine) || c.host == s.id)
        }) {
            out.push(row(c, "  "));
        }
    }
    for c in list.iter().filter(|c| {
        !c.host.is_empty()
            && !list
                .iter()
                .any(|s| s.id == format!("relay:{}", c.host) || s.id == c.host)
    }) {
        out.push(format!(
            "{} (on {})",
            row(c, ""),
            if c.host_name.is_empty() {
                &c.host
            } else {
                &c.host_name
            }
        ));
    }
    out
}

/// `cua spaces add <addr> --host`: registers a machine that provides
/// Spaces over its direct address, in this cua home's registry (the one the
/// daemon and `cua mcp` use).
pub async fn run_add_host(
    cmd: SpacesCmd,
    home: &Path,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let SpacesCmd::Add {
        url, token, name, ..
    } = cmd
    else {
        return Err(CuaError::Internal(
            "only `cua spaces add --host` adds a host".into(),
        ));
    };
    let spaces = cua_spaces::Spaces::builder().home(home).build();
    let info = spaces
        .add_direct_host(&url, token.filter(|t| !t.is_empty()), name)
        .await
        .map_err(CuaError::from)?;
    let host = spaces
        .registry()
        .direct_hosts()
        .map_err(CuaError::from)?
        .into_iter()
        .find(|h| h.space == info.id)
        .map(|h| h.name)
        .unwrap_or_else(|| info.name.clone());
    if json {
        line(
            out,
            serde_json::json!({ "host": host, "space": info.id, "via": "direct" }).to_string(),
        );
    } else {
        // The command is spelled with this build's CLI name here, not rewritten
        // afterwards: the host's name is the user's, and may read "cua host".
        line(
            out,
            format!(
                "Added host {host:?} ({}, direct). Create Spaces on it with `{} spaces create --on \"host:{host}\"`.",
                info.id,
                crate::util::cli()
            ),
        );
    }
    Ok(0)
}

/// How long `cua spaces ls` waits for the daemon's power answer.
const LS_DAEMON_POWER_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// The registry alone can't say how a local Space turns off and on: that
/// takes the daemon's local runtime. When a daemon runs, its list fills in
/// `power` and `power_state` for the rows that lack them, so `--json`
/// readers (the Electron app) offer Stop and Start and show a suspended
/// Space as off. No daemon, or no answer in time: the rows stay as they are.
async fn power_from_daemon(list: &mut [cua_spaces::SpaceInfo]) {
    if !list.iter().any(|s| s.power.is_empty()) || cua_daemon::client::live_address().is_none() {
        return;
    }
    let answer = tokio::time::timeout(LS_DAEMON_POWER_WAIT, async {
        let client = cua_daemon::client::existing_daemon().await.ok()?;
        let r = client
            .spaces()
            .list_spaces(cua_proto::daemon::v1::ListSpacesRequest {
                // Only the power of this machine's rows is read here.
                known_now: true,
            })
            .await
            .ok()?;
        Some(r.into_inner().spaces)
    })
    .await;
    let Ok(Some(records)) = answer else {
        return;
    };
    fill_power(
        list,
        records
            .iter()
            .map(|r| (r.id.as_str(), r.power.as_str(), r.power_state.as_str())),
    );
}

/// Copies `power` and `power_state` onto the rows that lack them, by id.
fn fill_power<'a>(
    list: &mut [cua_spaces::SpaceInfo],
    from: impl Iterator<Item = (&'a str, &'a str, &'a str)>,
) {
    for (id, power, state) in from {
        if power.is_empty() {
            continue;
        }
        if let Some(s) = list.iter_mut().find(|s| s.id == id && s.power.is_empty()) {
            s.power = power.to_string();
            s.power_state = state.to_string();
        }
    }
}

/// `cua spaces ls`.
pub async fn run_spaces(
    cmd: SpacesCmd,
    home: &Path,
    tokens: Option<Arc<dyn AccountTokens>>,
    device: Option<Arc<DeviceAuth>>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let SpacesCmd::Ls {
        relay,
        label,
        no_size,
    } = cmd
    else {
        return Err(CuaError::Internal(
            "only `cua spaces ls` runs over the registry directly".into(),
        ));
    };
    let mut b = cua_spaces::Spaces::builder().home(home);
    if let Some(t) = tokens {
        let mut account = cua_spaces::RelayAccount::new(relay_url(relay, home), t);
        if let Some(d) = device {
            account = account.with_device(d);
        }
        b = b.relay(account);
    }
    let spaces = b.build();
    let mut relay_error = None;
    if spaces.relay_account().is_some()
        && let Err(e) = spaces.relay_machines().await
    {
        relay_error = Some(e.to_string());
    }
    let mut list = spaces
        .list()
        .map_err(|e| CuaError::Internal(e.to_string()))?;
    with_labels(&mut list, &parse_labels(&label)?);
    power_from_daemon(&mut list).await;
    // Durable Spaces, from the coordinator of your machines (through the
    // daemon, when one runs): marked, and the ones waiting for a machine.
    let durable = durable_spaces().await;
    for d in &durable {
        if let Some(s) = list
            .iter_mut()
            .find(|s| Some(s.id.as_str()) == d["space"].as_str())
        {
            s.durable = d["name"].as_str().unwrap_or_default().to_string();
            // Shown by its durable name, whichever copy it is now.
            s.name = s.durable.clone();
        }
    }
    // Your machines that provide Spaces, with how loaded they are (each asked
    // at once, bounded).
    let hosts = tokio::time::timeout(std::time::Duration::from_secs(8), spaces.hosts())
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let resources = crate::resources_view::fetch(&spaces, !no_size).await;
    let columns = crate::resources_view::Columns {
        resources: &resources,
        own_machine: spaces.this_relay_machine_id(),
    };
    if json {
        let mut rows = ls_json(&list);
        for (row, info) in rows.iter_mut().zip(&list) {
            columns.annotate(info, row);
        }
        let mut body = serde_json::json!({ "spaces": rows, "machines": hosts, "durable": durable,
                                          "relay_error": relay_error });
        if let (Some(o), Some(extra)) = (
            body.as_object_mut(),
            crate::resources_view::ls_extras(&resources)
                .as_object()
                .cloned(),
        ) {
            o.extend(extra);
        }
        line(out, body.to_string());
        return Ok(0);
    }
    let load: std::collections::HashMap<String, String> =
        hosts.iter().map(|h| (h.id.clone(), load_text(h))).collect();
    if list.is_empty() {
        line(out, "No Spaces.");
    }
    for l in grouped_lines_with(&list, Some(&columns), &load) {
        line(out, l);
    }
    for d in durable
        .iter()
        .filter(|d| d["space"].as_str().is_none_or(str::is_empty))
    {
        line(
            out,
            format!(
                "{:<48} {:<24} (durable, waiting for a machine{})",
                "-",
                d["name"].as_str().unwrap_or_default(),
                d["error"]
                    .as_str()
                    .filter(|e| !e.is_empty())
                    .map(|e| format!(": {e}"))
                    .unwrap_or_default()
            ),
        );
    }
    if let Some(e) = relay_error {
        line(out, format!("(relay computers unavailable: {e})"));
    }
    if let Some(w) = columns.warning() {
        line(out, w);
    }
    Ok(0)
}

/// The durable Spaces the coordinator of your machines keeps, when a cua
/// daemon of Cua Spaces runs (none otherwise; bounded).
async fn durable_spaces() -> Vec<serde_json::Value> {
    if cua_daemon::client::existing_daemon().await.is_err() {
        return vec![];
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::runs::request("pool", "durable_list", serde_json::Value::Null),
    )
    .await
    {
        Ok(Ok(v)) => v["spaces"].as_array().cloned().unwrap_or_default(),
        _ => vec![],
    }
}

/// Passes a Space create's progress to the terminal's step lines.
struct StepListener(Arc<crate::util::StepLines>);

impl cua_sdk::SpaceCreateListener for StepListener {
    fn on_progress(&self, p: cua_sdk::SpaceCreateProgress) {
        use cua_sandbox_core::progress::{Phase, Progress, Transfer};
        let Some(phase) = Phase::parse(&p.phase) else {
            return;
        };
        let mut out = Progress::phase(phase).detail(p.detail);
        if let Some(f) = p.fraction {
            out = out.fraction(f);
        }
        if let Some(done) = p.bytes_done {
            out = out.bytes(Transfer {
                done,
                total: p.bytes_total.unwrap_or(0),
                per_second: p.bytes_per_second,
            });
        }
        self.0.show(&out);
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn spaces_placed_in_your_cloud_are_said_once() {
        assert_eq!(cloud_summary(&[], 16), None);
        let p = "Hetzner pool · hetzner-2 (cpx11, hil) · $0.0338/h".to_string();
        let placed = vec![("hetzner".to_string(), p.clone()); 3];
        assert_eq!(
            cloud_summary(&placed, 16).unwrap(),
            format!("3 of 16 Spaces placed in your cloud: {p}")
        );
        assert_eq!(
            cloud_summary(&placed[..1], 1).unwrap(),
            format!("1 of 1 Space placed in your cloud: {p}")
        );
    }

    use super::*;
    use cua_host::StaticToken;
    use cua_host::service::FakeServiceManager;
    use cua_host::testing::FakeRelay;

    fn parse(args: &[&str]) -> HostCmd {
        #[derive(clap::Parser)]
        struct W {
            #[command(subcommand)]
            c: HostCmd,
        }
        <W as clap::Parser>::try_parse_from(std::iter::once("host").chain(args.iter().copied()))
            .unwrap()
            .c
    }

    /// `cua spaces rm` names the command that deletes a local Space.
    #[test]
    fn the_share_prompt_names_this_os_not_touch_id_everywhere() {
        let p = share_confirm_prompt();
        assert_eq!(p.contains("Touch ID"), cfg!(target_os = "macos"));
        assert!(p.starts_with("Confirm the share"));
    }

    #[test]
    fn rm_of_a_local_space_points_to_delete() {
        let e = rm_refused_for_local("local:studio").to_string();
        assert!(e.contains("cua spaces delete local:studio"), "{e}");
        assert!(e.contains("leave it running"), "{e}");
    }

    /// `cua spaces rm` refuses a Space in a cloud (billed sandbox) and
    /// forgets one whose sandbox cua did not create.
    #[test]
    fn rm_refuses_cloud_spaces_and_points_to_delete() {
        for (id, cloud_delete) in [
            ("cloud:studio", ""),
            ("relay:abcdef0123456789", "here"),
            ("relay:abcdef0123456789", "host:other-mac"),
            ("relay:abcdef0123456789", "elsewhere"),
        ] {
            let e = rm_refusal(id, cloud_delete, false).expect(id).to_string();
            assert!(e.contains(&format!("cua spaces delete {id}")), "{e}");
            assert!(e.contains("billed"), "{e}");
        }
        assert!(rm_refusal("local:studio", "", true).is_some());
        assert!(rm_refusal("local:gone", "", false).is_none());
        assert!(rm_refusal("direct:10.0.0.5:3211", "", false).is_none());
        assert!(rm_refusal("relay:abcdef0123456789", "", false).is_none());
    }

    fn parse_spaces(args: &[&str]) -> Result<SpacesCmd, clap::Error> {
        #[derive(clap::Parser)]
        struct W {
            #[command(subcommand)]
            c: SpacesCmd,
        }
        <W as clap::Parser>::try_parse_from(std::iter::once("spaces").chain(args.iter().copied()))
            .map(|w| w.c)
    }

    /// `cua host setup` (and `status`) always says whether the desktop is
    /// shared.
    #[test]
    fn setup_output_says_whether_the_desktop_is_shared() {
        let text = |share: bool| {
            let s = HostStatus {
                configured: true,
                mode: Some("relay".into()),
                share_desktop: share,
                provide_spaces: true,
                ..Default::default()
            };
            let mut out = Vec::new();
            render(&s, &mut out);
            String::from_utf8(out).unwrap()
        };
        assert!(text(true).contains("\n  desktop: shared"), "{}", text(true));
        assert!(
            text(false).contains("\n  desktop: not shared"),
            "{}",
            text(false)
        );
        assert!(desktop_line(true).starts_with("  desktop: shared"));
        assert!(desktop_line(false).starts_with("  desktop: not shared"));
    }

    #[test]
    fn spare_machines_and_spaces_on_them_parse() {
        match parse(&["setup", "--profile", "spare", "--max-spaces", "6"]) {
            HostCmd::Setup {
                profile,
                max_spaces,
                ..
            } => assert_eq!((profile.as_deref(), max_spaces), (Some("spare"), Some(6))),
            other => panic!("{other:?}"),
        }
        match parse(&["setup", "--share", "spaces"]) {
            HostCmd::Setup { share, profile, .. } => {
                assert_eq!((share, profile), (Some(HostShare::Spaces), None))
            }
            other => panic!("{other:?}"),
        }
        match parse(&["config", "--desktop", "off", "--provide-spaces", "on"]) {
            HostCmd::Config {
                desktop,
                provide_spaces,
                ..
            } => assert_eq!(
                (desktop.as_deref(), provide_spaces.as_deref()),
                (Some("off"), Some("on"))
            ),
            other => panic!("{other:?}"),
        }
        match parse_spaces(&[
            "create",
            "macos:26",
            "--on",
            "spare mac mini",
            "--count",
            "2",
        ])
        .unwrap()
        {
            SpacesCmd::Create {
                image, on, count, ..
            } => assert_eq!(
                (image.as_deref(), on.as_deref(), count),
                (Some("macos:26"), Some("spare mac mini"), 2)
            ),
            other => panic!("{other:?}"),
        }
        assert!(parse_spaces(&["delete", "relay:space-1", "--force"]).is_ok());
        // A direct host: added with --host, set up with --direct.
        match parse_spaces(&["add", "100.64.0.9:3211", "--host", "--token", "t"]).unwrap() {
            SpacesCmd::Add { host, token, .. } => {
                assert!(host);
                assert_eq!(token.as_deref(), Some("t"));
            }
            other => panic!("{other:?}"),
        }
        assert!(parse_spaces(&["add", "h:1", "--host", "--service", "x"]).is_err());
        match parse(&[
            "setup",
            "--direct",
            "0.0.0.0:3211",
            "--share",
            "spaces",
            "--allow-any-address",
        ]) {
            HostCmd::Setup {
                direct,
                allow_any_address,
                ..
            } => assert_eq!(
                (direct.as_deref(), allow_any_address),
                (Some("0.0.0.0:3211"), true)
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn egress_flags_parse_and_conflict() {
        let parse = |a: &[&str]| parse_spaces(a).unwrap();
        match parse(&[
            "egress",
            "studio",
            "--deny",
            "all",
            "--allow",
            "1.1.1.1",
            "--allow",
            "10.0.0.0/8",
        ]) {
            SpacesCmd::Egress {
                space,
                deny,
                allow,
                clear,
            } => {
                assert_eq!(space, "studio");
                assert_eq!(deny, ["all"]);
                assert_eq!(allow, ["1.1.1.1", "10.0.0.0/8"]);
                assert!(!clear);
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse(&["egress", "studio", "--clear"]),
            SpacesCmd::Egress { clear: true, .. }
        ));
        // Clearing and naming rules together is a contradiction.
        assert!(parse_spaces(&["egress", "studio", "--clear", "--deny", "all"]).is_err());
        match parse(&[
            "create",
            "linux",
            "--egress-deny",
            "all",
            "--egress-allow",
            "1.1.1.1",
        ]) {
            SpacesCmd::Create {
                egress_deny,
                egress_allow,
                ..
            } => {
                assert_eq!(egress_deny, ["all"]);
                assert_eq!(egress_allow, ["1.1.1.1"]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn label_flags_parse_filter_and_show() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let want = parse_labels(&words(&["team=research", "run=7"])).unwrap();
        assert_eq!(want["team"], "research");
        for bad in ["team", "team=", "=x"] {
            assert!(
                matches!(
                    parse_labels(&words(&[bad])),
                    Err(CuaError::InvalidArgument(_))
                ),
                "{bad}"
            );
        }
        let space = |id: &str, labels: &[(&str, &str)]| cua_spaces::SpaceInfo {
            id: id.into(),
            name: id.into(),
            display_name: String::new(),
            provider: cua_spaces::Provider::Local,
            spacesd_version: String::new(),
            stop_reason: String::new(),
            volumes: Vec::new(),
            features: vec![],
            os: String::new(),
            os_name: String::new(),
            os_pretty_name: String::new(),
            image: String::new(),
            image_digest: String::new(),
            kind: String::new(),
            arch: String::new(),
            services: vec![],
            added_at: None,
            host: String::new(),
            host_name: String::new(),
            power: String::new(),
            power_state: String::new(),
            cloud: String::new(),
            cloud_place: String::new(),
            cloud_delete: String::new(),
            status: String::new(),
            durable: String::new(),
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            expires_at_unix: 0,
            on_expire: String::new(),
            created_at_unix: 0,
            egress: Default::default(),
            egress_enforced: None,
            sharing: None,
        };
        let mut list = vec![
            space("local:a", &[("team", "research"), ("run", "7")]),
            space("local:b", &[("team", "research")]),
            space("local:c", &[]),
        ];
        with_labels(&mut list, &Default::default());
        assert_eq!(list.len(), 3, "no filter keeps every Space");
        with_labels(
            &mut list,
            &parse_labels(&words(&["team=research"])).unwrap(),
        );
        assert_eq!(
            list.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["local:a", "local:b"]
        );
        with_labels(&mut list, &want);
        assert_eq!(list.len(), 1, "every label must match");
        assert_eq!(list[0].id, "local:a");
        assert_eq!(label_suffix(&list[0]), " [run=7, team=research]");
        assert_eq!(label_suffix(&space("local:c", &[])), "");
        let rows = grouped_lines(&list, &Default::default());
        assert!(rows[0].contains("[run=7, team=research]"), "{rows:?}");
        let json = ls_json(&list);
        assert_eq!(json[0]["labels"]["run"], "7");
    }

    #[test]
    fn ls_takes_power_from_the_daemon_where_the_registry_has_none() {
        let space = |id: &str, power: &str| cua_spaces::SpaceInfo {
            id: id.into(),
            name: id.into(),
            display_name: String::new(),
            provider: cua_spaces::Provider::Local,
            spacesd_version: String::new(),
            features: vec![],
            os: String::new(),
            os_name: String::new(),
            os_pretty_name: String::new(),
            image: String::new(),
            image_digest: String::new(),
            kind: String::new(),
            arch: String::new(),
            services: vec![],
            added_at: None,
            host: String::new(),
            host_name: String::new(),
            power: power.into(),
            power_state: String::new(),
            cloud: String::new(),
            cloud_place: String::new(),
            cloud_delete: String::new(),
            status: String::new(),
            durable: String::new(),
            volumes: Vec::new(),
            labels: Default::default(),
            expires_at_unix: 0,
            on_expire: String::new(),
            created_at_unix: 0,
            egress: Default::default(),
            egress_enforced: None,
            sharing: None,
            stop_reason: String::new(),
        };
        let mut list = vec![
            space("local:box", ""),
            space("relay:mini", "stop"),
            space("local:odd", ""),
        ];
        fill_power(
            &mut list,
            [
                ("local:box", "suspend", "suspended"),
                ("relay:mini", "suspend", "running"),
                ("local:odd", "", ""),
                ("local:gone", "stop", "stopped"),
            ]
            .into_iter(),
        );
        assert_eq!(
            (list[0].power.as_str(), list[0].power_state.as_str()),
            ("suspend", "suspended")
        );
        // What the registry already knew stays.
        assert_eq!(list[1].power, "stop");
        assert_eq!(list[2].power, "");
        let json = ls_json(&list);
        assert_eq!(json[0]["power"], "suspend");
        assert_eq!(json[0]["power_state"], "suspended");
    }

    /// A relay row for the `cua spaces ls` list tests: `host` set means a
    /// Space on that machine.
    fn space(id: &str, host: &str) -> cua_spaces::SpaceInfo {
        cua_spaces::SpaceInfo {
            id: id.into(),
            name: id.into(),
            display_name: String::new(),
            provider: cua_spaces::Provider::Relay,
            spacesd_version: String::new(),
            features: vec![],
            os: String::new(),
            os_name: String::new(),
            os_pretty_name: String::new(),
            image: String::new(),
            image_digest: String::new(),
            kind: String::new(),
            arch: String::new(),
            services: vec![],
            added_at: None,
            host: host.into(),
            host_name: if host.is_empty() {
                String::new()
            } else {
                "Mac mini (spare)".into()
            },
            power: String::new(),
            power_state: String::new(),
            cloud: String::new(),
            cloud_place: String::new(),
            cloud_delete: String::new(),
            status: String::new(),
            durable: String::new(),
            volumes: Vec::new(),
            labels: Default::default(),
            expires_at_unix: 0,
            on_expire: String::new(),
            created_at_unix: 0,
            egress: Default::default(),
            egress_enforced: None,
            sharing: None,
            stop_reason: String::new(),
        }
    }

    #[test]
    fn spaces_on_a_host_list_under_it() {
        let none = std::collections::HashMap::new();
        let lines = grouped_lines(
            &[
                space("relay:space-1", "mini1234"),
                space("relay:studio99", ""),
                space("relay:mini1234", ""),
                space("relay:space-2", "gone5678"),
            ],
            &none,
        );
        assert!(lines[0].starts_with("relay:studio99"), "{lines:?}");
        assert!(lines[1].starts_with("relay:mini1234"), "{lines:?}");
        assert!(lines[2].starts_with("  relay:space-1"), "{lines:?}");
        assert!(
            lines[3].starts_with("relay:space-2") && lines[3].ends_with("(on Mac mini (spare))"),
            "{lines:?}"
        );
        // A direct host's Spaces name its Space id.
        let direct = |id: &str, host: &str| {
            let mut s = space(id, host);
            s.provider = cua_spaces::Provider::Direct;
            s
        };
        let lines = grouped_lines(
            &[
                direct("direct:100.64.0.9:41001", "direct:100.64.0.9:3211"),
                direct("direct:100.64.0.9:3211", ""),
            ],
            &none,
        );
        assert!(lines[0].starts_with("direct:100.64.0.9:3211"), "{lines:?}");
        assert!(
            lines[1].starts_with("  direct:100.64.0.9:41001"),
            "{lines:?}"
        );
        assert_eq!(lines.len(), 2, "{lines:?}");
        // A machine the relay cannot reach now says why.
        let mut stopped = space("relay:mini1234", "");
        stopped.status = "not sharing".into();
        let mut off = space("relay:studio99", "");
        off.status = "offline".into();
        let lines = grouped_lines(&[stopped, off], &none);
        assert!(lines[0].ends_with("(not sharing)"), "{lines:?}");
        assert!(lines[1].ends_with("(offline)"), "{lines:?}");
        // A machine that provides Spaces says how loaded it is.
        let load: std::collections::HashMap<String, String> = [(
            "mini1234".to_string(),
            "2 running · 40% CPU · 55% memory".to_string(),
        )]
        .into();
        let lines = grouped_lines(&[space("relay:mini1234", "")], &load);
        assert!(
            lines[0].ends_with("· 2 running · 40% CPU · 55% memory"),
            "{lines:?}"
        );
    }

    #[test]
    fn this_computers_own_row_says_so() {
        let resources = cua_spaces::resources::Resources::default();
        let lines = |list: &[cua_spaces::SpaceInfo], own: Option<&str>| {
            let columns = crate::resources_view::Columns {
                resources: &resources,
                own_machine: own.map(str::to_string),
            };
            grouped_lines_for_tests(list, Some(&columns))
        };
        let tagged = |lines: &[String]| {
            lines
                .iter()
                .filter(|l| l.contains("(this computer)"))
                .cloned()
                .collect::<Vec<_>>()
        };
        // With this computer in the list: its own row, and only that row.
        let with = [
            space("relay:space-1", "studio99"),
            space("relay:studio99", ""),
            space("relay:mini1234", ""),
        ];
        let out = lines(&with, Some("studio99"));
        let own = tagged(&out);
        assert_eq!(own.len(), 1, "{out:?}");
        assert!(own[0].starts_with("relay:studio99 "), "{out:?}");
        assert!(
            out.iter().any(|l| l.starts_with("  relay:space-1")),
            "{out:?}"
        );
        assert!(
            out.iter().any(|l| l.starts_with("relay:mini1234")),
            "{out:?}"
        );
        // A Space on another machine whose id reads like this computer's
        // is not this computer.
        let child = [
            space("relay:mini1234", ""),
            space("relay:studio99", "mini1234"),
        ];
        let out = lines(&child, Some("studio99"));
        assert!(
            out.iter().any(|l| l.starts_with("  relay:studio99")),
            "{out:?}"
        );
        assert!(tagged(&out).is_empty(), "{out:?}");
        // Without this computer in the list (or no machine id at all): the
        // rows are what they were.
        let without = [space("relay:mini1234", ""), space("local:aurora", "")];
        let out = lines(&without, Some("studio99"));
        assert!(tagged(&out).is_empty(), "{out:?}");
        assert_eq!(out, lines(&without, None));
        assert!(tagged(&lines(&with, None)).is_empty());
    }

    #[test]
    fn volume_arguments_are_name_path_and_mode() {
        let v = parse_volumes(&[
            "shared:/mnt/shared".into(),
            "data/in/raw:/data:ro".into(),
            "w:/w:rw".into(),
        ])
        .unwrap();
        assert_eq!(v[0].name, "shared");
        assert_eq!(v[0].path, "/mnt/shared");
        assert!(!v[0].read_only);
        assert_eq!(
            (v[1].name.as_str(), v[1].subpath.as_str()),
            ("data", "in/raw")
        );
        assert!(v[1].read_only);
        assert!(!v[2].read_only);
        for bad in [
            "shared",
            "shared:rel",
            "Shared:/mnt/x",
            "a:/b:wx",
            "a:/b:ro:x",
            "a:/mnt/a",
            "",
        ] {
            let r = parse_volumes(&[bad.into()]);
            if bad == "a:/mnt/a" {
                assert!(r.is_ok());
            } else {
                assert!(r.is_err(), "{bad}");
            }
        }
        // Overlapping paths are refused by name.
        let e = parse_volumes(&["a:/mnt".into(), "b:/mnt/b".into()])
            .unwrap_err()
            .to_string();
        assert!(e.contains("inside"), "{e}");
    }

    /// A Space created from the app lists by the name typed there, not the
    /// container hostname its cua-spacesd reported; the id stays first and
    /// `--json` keeps both.
    #[test]
    fn spaces_list_by_the_name_the_user_typed() {
        let mut s = cua_spaces::SpaceInfo {
            id: "local:e2e-1005-cli".into(),
            name: "327b8dc70418".into(),
            display_name: String::new(),
            provider: cua_spaces::Provider::Local,
            spacesd_version: "0.5.3".into(),
            features: vec![],
            os: "linux".into(),
            os_name: String::new(),
            os_pretty_name: String::new(),
            image: String::new(),
            image_digest: String::new(),
            kind: String::new(),
            arch: String::new(),
            services: vec![],
            added_at: None,
            host: String::new(),
            host_name: String::new(),
            power: String::new(),
            power_state: String::new(),
            cloud: String::new(),
            cloud_place: String::new(),
            cloud_delete: String::new(),
            status: String::new(),
            durable: String::new(),
            volumes: Vec::new(),
            labels: Default::default(),
            expires_at_unix: 0,
            on_expire: String::new(),
            created_at_unix: 0,
            egress: Default::default(),
            egress_enforced: None,
            sharing: None,
            stop_reason: String::new(),
        };
        let lines = grouped_lines(std::slice::from_ref(&s), &Default::default());
        let cols: Vec<&str> = lines[0].split_whitespace().collect();
        assert_eq!(
            cols,
            ["local:e2e-1005-cli", "e2e-1005-cli", "local", "0.5.3"]
        );
        // Stopped on its own: the line says why, and so does --json.
        s.power = "stop".into();
        s.power_state = "stopped".into();
        s.stop_reason = "out of memory".into();
        let lines = grouped_lines(std::slice::from_ref(&s), &Default::default());
        assert!(
            lines[0].ends_with("(stopped: out of memory)"),
            "{}",
            lines[0]
        );
        assert_eq!(
            ls_json(std::slice::from_ref(&s))[0]["stop_reason"],
            "out of memory"
        );
        s.stop_reason = String::new();
        let lines = grouped_lines(std::slice::from_ref(&s), &Default::default());
        assert!(lines[0].ends_with("(stopped)"), "{}", lines[0]);
        let json = ls_json(std::slice::from_ref(&s));
        assert_eq!(json[0]["id"], "local:e2e-1005-cli");
        assert_eq!(json[0]["name"], "327b8dc70418");
        assert_eq!(json[0]["display_name"], "e2e-1005-cli");
        // A relay machine keeps its own name.
        s.id = "relay:96fedb7e".into();
        s.name = "gamma-4 Mac Studio".into();
        assert_eq!(display_name(&s), "gamma-4 Mac Studio");
        // No name reported: the id's name, else the id itself.
        s.name = String::new();
        s.id = "cloud:builder".into();
        assert_eq!(display_name(&s), "builder");
        s.id = "relay:96fedb7e".into();
        assert_eq!(display_name(&s), "relay:96fedb7e");
        // A name that is not a container hostname wins over the id's.
        s.id = "local:dev".into();
        s.name = "Design review".into();
        assert_eq!(display_name(&s), "Design review");
        // Renamed: the name it was given, in the list and its JSON.
        s.display_name = "design-review".into();
        assert_eq!(display_name(&s), "design-review");
        assert_eq!(
            ls_json(std::slice::from_ref(&s))[0]["display_name"],
            "design-review"
        );
    }

    #[test]
    fn spaces_rename_takes_the_space_and_its_new_name() {
        match parse_spaces(&["rename", "local:studio", "design-review"]).unwrap() {
            SpacesCmd::Rename { space, name } => {
                assert_eq!(
                    (space.as_str(), name.as_str()),
                    ("local:studio", "design-review")
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(parse_spaces(&["rename", "local:studio"]).is_err());
    }

    #[test]
    fn spaces_share_parses_roles_and_unshare_needs_who_or_all() {
        match parse_spaces(&["share", "local:studio", "bob@example.com"]).unwrap() {
            SpacesCmd::Share { role, yes, .. } => {
                assert_eq!((role.as_str(), yes), ("viewer", false))
            }
            other => panic!("{other:?}"),
        }
        match parse_spaces(&["share", "s", "bob@example.com", "--role", "editor", "--yes"]).unwrap()
        {
            SpacesCmd::Share { role, yes, .. } => {
                assert_eq!((role.as_str(), yes), ("editor", true))
            }
            other => panic!("{other:?}"),
        }
        assert!(parse_spaces(&["share", "s", "bob@example.com", "--role", "owner"]).is_err());
        assert!(parse_spaces(&["unshare", "s"]).is_err(), "who or --all");
        assert!(parse_spaces(&["unshare", "s", "bob@example.com", "--all"]).is_err());
        assert!(matches!(
            parse_spaces(&["unshare", "s", "--all"]).unwrap(),
            SpacesCmd::Unshare {
                all: true,
                who: None,
                ..
            }
        ));
        use cua_spaces::share::ShareConsent as _;
        // Without a terminal (as under `cargo test`) it refuses rather
        // than saying yes on the user's behalf.
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            assert_eq!(
                TerminalConsent { yes: false }.confirm("x").unwrap_err(),
                "no terminal to confirm on; pass --yes"
            );
        }
        assert!(TerminalConsent { yes: true }.confirm("x").is_ok());
        match parse_spaces(&["share", "s", "bob@example.com", "-y"]).unwrap() {
            SpacesCmd::Share { yes, .. } => assert!(yes),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn daemon_share_fails_fast_without_a_terminal_or_yes() {
        let err = daemon_share_gate(false, false).unwrap_err().to_string();
        assert!(err.contains("stdin is not a terminal"), "{err}");
        assert!(err.contains("--yes (-y)"), "{err}");
        assert!(daemon_share_gate(true, false).is_ok());
        assert!(daemon_share_gate(false, true).is_ok());
        assert!(daemon_share_gate(true, true).is_ok());
    }

    #[test]
    fn shares_print_one_line_per_account() {
        let s = cua_sdk::SpaceShares {
            space: "local:studio".into(),
            machine: "space-00000000000000aa".into(),
            invitee_space: "relay:space-00000000000000aa".into(),
            url: String::new(),
            online: true,
            shares: vec![
                cua_sdk::SpaceShareEntry {
                    who: "bob@example.com".into(),
                    role: "editor".into(),
                    connected: true,
                },
                cua_sdk::SpaceShareEntry {
                    who: "carol@example.com".into(),
                    role: "viewer".into(),
                    connected: false,
                },
            ],
        };
        let mut out = Vec::new();
        print_shares(&s, false, &mut out);
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "local:studio is shared as relay:space-00000000000000aa:"
        );
        assert!(
            lines[1].contains("bob@example.com") && lines[1].ends_with("editor  connected"),
            "{text}"
        );
        assert!(lines[2].trim_end().ends_with("viewer"), "{text}");
        let mut out = Vec::new();
        print_shares(
            &cua_sdk::SpaceShares {
                shares: vec![],
                ..s
            },
            false,
            &mut out,
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "local:studio is not shared.\n"
        );
    }

    #[test]
    fn status_lists_recent_access_and_flags_an_altered_log() {
        let s = HostStatus {
            recent_access: vec![cua_host::access::AccessRecord {
                at_ms: 1_000_000 - 120_000,
                via: "relay".into(),
                who: "Ada (acct-1)".into(),
                what: "ProcessService".into(),
            }],
            access_log_error: Some("line 3: altered".into()),
            ..HostStatus::default()
        };
        let mut out = Vec::new();
        render_access(&s, 1_000_000, &mut out);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("does not verify"), "{text}");
        assert!(
            text.contains("2m ago    Ada (acct-1) via relay: ProcessService"),
            "{text}"
        );
    }

    #[tokio::test]
    async fn host_setup_status_stop_remove_against_a_fake_relay() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let tokens = StaticToken("acct".into());
        let mut out = Vec::new();
        let code = run_host(
            parse(&[
                "setup",
                "--relay",
                &relay.url,
                "--name",
                "lab-box",
                "--allow",
                "friend@example.com",
                "--driver-bin",
                driver.to_str().unwrap(),
            ]),
            &home,
            &tokens,
            None,
            Some(manager.clone()),
            false,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("lab-box: relay "), "{text}");
        assert!(text.contains("sharing: on, offline"), "{text}");
        assert!(text.contains("also allowed: friend@example.com"), "{text}");

        let mut out = Vec::new();
        run_host(
            parse(&["status"]),
            &home,
            &tokens,
            None,
            Some(manager.clone()),
            true,
            &mut out,
        )
        .await
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["configured"], true);
        assert_eq!(v["mode"], "relay");
        // This Mac's numbers ride along.
        assert!(v["resources"]["thisMac"]["memoryTotal"].as_u64().unwrap() > 0);
        let id = v["machineId"].as_str().unwrap().to_string();

        // Another of the owner's computers, on the same relay account.
        cua_host::RelayClient::new(&relay.url)
            .unwrap()
            .register(
                "acct",
                &cua_host::relay::RegisterRequest {
                    id: "bbbb000000000002".into(),
                    name: "other-box".into(),
                    allow: vec![],
                    host: None,
                    meta: Default::default(),
                },
            )
            .await
            .unwrap();

        // `cua spaces ls` as the owner shows the machine.
        let mut out = Vec::new();
        // No --relay: the relay this machine joined.
        run_spaces(
            SpacesCmd::Ls {
                relay: None,
                label: vec![],
                no_size: false,
            },
            &home,
            Some(Arc::new(StaticToken("acct".into()))),
            None,
            false,
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains(&format!("relay:{id}")), "{text}");
        assert!(text.contains("lab-box") && text.contains("relay"), "{text}");
        // The new columns, and this machine's own row shows this Mac.
        assert!(text.starts_with("ID "), "{text}");
        for column in ["CPU", "MEMORY", "DISK ON THIS MAC"] {
            assert!(text.lines().next().unwrap().contains(column), "{text}");
        }
        let row = text.lines().find(|l| l.contains("lab-box")).unwrap();
        assert!(row.contains(" busy"), "{row}");
        assert!(row.contains(" free of "), "{row}");
        // This computer's own row says so; the other computer's does not.
        assert!(row.contains("(this computer)"), "{text}");
        assert_eq!(text.matches("(this computer)").count(), 1, "{text}");
        let other = text.lines().find(|l| l.contains("other-box")).unwrap();
        assert!(!other.contains("(this computer)"), "{text}");

        // The same account listed from a computer that is not set up as one
        // of them: no row is this computer.
        let elsewhere = tempfile::tempdir().unwrap();
        let mut out = Vec::new();
        run_spaces(
            SpacesCmd::Ls {
                relay: Some(relay.url.clone()),
                label: vec![],
                no_size: true,
            },
            &elsewhere.path().join(".cua"),
            Some(Arc::new(StaticToken("acct".into()))),
            None,
            false,
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("lab-box") && text.contains("other-box"),
            "{text}"
        );
        assert!(!text.contains("(this computer)"), "{text}");

        // `--json` carries the same numbers.
        let mut out = Vec::new();
        run_spaces(
            SpacesCmd::Ls {
                relay: None,
                no_size: true,
                label: vec![],
            },
            &home,
            Some(Arc::new(StaticToken("acct".into()))),
            None,
            true,
            &mut out,
        )
        .await
        .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(v["thisMac"]["cpuCount"].as_u64().unwrap() >= 1, "{v}");
        assert!(v["warning"]["level"].as_str().is_some(), "{v}");
        let own = v["spaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == format!("relay:{id}"))
            .unwrap();
        assert!(
            own["resources"]["memoryTotal"].as_u64().unwrap() > 0,
            "{own}"
        );

        let mut out = Vec::new();
        run_host(
            parse(&["stop"]),
            &home,
            &tokens,
            None,
            Some(manager.clone()),
            false,
            &mut out,
        )
        .await
        .unwrap();
        assert!(String::from_utf8(out).unwrap().contains("sharing: off"));
        assert!(!relay.machine(&id).unwrap().sharing);

        let mut out = Vec::new();
        run_host(
            parse(&["remove", "--force"]),
            &home,
            &tokens,
            None,
            Some(manager.clone()),
            false,
            &mut out,
        )
        .await
        .unwrap();
        assert!(relay.machine(&id).is_none());
        let mut out = Vec::new();
        let code = run_host(
            parse(&["status"]),
            &home,
            &tokens,
            None,
            Some(manager),
            false,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 1);
    }

    /// `cua host setup` on a machine with no stored session signs in for
    /// the command only: the relay registration works and no account
    /// session is written anywhere under the cua home.
    #[tokio::test]
    async fn setup_without_a_stored_session_signs_in_in_memory_only() {
        let relay = FakeRelay::start().await;
        relay.add_account("fresh-login-token", "user-1", None);
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let logins = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter = logins.clone();
        let tokens = SetupTokens::new(Arc::new(cua_host::NoAccount), move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok("fresh-login-token".to_string()) })
        });
        let mut out = Vec::new();
        run_host(
            parse(&[
                "setup",
                "--relay",
                &relay.url,
                "--driver-bin",
                driver.to_str().unwrap(),
            ]),
            &home,
            &tokens,
            None,
            Some(Arc::new(FakeServiceManager::default())),
            true,
            &mut out,
        )
        .await
        .unwrap();
        assert!(tokens.used_ephemeral());
        assert_eq!(logins.load(std::sync::atomic::Ordering::SeqCst), 1);
        let mut stack = vec![home.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let text = String::from_utf8_lossy(&std::fs::read(&p).unwrap()).into_owned();
                assert!(!text.contains("fresh-login-token"), "{}", p.display());
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                assert!(
                    name != "credentials.json" && name != "session.json",
                    "{}",
                    p.display()
                );
            }
        }
        // A stored session is used as is, without a sign-in.
        let stored = SetupTokens::new(Arc::new(StaticToken("acct".into())), || {
            Box::pin(async { panic!("no sign-in with a stored session") })
        });
        assert_eq!(stored.access_token().await.unwrap(), "acct");
        assert!(!stored.used_ephemeral());
    }

    /// The CLI keeps the core's failure stage without changing its error
    /// category or exit status. No real OS service is called.
    #[tokio::test]
    async fn setup_errors_keep_the_stage_and_error_category() {
        struct RefuseInstall;
        impl ServiceManager for RefuseInstall {
            fn kind(&self) -> RunnerKind {
                RunnerKind::Process
            }
            fn install(&self, _: &cua_host::ServiceSpec) -> cua_host::Result<()> {
                Err(cua_host::Error::Service("fixture install refused".into()))
            }
            fn start(&self) -> cua_host::Result<()> {
                panic!("must not start")
            }
            fn stop(&self) -> cua_host::Result<()> {
                panic!("must not stop")
            }
            fn uninstall(&self) -> cua_host::Result<()> {
                panic!("must not uninstall")
            }
            fn state(&self) -> cua_host::ServiceState {
                panic!("must not read service state")
            }
            fn port_free(&self, _: std::net::SocketAddr, _: cua_host::ports::Transport) -> bool {
                true
            }
        }
        let relay = FakeRelay::start().await;
        let dir = tempfile::tempdir().unwrap();
        let driver = dir.path().join("fixture-spacesd");
        std::fs::write(&driver, b"fixture only").unwrap();
        let missing = dir.path().join("missing-spacesd");
        for (stage, code, message) in [
            ("preflight", 2, "there is nothing to share"),
            ("token", 6, "fixture sign-in refused"),
            ("register", 6, "account token required"),
            ("download", 2, "does not exist"),
            ("service", 1, "fixture install refused"),
        ] {
            let tokens = SetupTokens::new(Arc::new(cua_host::NoAccount), move || {
                Box::pin(async move {
                    if stage == "token" {
                        Err(cua_host::Error::Unauthenticated(
                            "fixture sign-in refused".into(),
                        ))
                    } else {
                        Ok("unrecognized-account".into())
                    }
                })
            });
            let mut args = vec!["setup", "--driver-bin", driver.to_str().unwrap()];
            if matches!(stage, "token" | "register") {
                args.extend(["--relay", &relay.url]);
            } else {
                args.extend(["--direct", "127.0.0.1:3211"]);
            }
            if stage == "preflight" {
                args.extend(["--share", "none"]);
            }
            if stage == "download" {
                args[2] = missing.to_str().unwrap();
            }
            let mut out = Vec::new();
            let error = run_host(
                parse(&args),
                &dir.path().join(stage),
                &tokens,
                None,
                Some(Arc::new(RefuseInstall)),
                true,
                &mut out,
            )
            .await
            .unwrap_err();
            assert!(out.is_empty());
            assert_eq!(crate::exit_code(&error), code, "{stage}: {error}");
            match stage {
                "preflight" | "download" => assert!(matches!(&error, CuaError::InvalidArgument(_))),
                "token" | "register" => assert!(matches!(&error, CuaError::Unauthenticated(_))),
                "service" => assert!(matches!(&error, CuaError::Runtime(_))),
                _ => unreachable!(),
            }
            let text = error.to_string();
            assert!(
                text.contains(&format!("host setup failed at {stage}:")),
                "{text}"
            );
            assert!(text.contains(message), "{text}");
        }
        assert_eq!(relay.register_calls(), 1);
    }

    /// `--allow` sharing stays, is confirmed before it widens access, and is
    /// revocable with `unshare`; the driver policy follows.
    #[tokio::test]
    async fn share_needs_consent_and_unshare_revokes() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let tokens = StaticToken("acct".into());
        let run = |args: Vec<String>| {
            let home = home.clone();
            let manager = manager.clone();
            let tokens = tokens.clone();
            async move {
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                let mut out = Vec::new();
                run_host(
                    parse(&args),
                    &home,
                    &tokens,
                    None,
                    Some(manager),
                    true,
                    &mut out,
                )
                .await
                .map(|_| String::from_utf8(out).unwrap())
            }
        };
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        run(s(&[
            "setup",
            "--relay",
            &relay.url,
            "--allow",
            "friend@example.com",
            "--driver-bin",
            driver.to_str().unwrap(),
        ]))
        .await
        .unwrap();
        let id = Host::new(&home)
            .config()
            .unwrap()
            .unwrap()
            .machine_id
            .unwrap();
        // Not interactive and no --yes: refused.
        assert!(matches!(
            run(s(&["share", "bob@example.com"])).await,
            Err(CuaError::PermissionDenied(_))
        ));
        assert_eq!(relay.machine(&id).unwrap().allow, ["friend@example.com"]);
        let text = run(s(&["share", "bob@example.com", "--yes"]))
            .await
            .unwrap();
        assert!(text.contains("bob@example.com"), "{text}");
        assert_eq!(
            relay.machine(&id).unwrap().allow,
            ["friend@example.com", "bob@example.com"]
        );
        run(s(&["unshare", "FRIEND@example.com"])).await.unwrap();
        assert_eq!(relay.machine(&id).unwrap().allow, ["bob@example.com"]);
        assert_eq!(
            Host::new(&home).policy().unwrap().unwrap().allow,
            ["bob@example.com"]
        );
    }

    #[tokio::test]
    async fn direct_setup_needs_no_account_and_rejects_bad_addresses() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let mut out = Vec::new();
        let err = run_host(
            parse(&["setup", "--direct", "nope"]),
            &home,
            &cua_host::NoAccount,
            None,
            Some(manager.clone()),
            false,
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, CuaError::InvalidArgument(_)));
        run_host(
            parse(&[
                "setup",
                "--direct",
                "127.0.0.1:3299",
                "--driver-bin",
                driver.to_str().unwrap(),
            ]),
            &home,
            &cua_host::NoAccount,
            None,
            Some(manager),
            false,
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("direct http://127.0.0.1:3299"), "{text}");
    }

    /// Runs `cua host <args>` against the fake manager on a machine where
    /// `auto` finds no systemd; returns (stdout, stderr).
    async fn run_fake_io(
        home: &Path,
        manager: &Arc<FakeServiceManager>,
        args: &[&str],
        json: bool,
        detected: RunnerKind,
    ) -> (String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        run_host_to(
            parse(args),
            home,
            &StaticToken("acct".into()),
            None,
            Some(manager.clone()),
            json,
            &mut out,
            &mut err,
            detected,
        )
        .await
        .unwrap();
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    /// Runs `cua host <args>` against the fake manager; returns stdout.
    async fn run_fake(home: &Path, manager: &Arc<FakeServiceManager>, args: &[&str]) -> String {
        run_fake_io(home, manager, args, false, RunnerKind::Process)
            .await
            .0
    }

    fn direct_args<'a>(driver: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
        let mut v = vec![
            "setup",
            "--direct",
            "127.0.0.1:3299",
            "--driver-bin",
            driver,
        ];
        v.extend_from_slice(extra);
        v
    }

    #[tokio::test]
    async fn auto_setup_that_falls_back_to_a_process_warns() {
        let dir = tempfile::tempdir().unwrap();
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let text = run_fake(
            &dir.path().join(".cua"),
            &manager,
            &direct_args(driver.to_str().unwrap(), &[]),
        )
        .await;
        assert_eq!(
            text.matches("will not come back after a reboot").count(),
            1,
            "{text}"
        );
    }

    #[tokio::test]
    async fn explicit_process_runner_setup_does_not_warn() {
        let dir = tempfile::tempdir().unwrap();
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let text = run_fake(
            &dir.path().join(".cua"),
            &manager,
            &direct_args(driver.to_str().unwrap(), &["--runner", "process"]),
        )
        .await;
        assert!(!text.contains("will not come back"), "{text}");
    }

    #[tokio::test]
    async fn setup_runner_warning_goes_to_stderr_with_json() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let mut out = Vec::new();
        run_host(
            parse(&direct_args(
                driver.to_str().unwrap(),
                &["--runner", "process"],
            )),
            &home,
            &StaticToken("acct".into()),
            None,
            Some(manager.clone()),
            true,
            &mut out,
        )
        .await
        .unwrap();
        let host = Host::new(&home).with_service_manager(manager.clone());
        for json in [true, false] {
            let (mut o, mut e) = (Vec::new(), Vec::new());
            warn_setup_runner(
                &host,
                RunnerKind::Auto,
                RunnerKind::Process,
                json,
                &mut o,
                &mut e,
            )
            .unwrap();
            let (o, e) = (String::from_utf8(o).unwrap(), String::from_utf8(e).unwrap());
            let (warned, quiet) = if json { (e, o) } else { (o, e) };
            assert!(warned.contains("will not come back"), "{warned}");
            assert!(quiet.is_empty(), "{quiet}");
        }
        // A full --json auto setup keeps stdout as JSON; the warning is on stderr.
        let dir2 = tempfile::tempdir().unwrap();
        let home2 = dir2.path().join(".cua");
        let m2 = Arc::new(FakeServiceManager::default());
        let args = direct_args(driver.to_str().unwrap(), &[]);
        let (o, e) = run_fake_io(&home2, &m2, &args, true, RunnerKind::Process).await;
        assert!(!o.contains("warning:"), "{o}");
        serde_json::from_str::<serde_json::Value>(o.trim()).unwrap();
        assert_eq!(e.matches("will not come back").count(), 1, "{e}");
        // Re-running (unchanged path) keeps JSON on stdout, warning on stderr.
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        let m3 = Arc::new(FakeServiceManager::default());
        let home3 = dir2.path().join(".cua3");
        let setup = ["setup", "--relay", relay.url.as_str(), "--name", "lab-box"];
        let first = [&setup[..], &["--driver-bin", driver.to_str().unwrap()]].concat();
        run_fake_io(&home3, &m3, &first, true, RunnerKind::Process).await;
        let (o, e) = run_fake_io(&home3, &m3, &setup, true, RunnerKind::Process).await;
        serde_json::from_str::<serde_json::Value>(o.trim()).unwrap();
        assert_eq!(e.matches("will not come back").count(), 1, "{e}");
    }

    #[tokio::test]
    async fn retained_process_runner_without_auto_detection_does_not_warn() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let setup = ["setup", "--relay", relay.url.as_str(), "--name", "lab-box"];
        let first = [
            &setup[..],
            &[
                "--driver-bin",
                driver.to_str().unwrap(),
                "--runner",
                "process",
            ],
        ]
        .concat();
        run_fake(&home, &manager, &first).await;
        // Default rerun (unchanged path) where auto detection would pick systemd.
        let (o, e) = run_fake_io(&home, &manager, &setup, false, RunnerKind::Systemd).await;
        assert!(!o.contains("will not come back") && e.is_empty(), "{o}{e}");
        let host = Host::new(&home).with_service_manager(manager.clone());
        assert_eq!(host.config().unwrap().unwrap().runner, "process");
    }

    #[tokio::test]
    async fn auto_setup_rerun_on_a_process_host_warns() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = dir.path().join("drv");
        std::fs::write(&driver, b"x").unwrap();
        let manager = Arc::new(FakeServiceManager::default());
        let setup = ["setup", "--relay", relay.url.as_str(), "--name", "lab-box"];
        let first = run_fake(
            &home,
            &manager,
            &[&setup[..], &["--driver-bin", driver.to_str().unwrap()]].concat(),
        )
        .await;
        assert_eq!(first.matches("will not come back").count(), 1, "{first}");
        let again = run_fake(&home, &manager, &setup).await;
        assert_eq!(again.matches("will not come back").count(), 1, "{again}");
        manager.stop().unwrap();
        let restarted = run_fake(&home, &manager, &setup).await;
        assert!(
            restarted.contains("Started the host service again"),
            "{restarted}"
        );
        assert_eq!(
            restarted.matches("will not come back").count(),
            1,
            "{restarted}"
        );
    }

    #[test]
    fn status_names_the_runner_and_notes_a_process_runner() {
        let text = |kind: &str, running: bool, detail: &str| {
            let s = HostStatus {
                configured: true,
                mode: Some("relay".into()),
                service: cua_host::ServiceState {
                    installed: true,
                    running,
                    kind: kind.into(),
                    detail: detail.into(),
                },
                ..Default::default()
            };
            let mut out = Vec::new();
            render(&s, &mut out);
            String::from_utf8(out).unwrap()
        };
        let p = text("process", true, "pid 42");
        assert!(p.contains("\n  runner: process running (pid 42)\n"), "{p}");
        assert!(
            p.contains("does not start the host again after a reboot"),
            "{p}"
        );
        let s = text("systemd", false, "");
        assert!(
            s.contains("  runner: systemd installed, not running"),
            "{s}"
        );
        assert!(!s.contains("does not start the host again"), "{s}");
    }
    #[test]
    fn relay_and_direct_conflict() {
        #[derive(clap::Parser)]
        struct W {
            #[command(subcommand)]
            c: HostCmd,
        }
        assert!(
            <W as clap::Parser>::try_parse_from([
                "host",
                "setup",
                "--relay",
                "https://r",
                "--direct",
                "1.2.3.4:1"
            ])
            .is_err()
        );
    }

    mod machines_rm {
        use super::*;
        use cua_host::MemoryKeySlot;
        use cua_host::relay::{DeviceState, RegisterRequest};
        use std::cell::Cell;

        const ID: &str = "0123abcd4567ef89";

        struct Fx {
            _env: crate::util::test_env::Guard,
            relay: FakeRelay,
            cua: Arc<cua_sdk::Cua>,
            _dir: tempfile::TempDir,
            auth: Arc<DeviceAuth>,
            laptop: String,
            remote: Vec<String>,
        }

        async fn device(relay: &FakeRelay, name: &str, i: usize) -> (Arc<DeviceAuth>, String) {
            let d = Arc::new(
                DeviceAuth::new(
                    &relay.url,
                    Arc::new(StaticToken("acct".into())),
                    Arc::new(MemoryKeySlot::default()),
                    name,
                )
                .unwrap()
                .with_machine_id(Some(format!("machine-of-{i}"))),
            );
            relay.fresh_sign_in("user-1");
            d.enroll().await.unwrap();
            let id = d.device_id().unwrap().unwrap();
            (d, id)
        }

        async fn register(
            relay: &FakeRelay,
            id: &str,
            name: &str,
            host: Option<&str>,
            meta: std::collections::BTreeMap<String, String>,
        ) {
            RelayClient::new(&relay.url)
                .unwrap()
                .register(
                    "acct",
                    &RegisterRequest {
                        id: id.into(),
                        name: name.into(),
                        allow: vec![],
                        host: host.map(Into::into),
                        meta,
                    },
                )
                .await
                .unwrap();
        }

        async fn fx(remote_names: &[&str]) -> Fx {
            let env = crate::util::test_env::isolated();
            let relay = FakeRelay::start().await;
            relay.add_account("acct", "user-1", Some("ada@example.com"));
            relay.require_devices(true);
            relay.fresh_sign_in("user-1");
            let (auth, laptop) = device(&relay, "laptop", 0).await;
            let mut remote = vec![];
            for (i, n) in remote_names.iter().enumerate() {
                remote.push(device(&relay, n, i + 1).await.1);
            }
            register(&relay, ID, "old-laptop", None, Default::default()).await;
            let dir = tempfile::tempdir().unwrap();
            let cua = cua_sdk::Cua::embedded(cua_sdk::CuaConfig {
                state_dir: Some(dir.path().join("sandboxes").display().to_string()),
                spaces_home: Some(dir.path().join("cua").display().to_string()),
                ..Default::default()
            })
            .unwrap();
            let (url, dev) = (relay.url.clone(), auth.clone());
            attach_relay_account(&cua, || {
                cua_spaces::RelayAccount::new(url, Arc::new(StaticToken("acct".into())))
                    .with_device(dev)
            });
            Fx {
                _env: env,
                relay,
                cua,
                _dir: dir,
                auth,
                laptop,
                remote,
            }
        }

        /// Runs `rm`; returns the result, the output and the prompt count.
        async fn rm(
            f: &Fx,
            arg: &str,
            json: bool,
            answer: impl Fn() -> bool,
        ) -> (Result<i32, CuaError>, String, u32) {
            let asked = Cell::new(0);
            let mut out = Vec::new();
            let r = remove_machine(
                &f.cua,
                &f.auth,
                &StaticToken("acct".into()),
                arg,
                &|_| {
                    asked.set(asked.get() + 1);
                    answer()
                },
                json,
                &mut out,
            )
            .await;
            (r, String::from_utf8(out).unwrap(), asked.get())
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn machines_rm_removes_a_gone_computer_then_revokes_its_device() {
            let f = fx(&["old-laptop"]).await;
            let (r, text, _) = rm(&f, "old-laptop", false, || true).await;
            assert_eq!(r.unwrap(), 0, "{text}");
            assert!(f.relay.machine(ID).is_none());
            assert_eq!(
                f.relay.device_state(&f.remote[0]),
                Some(DeviceState::Revoked)
            );
            assert_eq!(f.relay.device_state(&f.laptop), Some(DeviceState::Enrolled));
            assert!(
                text.contains("Removed old-laptop (relay:0123abcd4567ef89)"),
                "{text}"
            );
            assert!(text.contains("Revoked old-laptop ("), "{text}");
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn machines_rm_by_bare_id_reports_json() {
            let f = fx(&["old-laptop"]).await;
            let (r, text, _) = rm(&f, ID, true, || true).await;
            assert_eq!(r.unwrap(), 0, "{text}");
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["removed"], true, "{text}");
            assert_eq!(v["machine"]["id"], "relay:0123abcd4567ef89");
            assert_eq!(v["device"]["status"], "revoked", "{text}");
            assert_eq!(v["device"]["ids"], serde_json::json!([f.remote[0]]));
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn machines_rm_refuses_a_connected_computer() {
            let f = fx(&["old-laptop"]).await;
            f.relay.set_online(ID, true, "1.0");
            let (r, _, asked) = rm(&f, "old-laptop", false, || true).await;
            let Err(CuaError::InvalidArgument(m)) = r else {
                panic!("{r:?}")
            };
            assert!(
                m.contains("is connected now") && m.contains("cua host remove"),
                "{m}"
            );
            assert_eq!(asked, 0);
            assert!(f.relay.machine(ID).is_some());
            assert_eq!(
                f.relay.device_state(&f.remote[0]),
                Some(DeviceState::Enrolled)
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn machines_rm_unknown_name_is_not_found() {
            let f = fx(&["old-laptop"]).await;
            let (r, text, asked) = rm(&f, "no-such-box", false, || true).await;
            let Err(CuaError::NotFound(m)) = r else {
                panic!("{r:?}")
            };
            assert!(m.contains("no-such-box"), "{m}");
            assert_eq!(asked, 0);
            assert!(text.is_empty());
            assert!(f.relay.machine(ID).is_some());
            assert_eq!(
                f.relay.device_state(&f.remote[0]),
                Some(DeviceState::Enrolled)
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn machines_rm_rechecks_after_confirmation() {
            let f = fx(&["old-laptop"]).await;
            let (r, _, asked) = rm(&f, "old-laptop", false, || {
                f.relay.set_online(ID, true, "1.0");
                true
            })
            .await;
            assert!(matches!(r, Err(CuaError::InvalidArgument(_))), "{r:?}");
            assert_eq!(asked, 1);
            assert!(f.relay.machine(ID).is_some());
            assert_eq!(
                f.relay.device_state(&f.remote[0]),
                Some(DeviceState::Enrolled)
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn machines_rm_never_revokes_this_device_or_guesses() {
            // (a) A machine named like this device: this device stays.
            let f = fx(&[]).await;
            let other = "fedcba9876543210";
            register(&f.relay, other, "laptop", None, Default::default()).await;
            let (r, text, _) = rm(&f, "laptop", false, || true).await;
            assert_eq!(r.unwrap(), 0, "{text}");
            assert!(f.relay.machine(other).is_none());
            assert_eq!(f.relay.device_state(&f.laptop), Some(DeviceState::Enrolled));
            assert!(text.contains("no device was revoked"), "{text}");
            drop(f); // releases the isolated environment

            // (b) Two devices share the name: neither is revoked.
            let f = fx(&["old-laptop", "old-laptop"]).await;
            let (r, text, _) = rm(&f, "old-laptop", false, || true).await;
            assert_eq!(r.unwrap(), 0, "{text}");
            assert!(f.relay.machine(ID).is_none());
            for id in &f.remote {
                assert_eq!(f.relay.device_state(id), Some(DeviceState::Enrolled));
                assert!(text.contains(id.as_str()), "{text}");
            }
            assert!(text.contains("cua devices revoke <id>"), "{text}");
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn machines_rm_refuses_a_runtime_bound_to_another_relay() {
            // A daemon-like runtime on relay B, the caller's account on A.
            let f = fx(&["old-laptop"]).await;
            let other = FakeRelay::start().await;
            let dir = tempfile::tempdir().unwrap();
            let cua = cua_sdk::Cua::embedded(cua_sdk::CuaConfig {
                state_dir: Some(dir.path().join("sandboxes").display().to_string()),
                spaces_home: Some(dir.path().join("cua").display().to_string()),
                ..Default::default()
            })
            .unwrap();
            let url = other.url.clone();
            attach_relay_account(&cua, || {
                cua_spaces::RelayAccount::new(url, Arc::new(StaticToken("acct".into())))
            });
            let g = Fx { cua, ..f };
            let (r, text, asked) = rm(&g, "old-laptop", false, || true).await;
            let Err(CuaError::InvalidArgument(m)) = r else {
                panic!("{r:?}")
            };
            assert!(m.contains("--embedded"), "{m}");
            assert_eq!(asked, 0);
            assert!(text.is_empty(), "{text}");
            assert!(g.relay.machine(ID).is_some());
            assert_eq!(
                g.relay.device_state(&g.remote[0]),
                Some(DeviceState::Enrolled)
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn machines_rm_device_failure_keeps_recovery_steps() {
            // This device is revoked while the prompt is open (and the relay
            // stops requiring devices): the row goes, the revoke is refused.
            for json in [false, true] {
                let f = fx(&["old-laptop"]).await;
                let dir = tempfile::tempdir().unwrap();
                let cua = cua_sdk::Cua::embedded(cua_sdk::CuaConfig {
                    state_dir: Some(dir.path().join("sandboxes").display().to_string()),
                    spaces_home: Some(dir.path().join("cua").display().to_string()),
                    ..Default::default()
                })
                .unwrap();
                let url = f.relay.url.clone();
                attach_relay_account(&cua, || {
                    cua_spaces::RelayAccount::new(url, Arc::new(StaticToken("acct".into())))
                });
                let g = Fx { cua, ..f };
                let (admin, _) = device(&g.relay, "admin", 9).await;
                let mut out = Vec::new();
                let r = remove_machine(
                    &g.cua,
                    &g.auth,
                    &StaticToken("acct".into()),
                    "old-laptop",
                    &|_| {
                        g.relay.require_devices(false);
                        std::thread::scope(|s| {
                            s.spawn(|| {
                                tokio::runtime::Runtime::new()
                                    .unwrap()
                                    .block_on(admin.revoke(&g.laptop))
                                    .unwrap()
                            });
                        });
                        true
                    },
                    json,
                    &mut out,
                )
                .await;
                let text = String::from_utf8(out).unwrap();
                assert!(
                    matches!(r, Err(CuaError::PermissionDenied(_))),
                    "{r:?} {text}"
                );
                assert!(g.relay.machine(ID).is_none(), "{text}");
                assert_eq!(
                    g.relay.device_state(&g.remote[0]),
                    Some(DeviceState::Enrolled)
                );
                assert!(text.contains("cua devices revoke <id>"), "{text}");
                if json {
                    assert!(text.contains(r#""status":"failed""#), "{text}");
                }
            }
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn machines_rm_declined_changes_nothing() {
            let f = fx(&["old-laptop"]).await;
            let (r, _, asked) = rm(&f, "old-laptop", false, || false).await;
            let Err(CuaError::PermissionDenied(m)) = r else {
                panic!("{r:?}")
            };
            assert!(m.contains("pass --yes"), "{m}");
            assert_eq!(asked, 1);
            assert!(f.relay.machine(ID).is_some());
            assert_eq!(
                f.relay.device_state(&f.remote[0]),
                Some(DeviceState::Enrolled)
            );
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
        async fn machines_rm_refuses_spaces() {
            let f = fx(&[]).await;
            let none = Default::default;
            register(&f.relay, "space-hosted1", "hosted", Some(ID), none()).await;
            let meta = [("cua.cloud.provider".to_string(), "hetzner".to_string())].into();
            register(&f.relay, "space-cloud001", "cloudy", None, meta).await;
            register(&f.relay, "space-selfreg1", "selfreg", None, none()).await;
            for (id, name) in [
                ("space-hosted1", "hosted"),
                ("space-cloud001", "cloudy"),
                ("space-selfreg1", "selfreg"),
            ] {
                let (r, _, asked) = rm(&f, name, false, || true).await;
                let Err(CuaError::InvalidArgument(m)) = r else {
                    panic!("{name}: {r:?}")
                };
                assert!(m.contains("not a computer"), "{m}");
                assert_eq!(asked, 0);
                assert!(f.relay.machine(id).is_some());
            }
        }
    }

    /// `cua mcp` without a daemon (embedded) sees the account's relay
    /// machines, as the daemon's Spaces do; it attaches only once and never
    /// to a daemon-backed `cua`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn embedded_mcp_spaces_list_the_accounts_relay_machines() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", None);
        cua_host::RelayClient::new(&relay.url)
            .unwrap()
            .register(
                "acct",
                &cua_host::relay::RegisterRequest {
                    id: "0123abcd4567ef89".into(),
                    name: "lab-box".into(),
                    allow: vec![],
                    host: None,
                    meta: Default::default(),
                },
            )
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cua = cua_sdk::Cua::embedded(cua_sdk::CuaConfig {
            state_dir: Some(dir.path().join("sandboxes").display().to_string()),
            spaces_home: Some(dir.path().join("cua").display().to_string()),
            ..Default::default()
        })
        .unwrap();
        let list = |cua: Arc<cua_sdk::Cua>| async move {
            cua.spaces()
                .call_tool_json("list_spaces".into(), None)
                .await
                .unwrap()
                .text
        };
        assert!(!list(cua.clone()).await.contains("relay:0123abcd4567ef89"));

        let account = || {
            cua_spaces::RelayAccount::new(relay.url.clone(), Arc::new(StaticToken("acct".into())))
        };
        assert!(attach_relay_account(&cua, account));
        let text = list(cua.clone()).await;
        assert!(text.contains("relay:0123abcd4567ef89"), "{text}");
        assert!(text.contains("lab-box"), "{text}");
        // Already attached: left alone.
        assert!(!attach_relay_account(&cua, || unreachable!()));
    }

    fn try_parse(args: &[&str]) -> Result<HostCmd, clap::Error> {
        #[derive(clap::Parser)]
        struct W {
            #[command(subcommand)]
            c: HostCmd,
        }
        <W as clap::Parser>::try_parse_from(std::iter::once("host").chain(args.iter().copied()))
            .map(|w| w.c)
    }

    /// What a setup left on the machine: config and policy settings, and
    /// whether the service runs without the desktop services.
    type Seen = ((bool, bool), (bool, bool), bool);

    fn seen(home: &Path, manager: &FakeServiceManager) -> Seen {
        let host = Host::new(home);
        let c = host.config().unwrap().unwrap();
        let p = host.policy().unwrap().unwrap();
        let spec = manager.spec.lock().unwrap().clone().unwrap();
        (
            (c.share_desktop, c.provide_spaces),
            (p.share_desktop, p.provide_spaces),
            spec.args.iter().any(|a| a == "--no-desktop"),
        )
    }

    async fn setup_run(
        home: &Path,
        manager: &Arc<FakeServiceManager>,
        args: &[&str],
        json: bool,
    ) -> Result<String, CuaError> {
        let mut out = Vec::new();
        run_host(
            parse(args),
            home,
            &StaticToken("acct".into()),
            None,
            Some(manager.clone()),
            json,
            &mut out,
        )
        .await?;
        Ok(String::from_utf8(out).unwrap())
    }

    fn fixture_driver(dir: &Path) -> PathBuf {
        let driver = dir.join("drv");
        std::fs::write(&driver, b"x").unwrap();
        driver
    }

    /// Each `--share` value and each old flag spelling of it set up the
    /// same host: config, driver policy and service arguments.
    #[tokio::test]
    async fn share_values_and_their_old_aliases_set_up_the_same_host() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", Some("ada@example.com"));
        let rows: [(&[&[&str]], Seen); 3] = [
            (
                &[
                    &["--share", "desktop"],
                    &["--no-provide-spaces"],
                    &["--desktop", "--no-provide-spaces"],
                ],
                ((true, false), (true, false), false),
            ),
            (
                &[
                    &["--share", "spaces"],
                    &["--profile", "spare"],
                    &["--no-desktop", "--provide-spaces"],
                ],
                ((false, true), (false, true), true),
            ),
            (
                &[
                    &["--share", "both"],
                    &["--provide-spaces"],
                    &["--desktop", "--provide-spaces"],
                    &["--profile", "desktop", "--provide-spaces"],
                ],
                ((true, true), (true, true), false),
            ),
        ];
        for (spellings, want) in rows {
            for extra in spellings {
                let dir = tempfile::tempdir().unwrap();
                let home = dir.path().join(".cua");
                let driver = fixture_driver(dir.path());
                let manager = Arc::new(FakeServiceManager::default());
                let mut args = vec![
                    "setup",
                    "--relay",
                    &relay.url,
                    "--driver-bin",
                    driver.to_str().unwrap(),
                ];
                args.extend_from_slice(extra);
                setup_run(&home, &manager, &args, true).await.unwrap();
                assert_eq!(seen(&home, &manager), want, "relay {extra:?}");
            }
        }
        // Direct mode needs no account.
        let direct = |extra: &'static [&'static str]| async move {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join(".cua");
            let driver = fixture_driver(dir.path());
            let manager = Arc::new(FakeServiceManager::default());
            let mut args = vec![
                "setup",
                "--direct",
                "127.0.0.1:3211",
                "--driver-bin",
                driver.to_str().unwrap(),
            ];
            args.extend_from_slice(extra);
            setup_run(&home, &manager, &args, true).await.unwrap();
            seen(&home, &manager)
        };
        let share = direct(&["--share", "spaces"]).await;
        assert_eq!(share, ((false, true), (false, true), true));
        assert_eq!(share, direct(&["--profile", "spare"]).await);
    }

    /// `none` (and its old spelling) is refused before anything happens.
    #[tokio::test]
    async fn share_none_and_its_old_alias_refuse_before_registering() {
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", None);
        for extra in [
            &["--share", "none"][..],
            &["--no-desktop", "--no-provide-spaces"][..],
        ] {
            let dir = tempfile::tempdir().unwrap();
            let home = dir.path().join(".cua");
            let driver = fixture_driver(dir.path());
            let manager = Arc::new(FakeServiceManager::default());
            let mut args = vec![
                "setup",
                "--relay",
                &relay.url,
                "--driver-bin",
                driver.to_str().unwrap(),
            ];
            args.extend_from_slice(extra);
            let e = setup_run(&home, &manager, &args, true).await.unwrap_err();
            assert!(matches!(e, CuaError::InvalidArgument(_)), "{e:?}");
            assert_eq!(crate::exit_code(&e), 2);
            let text = e.to_string();
            assert!(text.contains("host setup failed at preflight:"), "{text}");
            assert!(
                text.contains("use --share desktop, spaces or both"),
                "{text}"
            );
            assert!(!home.join("host").join("config.json").exists());
            assert!(manager.calls.lock().unwrap().is_empty());
        }
        assert_eq!(relay.register_calls(), 0);
    }

    #[test]
    fn share_conflicts_with_the_old_flags_and_rejects_other_values() {
        for old in [
            &["--profile", "spare"][..],
            &["--desktop"][..],
            &["--no-desktop"][..],
            &["--provide-spaces"][..],
            &["--no-provide-spaces"][..],
        ] {
            let mut args = vec!["setup", "--share", "both"];
            args.extend_from_slice(old);
            let e = try_parse(&args).unwrap_err();
            assert_eq!(
                e.kind(),
                clap::error::ErrorKind::ArgumentConflict,
                "{old:?}"
            );
        }
        let e = try_parse(&["setup", "--share", "some"]).unwrap_err();
        assert_eq!(e.kind(), clap::error::ErrorKind::InvalidValue);
        assert!(
            e.to_string()
                .contains("[possible values: desktop, spaces, both, none]")
        );
        match try_parse(&["setup", "--pair", "K7QM-4XPB-2RHD", "--share", "spaces"]).unwrap() {
            HostCmd::Setup { pair, share, .. } => assert_eq!(
                (pair.as_deref(), share),
                (Some("K7QM-4XPB-2RHD"), Some(HostShare::Spaces))
            ),
            other => panic!("{other:?}"),
        }
        // Help shows only --share.
        let help = <HostCmd as clap::Subcommand>::augment_subcommands(clap::Command::new("host"))
            .find_subcommand_mut("setup")
            .unwrap()
            .render_long_help()
            .to_string();
        assert!(help.contains("--share"), "{help}");
        for old in [
            "--profile",
            "--no-desktop",
            "--provide-spaces",
            "--no-provide-spaces",
        ] {
            assert!(!help.contains(old), "{old} is hidden: {help}");
        }
    }

    /// A computer configured to share nothing (`host config`) still refuses
    /// `--share none`; the unchanged shortcut must not answer for it.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn share_none_is_refused_even_by_a_host_configured_to_share_nothing() {
        let _env = crate::util::test_env::isolated();
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", None);
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = fixture_driver(dir.path());
        // SAFETY: test_env::isolated holds the environment lock.
        unsafe { std::env::set_var("CUA_SPACESD_BIN", &driver) };
        let manager = Arc::new(FakeServiceManager::default());
        setup_run(&home, &manager, &["setup", "--relay", &relay.url], true)
            .await
            .unwrap();
        Host::new(&home)
            .with_service_manager(manager.clone())
            .configure(cua_host::HostSettingsChange {
                share_desktop: Some(false),
                provide_spaces: Some(false),
                ..Default::default()
            })
            .await
            .unwrap();
        let e = setup_run(
            &home,
            &manager,
            &["setup", "--relay", &relay.url, "--share", "none"],
            true,
        )
        .await
        .unwrap_err();
        assert_eq!(crate::exit_code(&e), 2, "{e}");
        assert!(
            e.to_string()
                .contains("use --share desktop, spaces or both"),
            "{e}"
        );
    }

    /// Setup again with `--share` applies both settings, and changes
    /// nothing when they already match.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn setup_again_with_share_applies_only_a_change() {
        let _env = crate::util::test_env::isolated();
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", None);
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        let driver = fixture_driver(dir.path());
        // SAFETY: test_env::isolated holds the environment lock.
        unsafe { std::env::set_var("CUA_SPACESD_BIN", &driver) };
        let manager = Arc::new(FakeServiceManager::default());
        let run = |share: &'static str| {
            let (home, manager, url) = (home.clone(), manager.clone(), relay.url.clone());
            async move {
                setup_run(
                    &home,
                    &manager,
                    &["setup", "--relay", &url, "--share", share],
                    true,
                )
                .await
                .unwrap()
            }
        };
        let both = ((true, true), (true, true), false);
        run("both").await;
        assert_eq!(seen(&home, &manager), both);
        assert_eq!(relay.register_calls(), 1);
        let out = run("both").await;
        assert!(out.contains("\"changed\":false"), "{out}");
        assert_eq!((seen(&home, &manager), relay.register_calls()), (both, 1));
        run("spaces").await;
        assert_eq!(seen(&home, &manager), ((false, true), (false, true), true));
        assert_eq!(relay.register_calls(), 2);
        run("both").await;
        assert_eq!(seen(&home, &manager), both);
        assert_eq!(relay.register_calls(), 3);
        run("desktop").await;
        assert_eq!(seen(&home, &manager), ((true, false), (true, false), false));
    }

    /// The old flags re-run exactly as before `--share`: a flag that does
    /// not name the desktop is not checked against it.
    #[tokio::test]
    #[allow(clippy::await_holding_lock)]
    async fn old_flags_on_a_rerun_keep_mains_behaviour() {
        let _env = crate::util::test_env::isolated();
        let relay = FakeRelay::start().await;
        relay.add_account("acct", "user-1", None);
        let dir = tempfile::tempdir().unwrap();
        let driver = fixture_driver(dir.path());
        // SAFETY: test_env::isolated holds the environment lock.
        unsafe { std::env::set_var("CUA_SPACESD_BIN", &driver) };
        let url = relay.url.clone();
        let run =
            |home: PathBuf, manager: Arc<FakeServiceManager>, flags: &'static [&'static str]| {
                let url = url.clone();
                async move {
                    let mut args = vec!["setup", "--relay", &url];
                    args.extend_from_slice(flags);
                    setup_run(&home, &manager, &args, true).await.unwrap()
                }
            };
        let spare = ((false, true), (false, true), true);
        let home = dir.path().join("a");
        let manager = Arc::new(FakeServiceManager::default());
        run(home.clone(), manager.clone(), &["--profile", "spare"]).await;
        assert_eq!(seen(&home, &manager), spare);
        let calls = relay.register_calls();
        let out = run(home.clone(), manager.clone(), &["--provide-spaces"]).await;
        assert!(out.contains("\"changed\":false"), "{out}");
        assert_eq!(
            (seen(&home, &manager), relay.register_calls()),
            (spare, calls)
        );
        run(home.clone(), manager.clone(), &["--no-provide-spaces"]).await;
        assert_eq!(seen(&home, &manager), ((true, false), (true, false), false));
        assert_eq!(relay.register_calls(), calls + 1);

        let home = dir.path().join("b");
        let manager = Arc::new(FakeServiceManager::default());
        run(home.clone(), manager.clone(), &["--no-provide-spaces"]).await;
        run(home.clone(), manager.clone(), &["--provide-spaces"]).await;
        assert_eq!(seen(&home, &manager), ((true, true), (true, true), false));
    }
}
