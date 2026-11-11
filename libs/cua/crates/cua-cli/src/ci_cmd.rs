//! `cua ci …`: Cua CI: GitHub Actions jobs on Cua Spaces. A thin layer over
//! the `cua-ci` crate: `setup` keeps the GitHub credential in the OS
//! keychain, writes the pool's TOML and installs the service that runs
//! `cua ci serve`; the rest read the pool's files or drive the pool.

use std::io::{IsTerminal as _, Read as _, Write};
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Subcommand};
use cua_ci::config::{AuthKind, EgressMode, GitHubConfig, HostResources, PoolConfig, Scope};
use cua_ci::github::{AppKey, Credentials, GitHubClient, RunnerApi as _};
use cua_ci::secret::{APP_KEY_SERVICE, KeySource, Secret, SecretStore, StoreKind, TOKEN_SERVICE};
use cua_ci::{CiPaths, Pool, PoolDeps, SystemClock};
use cua_sdk::CuaError;

use crate::util::line;

#[derive(Subcommand, Debug, Clone)]
pub enum CiCmd {
    /// Connect this computer to Cua CI: sign in, install the hosted GitHub
    /// app on your organization (you pick the repositories), and serve
    /// jobs here. Prints the `runs-on` line for your workflows.
    #[command(after_help = "Examples:
  # The usual way: sign in, install the app on GitHub in your browser, serve jobs
  cua ci connect
  # Serve only these repositories, and open a pull request that changes their workflows
  cua ci connect --allow acme/app --allow acme/api --open-pr
  # A headless Mac over SSH: print the links instead of opening a browser
  cua ci connect --no-browser
  # Your own GitHub App with its key in a 0600 file (self-hosting, no Cua service)
  cua ci connect --own-app --org trycua --app-id 123456 --key-file ~/.cua/ci/github-app.pem")]
    Connect(Box<ConnectArgs>),
    /// Add capacity: a computer over SSH. `host USER@ADDRESS` installs and
    /// joins any rented or owned computer, which then serves jobs as one more
    /// computer. Cloud accounts are not in this build yet.
    #[command(after_help = "Examples:
  cua ci add host root@build.example.com")]
    Add(Box<AddArgs>),
    /// Remove capacity added with `cua ci add`: ends the computers Cua
    /// started there (only the ones it recorded and tagged), then forgets
    /// the target.
    #[command(after_help = "Examples:
  cua ci remove host root@build.example.com")]
    Remove {
        /// `aws`, `gcp`, `azure`, `hetzner`, `modal`, `macstadium` or `host`.
        target: String,
        /// For `host`: its SSH address.
        address: Option<String>,
        /// Do not ask.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// What Cua CI is doing: computers and free slots, waiting and running
    /// jobs, the last 7 days, and this month's spend against the budget.
    /// Pools of your own GitHub App show with `--pool`.
    #[command(after_help = "Examples:
  cua ci status
  cua ci status --json
  cua ci status --pool trycua")]
    Status {
        /// A pool made with `connect --own-app` (default: Cua CI, else every pool).
        #[arg(long)]
        pool: Option<String>,
    },
    /// Everything the CI Runners page shows, as JSON: setup steps, the
    /// repositories that ran jobs, usage over the last 7 days, the jobs
    /// table (wait, run, cost, computer, cloud), the autopilot's reasons and
    /// the budget. Read from this computer's own records, no network
    /// (`--live` adds the repositories the service lists).
    #[command(after_help = "Examples:
  cua ci overview
  cua ci overview --live")]
    Overview {
        /// Also ask the service which repositories the GitHub app can see
        /// and which of them may run jobs (needs the network).
        #[arg(long)]
        live: bool,
    },
    /// What the Cua Spaces apps run for the CI Runners page: one operation
    /// (`overview`, `connect`, `connect_token`, `connect_cancel`,
    /// `repositories`, `goal`, `log`), its arguments as one JSON object on
    /// stdin, one JSON line on stdout. Not for typing: use the commands above.
    #[command(hide = true)]
    App {
        /// The operation.
        op: String,
    },
    /// Why capacity looks the way it does: what Cua chose from your job
    /// history (computers kept ready, idle time, spot or on-demand) and the
    /// reason for each, in plain words.
    #[command(after_help = "Examples:
  cua ci why
  cua ci why aws")]
    Why {
        /// A target (`aws`, `hetzner`, ...); default: every target.
        target: Option<String>,
    },
    /// Which repositories Cua CI serves, how fork pull requests are
    /// treated, concurrency caps and the monthly budget. With no options
    /// it shows the current settings.
    #[command(after_help = "Examples:
  cua ci config
  cua ci config --allow acme/api
  cua ci config --fork-prs deny
  cua ci config --max-jobs 4 --max-macos-jobs 2 --max-job-minutes 90
  cua ci config --budget 50 --on-budget-limit stop
  cua ci config --repo acme/app --fork-prs restricted --max-jobs 2")]
    Config(Box<ConfigArgs>),
    /// Stop serving jobs on this computer and remove it from Cua CI. The
    /// GitHub app stays installed unless you ask. For a pool of your own
    /// GitHub App, `--pool` removes it (its Spaces, runners and key).
    #[command(after_help = "Examples:
  cua ci disconnect
  cua ci disconnect --repo acme/old
  cua ci disconnect --all-machines --unlink
  cua ci disconnect --pool trycua --yes")]
    Disconnect {
        /// Stop serving this repository only (repeatable).
        #[arg(long, value_name = "REPO")]
        repo: Vec<String>,
        /// Remove every computer of the account, not just this one.
        #[arg(long)]
        all_machines: bool,
        /// Also unlink the GitHub installation (removes Cua's runner group and runners).
        #[arg(long)]
        unlink: bool,
        /// Also uninstall the Cua CI app from GitHub.
        #[arg(long)]
        uninstall: bool,
        /// A pool made with `connect --own-app`.
        #[arg(long)]
        pool: Option<String>,
        /// Do not ask.
        #[arg(long, short = 'y')]
        yes: bool,
    },
    /// Run what this computer serves in the foreground (what the service
    /// runs): Cua CI jobs, or a pool of your own GitHub App with `--pool`.
    #[command(after_help = "Examples:
  cua ci serve
  cua ci serve --pool trycua
  # One pass of an own-app pool, then exit
  cua ci serve --pool trycua --once")]
    Serve {
        /// A pool made with `connect --own-app` (default: Cua CI).
        #[arg(long)]
        pool: Option<String>,
        /// One reconcile pass of an own-app pool (create, check and clean up), then exit.
        #[arg(long)]
        once: bool,
    },
    /// An own-app pool's runners now: Space, runner, state and age.
    #[command(
        visible_alias = "list",
        after_help = "Examples:
  cua ci ls
  cua ci ls --pool trycua --json"
    )]
    Ls {
        /// The pool (default: every pool).
        #[arg(long)]
        pool: Option<String>,
    },
    /// Recent jobs; or one job's timeline and Cua CI log (the job's own
    /// step logs stay at GitHub). An own-app pool's events with `--pool`.
    #[command(after_help = "Examples:
  cua ci logs
  cua ci logs --repo acme/app
  cua ci logs 9912345678/27300000111
  cua ci logs job_01HZX --follow
  cua ci logs --pool trycua --slot 3f9a12c0")]
    Logs {
        /// A job: its `run/job` key or id (default: list recent jobs).
        job: Option<String>,
        /// Only this repository's jobs.
        #[arg(long)]
        repo: Option<String>,
        /// Keep showing new lines until the job ends.
        #[arg(long, short = 'f')]
        follow: bool,
        /// How many jobs to list.
        #[arg(long, default_value_t = 30)]
        limit: u32,
        /// A pool made with `connect --own-app`.
        #[arg(long)]
        pool: Option<String>,
        /// A runner slot of that pool (`cua ci ls`, or the event log).
        #[arg(long)]
        slot: Option<String>,
        /// How many recent events of that pool to show.
        #[arg(long, default_value_t = 40)]
        events: usize,
    },
    /// Stop an own-app pool's service (idle runners stay until it starts again).
    #[command(after_help = "Examples:
  cua ci stop --pool trycua")]
    Stop {
        /// The pool (default: the only one).
        #[arg(long)]
        pool: Option<String>,
    },
    /// Start an own-app pool's service again.
    #[command(after_help = "Examples:
  cua ci start --pool trycua")]
    Start {
        /// The pool (default: the only one).
        #[arg(long)]
        pool: Option<String>,
    },
    /// Empty the dependency caches and repository mirrors of this
    /// computer's pools, for a cold run (jobs restore nothing and the first
    /// checkout fetches in full).
    #[command(after_help = "Examples:
  cua ci cache clear
  cua ci cache clear --pool trycua")]
    Cache {
        #[command(subcommand)]
        cmd: CacheCmd,
    },
}

/// `cua ci cache`.
#[derive(Subcommand, Debug, Clone)]
pub enum CacheCmd {
    /// Delete the caches, mirrors and golden saved starts.
    #[command(after_help = "Examples:
  cua ci cache clear
  cua ci cache clear --pool trycua")]
    Clear {
        /// Only this pool (default: every pool of this computer).
        #[arg(long)]
        pool: Option<String>,
    },
}

/// `cua ci connect`: the hosted Cua CI service by default; your own GitHub
/// App with `--own-app` (the options of the second group).
#[derive(Args, Debug, Clone)]
pub struct ConnectArgs {
    /// Use your own GitHub App (its key kept in the keychain or a file only
    /// you can read) and no Cua service, for self-hosting. `--org` or
    /// `--repo` and the App options below go with it.
    #[arg(long)]
    own_app: bool,
    /// A Cua organization to connect (id, alias or name); default: your
    /// own account.
    #[arg(long)]
    team: Option<String>,
    /// Serve jobs of this repository (`owner/name`, repeatable). Default:
    /// the ones you pick, or ask.
    #[arg(long, value_name = "REPO")]
    allow: Vec<String>,
    /// Open a pull request in each served repository that points its
    /// workflows at Cua CI (it uses your own GitHub sign-in: `gh auth
    /// login`, or GH_TOKEN). Nothing is pushed to the default branch.
    #[arg(long)]
    open_pr: bool,
    /// With --open-pr: change only these workflow files.
    #[arg(long, value_name = "PATH")]
    workflows: Vec<String>,
    /// Do not ask.
    #[arg(long, short = 'y')]
    yes: bool,
    /// Print the sign-in and GitHub links instead of opening a browser.
    #[arg(long)]
    no_browser: bool,
    /// macOS jobs at once on this computer (default 2 on Apple silicon;
    /// at most 2 per Mac).
    #[arg(long)]
    macos_slots: Option<u32>,
    /// Linux jobs at once on this computer (default: one per 4 cores).
    #[arg(long)]
    linux_slots: Option<u32>,
    /// The image macOS jobs start from.
    #[arg(long, value_name = "IMAGE")]
    macos_image: Option<String>,
    /// The image Linux jobs start from.
    #[arg(long, value_name = "IMAGE")]
    linux_image: Option<String>,
    /// The name shown in `cua ci status` (default: this computer's name).
    #[arg(long)]
    name: Option<String>,
    /// Also add a cloud right away (`aws`, `gcp`, `azure`, `hetzner`,
    /// `modal`, `macstadium`): the same as `cua ci add` after connecting.
    /// Not in this build yet.
    #[arg(long, value_name = "TARGET")]
    cloud: Option<String>,
    #[command(flatten)]
    own: SetupArgs,
}

/// `cua ci add`.
#[derive(Args, Debug, Clone)]
pub struct AddArgs {
    /// `aws`, `gcp`, `azure`, `hetzner`, `modal`, `macstadium` or `host`.
    target: String,
    /// For `host`: its SSH address (`USER@ADDRESS`).
    address: Option<String>,
    /// Monthly budget in dollars (default: asked once, 50).
    #[arg(long, value_name = "DOLLARS")]
    budget: Option<f64>,
    /// Hard cap in dollars an hour (default: a twentieth of the budget).
    #[arg(long, value_name = "DOLLARS")]
    max_per_hour: Option<f64>,
    /// Fix how many free slots are kept ready so a job does not wait for a
    /// computer to boot (0: never). Without it Cua chooses from your own job
    /// history, inside the budget. Never on Mac hosts.
    #[arg(long, value_name = "SLOTS")]
    warm: Option<u32>,
    /// What Cua optimizes when it chooses: `cheapest` (lowest bill),
    /// `balanced` (default) or `fastest` (shortest starts). The budget
    /// stays a hard limit for all three.
    #[arg(long, value_name = "GOAL", value_parser = ["cheapest", "balanced", "fastest"])]
    goal: Option<String>,
    /// Region or location (default: the nearest one).
    #[arg(long)]
    region: Option<String>,
    /// AWS: Mac hosts for macOS jobs (billed at least 24 hours a host).
    #[arg(long)]
    macos: bool,
    /// Do not ask.
    #[arg(long, short = 'y')]
    yes: bool,
    /// Print links instead of opening a browser.
    #[arg(long)]
    no_browser: bool,
    /// Hetzner, Modal, MacStadium: read the token from stdin.
    #[arg(long)]
    token_stdin: bool,
    /// AWS: the 12-digit account id (found automatically when `aws` is
    /// signed in on this computer).
    #[arg(long, value_name = "ID")]
    account_id: Option<String>,
    /// Google Cloud: the project number the trust step prints.
    #[arg(long, value_name = "NUMBER")]
    project_number: Option<String>,
    /// Azure: the subscription id.
    #[arg(long, value_name = "ID")]
    subscription: Option<String>,
    /// Azure: the Microsoft Entra tenant id.
    #[arg(long, value_name = "ID")]
    tenant: Option<String>,
    /// Azure: the managed identity's client id.
    #[arg(long, value_name = "ID")]
    client_id: Option<String>,
    /// host: a private key file for the SSH login. With it Cua manages the
    /// computer itself (installs and removes its own units over SSH, keeps
    /// the key in this computer's keychain, runs jobs in isolated Spaces on
    /// it). Without it the computer signs in to Cua on its own.
    #[arg(long, value_name = "FILE")]
    key: Option<String>,
    /// host: read the private key from stdin.
    #[arg(long)]
    key_stdin: bool,
    /// host: the computer's public host key as `<type> <base64>` (default:
    /// fetched with ssh-keyscan and confirmed by its fingerprint).
    #[arg(long, value_name = "KEY")]
    host_key: Option<String>,
    /// host: what the computer costs a month in dollars when rented (default:
    /// yours, free).
    #[arg(long, value_name = "DOLLARS")]
    monthly: Option<f64>,
}

impl AddArgs {
    fn into_add(self) -> crate::ci_targets::Add {
        crate::ci_targets::Add {
            target: self.target,
            address: self.address,
            budget: self.budget,
            max_per_hour: self.max_per_hour,
            warm: self.warm,
            goal: self.goal,
            region: self.region,
            macos: self.macos,
            yes: self.yes,
            no_browser: self.no_browser,
            token_stdin: self.token_stdin,
            account_id: self.account_id,
            project_number: self.project_number,
            subscription: self.subscription,
            tenant: self.tenant,
            client_id: self.client_id,
            key: self.key,
            key_stdin: self.key_stdin,
            host_key: self.host_key,
            monthly: self.monthly,
        }
    }
}

/// `cua ci config`.
#[derive(Args, Debug, Clone, Default)]
pub struct ConfigArgs {
    /// Serve jobs of this repository (`owner/name`, repeatable).
    #[arg(long, value_name = "REPO")]
    allow: Vec<String>,
    /// Stop serving this repository (repeatable).
    #[arg(long, value_name = "REPO")]
    remove: Vec<String>,
    /// Accept `owner/*` in --allow (every repository of an owner).
    #[arg(long)]
    allow_org_wide: bool,
    /// Fork pull requests: `restricted` (default: no secrets, read-only
    /// caches, no private network), `deny` (never served) or `allow`.
    #[arg(long, value_parser = ["restricted", "deny", "allow"])]
    fork_prs: Option<String>,
    /// Apply --fork-prs and --max-jobs to this repository only.
    #[arg(long, value_name = "REPO")]
    repo: Option<String>,
    /// Jobs at once (a number, or `none`).
    #[arg(long, value_name = "N")]
    max_jobs: Option<String>,
    /// macOS jobs at once (a number, or `none`).
    #[arg(long, value_name = "N")]
    max_macos_jobs: Option<String>,
    /// The longest a job may run, in minutes (a number, or `none`).
    #[arg(long, value_name = "MINUTES")]
    max_job_minutes: Option<String>,
    /// The longest a job waits for capacity (`15m`, or `none`).
    #[arg(long, value_name = "DURATION")]
    queue_timeout: Option<String>,
    /// Monthly budget in dollars for cloud computers (`50`, or `none`).
    /// With none set, cloud computers are not used.
    #[arg(long, value_name = "DOLLARS")]
    budget: Option<String>,
    /// At the budget limit: `stop` (default) sends no more jobs to cloud
    /// computers; `warn` only warns.
    #[arg(long, value_parser = ["stop", "warn"])]
    on_budget_limit: Option<String>,
    /// What the budget counts: `byoc` (cloud computers, default) or `all`.
    #[arg(long, value_parser = ["byoc", "all"])]
    budget_scope: Option<String>,
    /// Network for jobs: `deny-private` (default) keeps them off your
    /// private network and tailnet; `off`.
    #[arg(long, value_parser = ["deny-private", "off"])]
    egress: Option<String>,
    /// Confirm the changes that widen what jobs may do.
    #[arg(long, short = 'y')]
    yes: bool,
}

#[derive(Args, Debug, Clone)]
#[command(group(clap::ArgGroup::new("scope").required(false).args(["org", "repo"])))]
#[command(group(clap::ArgGroup::new("credential").args(["private_key_file", "private_key_stdin", "token_stdin", "key_file"])))]
pub struct SetupArgs {
    /// The GitHub organization: the one to install the Cua CI app on, or
    /// with --own-app the organization to register runners for (`--org trycua`).
    #[arg(long)]
    org: Option<String>,
    /// With --own-app: register runners for one repository (`--repo owner/name`).
    #[arg(long)]
    repo: Option<String>,
    /// The pool's name (default: the organization, or owner-name).
    #[arg(long)]
    pool: Option<String>,
    /// The organization runner group runners join (default: the
    /// organization's default group). Restrict it to selected
    /// repositories at GitHub.
    #[arg(long)]
    group: Option<String>,
    /// Labels besides self-hosted, the OS, the architecture and
    /// cua (comma-separated).
    #[arg(long, value_delimiter = ',')]
    labels: Vec<String>,
    /// The image each runner's Space starts from: a registry image, or
    /// `checkpoint:<name>` (a golden image saved with `cua sb checkpoint`,
    /// with --os).
    #[arg(long, default_value = cua_ci::config::DEFAULT_MACOS_IMAGE)]
    image: String,
    /// The image's OS, `macos` or `linux` (default: from the image's name;
    /// needed for a checkpoint).
    #[arg(long, value_parser = ["macos", "linux"])]
    os: Option<String>,
    /// Where job Spaces run: `local` (this computer), `auto` (any of
    /// your computers) or `host:<computer>`.
    #[arg(long, default_value = "local")]
    on: String,
    /// At most this many runners at once (macOS on one computer: at most 2).
    #[arg(long, default_value_t = 2)]
    max: u32,
    /// Idle runners to keep ready (default: --max).
    #[arg(long)]
    warm: Option<u32>,
    /// vCPUs per runner (default for macOS here: half this Mac's cores per
    /// runner).
    #[arg(long)]
    cpus: Option<u32>,
    /// Memory per runner in MiB (default for macOS here: this Mac's
    /// memory less 8 GiB split between the runners, at most 8 GiB).
    #[arg(long)]
    memory_mb: Option<u64>,
    /// Disk per runner in GiB (default: the image's).
    #[arg(long)]
    disk_gb: Option<u32>,
    /// Stop a job that runs longer (its Space is deleted).
    #[arg(long, default_value = "6h")]
    job_timeout: String,
    /// Replace an idle runner after this long.
    #[arg(long, default_value = "60m")]
    idle_timeout: String,
    /// `deny-private` (default): runner Spaces cannot reach private,
    /// tailnet, link-local or unique-local addresses; `off`.
    #[arg(long, default_value = "deny-private", value_parser = ["deny-private", "off"])]
    egress: String,
    /// Refuse to start runners where egress cannot be enforced (macOS VMs
    /// today) instead of starting them without it.
    #[arg(long)]
    require_egress: bool,
    /// The GitHub App's id.
    #[arg(long)]
    app_id: Option<u64>,
    /// The App's installation id (default: looked up).
    #[arg(long)]
    installation_id: Option<u64>,
    /// The App's private key (.pem). Delete the file afterwards: the key
    /// is kept in the keychain.
    #[arg(long)]
    private_key_file: Option<PathBuf>,
    /// Read the App's private key from stdin.
    #[arg(long)]
    private_key_stdin: bool,
    /// Keep the App's private key in this file instead of the keychain (it
    /// must be yours, mode 0600). For a headless host or one set up over
    /// SSH, whose login keychain cannot be used. The pool reads it on every
    /// start; nothing is copied.
    #[arg(long, value_name = "PATH")]
    key_file: Option<PathBuf>,
    /// Read a GitHub token from stdin instead of using an App.
    #[arg(long)]
    token_stdin: bool,
    /// Where the credential is kept: `keychain` (the login keychain on
    /// macOS) or `file` (a file only you can read under
    /// ~/.cua/ci/secrets/; for a Mac set up over SSH, whose keychain is
    /// locked).
    #[arg(long, default_value = "keychain", value_parser = ["keychain", "file"])]
    secret_store: String,
    /// The GitHub REST API (GitHub Enterprise Server: `https://HOST/api/v3`).
    #[arg(long, default_value = "https://api.github.com")]
    api_url: String,
    /// Write the configuration without installing the service (run
    /// `cua ci serve` yourself).
    #[arg(long)]
    no_service: bool,
    /// Do not check the credential with GitHub.
    #[arg(long)]
    no_verify: bool,
}

/// Whether `cmd` drives Spaces (and so needs the SDK handle).
pub fn needs_spaces(cmd: &CiCmd) -> bool {
    match cmd {
        CiCmd::Serve { .. } => true,
        CiCmd::Disconnect { repo, .. } => repo.is_empty(),
        CiCmd::Add(a) => crate::ci_targets::needs_spaces(&a.target),
        CiCmd::Connect(a) => a
            .cloud
            .as_deref()
            .is_some_and(crate::ci_targets::needs_spaces),
        _ => false,
    }
}

/// Whether a command without `--pool` (or with `--pool hosted`) is about
/// Cua CI itself rather than a pool of your own GitHub App.
fn hosted_target(paths: &CiPaths, pool: Option<&str>) -> bool {
    match pool {
        Some(p) => p == crate::ci_hosted::HOSTED_POOL,
        None => crate::ci_hosted::is_connected(paths),
    }
}

/// What runs before the SDK handle is opened (which may start the cua
/// daemon): `disconnect` asks first.
pub fn preflight(cmd: &mut CiCmd) -> Result<(), CuaError> {
    let CiCmd::Disconnect {
        repo,
        pool,
        yes,
        all_machines,
        unlink,
        ..
    } = cmd
    else {
        return Ok(());
    };
    let paths = CiPaths::from_env();
    if !repo.is_empty() {
        return Ok(()); // only the allow list changes
    }
    if hosted_target(&paths, pool.as_deref()) {
        if !*yes {
            if !std::io::stdin().is_terminal() {
                return Err(CuaError::InvalidArgument(
                    "disconnecting stops this computer's Cua CI service and deletes the Spaces of \
                     running jobs; pass --yes"
                        .into(),
                ));
            }
            eprint!(
                "Disconnect this computer from Cua CI: stop its service, delete the Spaces of running jobs \
                 and remove {}{}? [y/N] ",
                if *all_machines {
                    "every computer of the account"
                } else {
                    "the computer"
                },
                if *unlink {
                    " and unlink the GitHub installation"
                } else {
                    ""
                }
            );
            let mut answer = String::new();
            let _ = std::io::stdin().read_line(&mut answer);
            if !matches!(answer.trim(), "y" | "Y" | "yes") {
                return Err(CuaError::Cancelled("kept the connection".into()));
            }
        }
        *pool = Some(crate::ci_hosted::HOSTED_POOL.into());
        *yes = true;
        return Ok(());
    }
    let name = pick_pool(&paths, pool.clone())?;
    let cfg = load(&paths, &name)?;
    if !*yes {
        if !std::io::stdin().is_terminal() {
            return Err(CuaError::InvalidArgument(format!(
                "removing pool {name} deletes its Spaces and runners; pass --yes"
            )));
        }
        eprint!(
            "Remove runner pool {name} ({}): stop its service, delete its Spaces and runners (a \
             running job is stopped), and remove its configuration and credential? [y/N] ",
            cfg.scope
        );
        let mut answer = String::new();
        let _ = std::io::stdin().read_line(&mut answer);
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            return Err(CuaError::Cancelled("kept the pool".into()));
        }
    }
    *pool = Some(name);
    *yes = true;
    Ok(())
}

pub(crate) fn err(e: cua_ci::Error) -> CuaError {
    match e {
        cua_ci::Error::Config(m) => CuaError::InvalidArgument(m),
        cua_ci::Error::GitHub(m) => CuaError::Http(format!("GitHub: {m}")),
        cua_ci::Error::Secret(m) => CuaError::PermissionDenied(format!("credential store: {m}")),
        cua_ci::Error::Service(a) => match a.status {
            401 => CuaError::Unauthenticated(format!("{a}. Sign in with `cua auth login`.")),
            402 | 403 => CuaError::PermissionDenied(a.to_string()),
            404 => CuaError::NotFound(a.to_string()),
            400 | 422 => CuaError::InvalidArgument(a.to_string()),
            426 => CuaError::Unsupported(format!("{a}. Update cua.")),
            _ => CuaError::Http(a.to_string()),
        },
        other => CuaError::Internal(other.to_string()),
    }
}

fn store(paths: &CiPaths, kind: StoreKind) -> Box<dyn SecretStore> {
    cua_ci::secret::store(kind, paths.secrets())
}

/// `--pool`, or the only pool.
fn pick_pool(paths: &CiPaths, pool: Option<String>) -> Result<String, CuaError> {
    if let Some(p) = pool {
        if !paths.config(&p).is_file() {
            return Err(CuaError::NotFound(format!(
                "no runner pool {p:?} (pools: {}); create it with `cua ci connect --own-app`",
                list_or_none(&paths.pools())
            )));
        }
        return Ok(p);
    }
    let pools = paths.pools();
    match pools.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(CuaError::NotFound(
            "no runner pool yet; create one with `cua ci connect --own-app --org ORG ...`".into(),
        )),
        many => Err(CuaError::InvalidArgument(format!(
            "several runner pools ({}); choose one with --pool",
            many.join(", ")
        ))),
    }
}

fn list_or_none(v: &[String]) -> String {
    if v.is_empty() {
        "none".into()
    } else {
        v.join(", ")
    }
}

/// The pools a read-only command shows.
fn pools_for(paths: &CiPaths, pool: Option<String>) -> Result<Vec<String>, CuaError> {
    match pool {
        Some(p) => Ok(vec![pick_pool(paths, Some(p))?]),
        None => Ok(paths.pools()),
    }
}

fn load(paths: &CiPaths, pool: &str) -> Result<PoolConfig, CuaError> {
    PoolConfig::load(&paths.config(pool)).map_err(err)
}

/// The pool's GitHub client, from its stored credential.
fn github(cfg: &PoolConfig, paths: &CiPaths) -> Result<GitHubClient, CuaError> {
    let g = &cfg.github;
    // A key file (CUA_CI_GITHUB_APP_KEY_FILE, or the pool's
    // private_key_file) comes first, then the credential store.
    let file = match g.auth {
        AuthKind::App => cua_ci::secret::app_key_file(g.private_key_file.as_deref()),
        AuthKind::Token => None,
    };
    let keys = store(paths, g.secret_store);
    let (secret, _source): (Secret, KeySource) = cua_ci::secret::resolve_app_key(
        file.as_deref(),
        keys.as_ref(),
        &g.secret_service,
        &g.secret_account,
    )
    .map_err(err)?
    .ok_or_else(|| {
        CuaError::NotFound(format!(
            "no stored GitHub credential for pool {} ({}); run `cua ci connect --own-app` again, or \
             point private_key_file (or CUA_CI_GITHUB_APP_KEY_FILE) at the App's key file",
            cfg.name,
            keys.describe(&g.secret_service, &g.secret_account)
        ))
    })?;
    let creds = credentials(g.auth, g.app_id, g.installation_id, secret)?;
    GitHubClient::new(&g.api_url, cfg.scope.clone(), creds).map_err(err)
}

fn credentials(
    auth: AuthKind,
    app_id: Option<u64>,
    installation_id: Option<u64>,
    secret: Secret,
) -> Result<Credentials, CuaError> {
    Ok(match auth {
        AuthKind::App => Credentials::App {
            app_id: app_id.ok_or_else(|| CuaError::InvalidArgument("--app-id is needed".into()))?,
            installation_id,
            key: Arc::new(AppKey::from_pem(secret.expose()).map_err(err)?),
        },
        AuthKind::Token => Credentials::Token(secret),
    })
}

pub async fn run(
    cmd: CiCmd,
    cua: Option<Arc<cua_sdk::Cua>>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let paths = CiPaths::from_env();
    match cmd {
        CiCmd::Connect(a) => connect(*a, &paths, cua, json, out).await,
        CiCmd::Add(a) => crate::ci_targets::add(a.into_add(), &paths, cua, json, out).await,
        CiCmd::Remove {
            target,
            address,
            yes,
        } => crate::ci_targets::remove(target, address, yes, &paths, json, out).await,
        CiCmd::Overview { live } => crate::ci_overview::run(&paths, live, out).await,
        CiCmd::App { op } => crate::ci_app::run(&op, &paths, out).await,
        CiCmd::Why { target } => crate::ci_targets::why(target, &paths, json, out),
        CiCmd::Serve { pool, once } => {
            let cua = cua.ok_or_else(|| CuaError::Internal("no Spaces handle".into()))?;
            if hosted_target(&paths, pool.as_deref()) {
                return crate::ci_hosted::serve(&paths, cua, out).await;
            }
            let pool = pick_pool(&paths, pool)?;
            serve(&paths, &pool, cua, once, out).await
        }
        CiCmd::Status { pool } => {
            if hosted_target(&paths, pool.as_deref()) {
                crate::ci_hosted::status(&paths, json, out).await
            } else {
                status(&paths, pool, json, out)
            }
        }
        CiCmd::Config(a) => {
            let a = *a;
            let c = crate::ci_hosted::ConfigChange {
                allow: a.allow,
                remove: a.remove,
                allow_org_wide: a.allow_org_wide,
                fork_prs: a.fork_prs,
                repo: a.repo,
                max_jobs: a.max_jobs,
                max_macos_jobs: a.max_macos_jobs,
                max_job_minutes: a.max_job_minutes,
                queue_timeout: a.queue_timeout,
                budget: a.budget,
                on_budget_limit: a.on_budget_limit,
                budget_scope: a.budget_scope,
                egress: a.egress,
                yes: a.yes,
            };
            crate::ci_hosted::config(&paths, c, json, out).await
        }
        CiCmd::Cache {
            cmd: CacheCmd::Clear { pool },
        } => cache_clear(&paths, pool, json, out),
        CiCmd::Ls { pool } => ls(&paths, pool, json, out),
        CiCmd::Logs {
            job,
            repo,
            follow,
            limit,
            pool,
            slot,
            events,
        } => {
            if pool.is_none() && slot.is_none() && hosted_target(&paths, None) {
                crate::ci_hosted::logs(&paths, job, repo, follow, limit, json, out).await
            } else {
                logs(&paths, pool, slot, events, json, out)
            }
        }
        CiCmd::Stop { pool } => {
            let pool = pick_pool(&paths, pool)?;
            let was = cua_ci::service::stop(&pool).map_err(err)?;
            line(
                out,
                if was {
                    format!(
                        "Stopped the runner service of {pool}. Idle runners stay until `cua ci start --pool {pool}`."
                    )
                } else {
                    format!("The runner service of {pool} was not running.")
                },
            );
            Ok(0)
        }
        CiCmd::Start { pool } => {
            let pool = pick_pool(&paths, pool)?;
            cua_ci::service::start(&pool).map_err(err)?;
            line(out, format!("Started the runner service of {pool}."));
            Ok(0)
        }
        CiCmd::Disconnect {
            repo,
            all_machines,
            unlink,
            uninstall,
            pool,
            yes,
        } => {
            if !repo.is_empty() {
                // Only the allow list changes: no Spaces are touched.
                let c = crate::ci_hosted::ConfigChange {
                    remove: repo,
                    ..Default::default()
                };
                return crate::ci_hosted::config(&paths, c, json, out).await;
            }
            let cua = cua.ok_or_else(|| CuaError::Internal("no Spaces handle".into()))?;
            if hosted_target(&paths, pool.as_deref()) {
                let d = crate::ci_hosted::Disconnect {
                    all_machines,
                    unlink,
                    uninstall,
                };
                return crate::ci_hosted::disconnect(&paths, d, cua, json, out).await;
            }
            let pool = pick_pool(&paths, pool)?;
            remove(&paths, &pool, cua, yes, json, out).await
        }
    }
}

/// The flags of `--own-app` that were given (so a hosted connect can say
/// they need it).
fn own_app_flags(a: &SetupArgs) -> Vec<&'static str> {
    let mut v = Vec::new();
    let mut flag = |on: bool, name: &'static str| {
        if on {
            v.push(name);
        }
    };
    flag(a.repo.is_some(), "--repo");
    flag(a.pool.is_some(), "--pool");
    flag(a.group.is_some(), "--group");
    flag(!a.labels.is_empty(), "--labels");
    flag(a.image != cua_ci::config::DEFAULT_MACOS_IMAGE, "--image");
    flag(a.os.is_some(), "--os");
    flag(a.warm.is_some(), "--warm");
    flag(a.cpus.is_some(), "--cpus");
    flag(a.memory_mb.is_some(), "--memory-mb");
    flag(a.disk_gb.is_some(), "--disk-gb");
    flag(a.require_egress, "--require-egress");
    flag(a.app_id.is_some(), "--app-id");
    flag(a.installation_id.is_some(), "--installation-id");
    flag(a.private_key_file.is_some(), "--private-key-file");
    flag(a.private_key_stdin, "--private-key-stdin");
    flag(a.key_file.is_some(), "--key-file");
    flag(a.token_stdin, "--token-stdin");
    flag(a.no_verify, "--no-verify");
    v
}

async fn connect(
    a: ConnectArgs,
    paths: &CiPaths,
    cua: Option<Arc<cua_sdk::Cua>>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    if a.own_app {
        if a.cloud.is_some() {
            return Err(CuaError::InvalidArgument(
                "--cloud goes with the hosted service, not --own-app".into(),
            ));
        }
        if a.own.org.is_none() && a.own.repo.is_none() {
            return Err(CuaError::InvalidArgument(
                "--own-app needs --org ORG or --repo OWNER/NAME (with --app-id and a key file, or --token-stdin)".into(),
            ));
        }
        if !a.allow.is_empty() || a.open_pr || a.team.is_some() {
            return Err(CuaError::InvalidArgument(
                "--allow, --open-pr and --team are for the hosted Cua CI service, not --own-app"
                    .into(),
            ));
        }
        return setup(a.own, paths, json, out, None).await;
    }
    let bad = own_app_flags(&a.own);
    if !bad.is_empty() {
        return Err(CuaError::InvalidArgument(format!(
            "{} {} for your own GitHub App: add --own-app (or leave them out to use the hosted \
             Cua CI app; --allow picks repositories)",
            bad.join(", "),
            if bad.len() == 1 { "is" } else { "are" }
        )));
    }
    if !a.workflows.is_empty() && !a.open_pr {
        return Err(CuaError::InvalidArgument(
            "--workflows goes with --open-pr".into(),
        ));
    }
    let cloud = match &a.cloud {
        Some(c) => {
            let k = cua_ci::targets::Kind::parse(c).map_err(CuaError::InvalidArgument)?;
            if k == cua_ci::targets::Kind::Host {
                return Err(CuaError::InvalidArgument(
                    "--cloud takes a cloud; for a computer over SSH use `cua ci add host USER@ADDRESS`".into(),
                ));
            }
            if !crate::ci_targets::CLOUD_CAPACITY {
                return Err(CuaError::Unsupported(format!(
                    "--cloud {c}: cloud capacity is not in this build of cua yet; connect without it, \
                     or add a computer with `cua ci add host USER@ADDRESS`"
                )));
            }
            Some(crate::ci_targets::Add {
                target: c.clone(),
                yes: a.yes,
                no_browser: a.no_browser,
                ..Default::default()
            })
        }
        None => None,
    };
    let code = crate::ci_hosted::connect(
        crate::ci_hosted::Connect {
            team: a.team,
            github_org: a.own.org,
            allow: a.allow,
            open_pr: a.open_pr,
            workflows: a.workflows,
            yes: a.yes,
            no_browser: a.no_browser,
            on: a.own.on,
            macos_slots: a.macos_slots,
            linux_slots: a.linux_slots,
            macos_image: a.macos_image,
            linux_image: a.linux_image,
            name: a.name,
            no_service: a.own.no_service,
            github_api: std::env::var("CUA_CI_GITHUB_API")
                .ok()
                .filter(|v| !v.is_empty()),
        },
        paths,
        json,
        out,
    )
    .await?;
    match cloud {
        Some(add) if code == 0 => crate::ci_targets::add(add, paths, cua, json, out).await,
        _ => Ok(code),
    }
}

fn read_stdin() -> Result<String, CuaError> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| CuaError::InvalidArgument(format!("reading stdin: {e}")))?;
    Ok(s)
}

/// A pool of your own GitHub credential from a token already in hand (the
/// Cua Spaces apps hold it): the same setup as `cua ci connect --own-app
/// --token-stdin`, the token never on a command line.
pub(crate) async fn connect_own_token(
    scope: &str,
    token: String,
    opts: OwnTokenOptions,
    paths: &CiPaths,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    use clap::{Args as _, FromArgMatches as _};
    let mut argv = vec![
        "connect".to_string(),
        "--own-app".into(),
        "--token-stdin".into(),
    ];
    match scope.split_once(':') {
        Some(("org", o)) => argv.extend(["--org".into(), o.to_string()]),
        Some(("repo", r)) => argv.extend(["--repo".into(), r.to_string()]),
        _ => {
            return Err(CuaError::InvalidArgument(format!(
                "{scope:?}: use org:NAME or repo:OWNER/NAME"
            )));
        }
    }
    argv.extend(["--secret-store".into(), opts.secret_store]);
    argv.extend(["--api-url".into(), opts.api_url]);
    if !opts.service {
        argv.push("--no-service".into());
    }
    let cmd = ConnectArgs::augment_args(clap::Command::new("connect"));
    let m = cmd
        .try_get_matches_from(argv)
        .map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
    let a =
        ConnectArgs::from_arg_matches(&m).map_err(|e| CuaError::InvalidArgument(e.to_string()))?;
    setup(a.own, paths, true, out, Some(token)).await
}

/// How [`connect_own_token`] keeps and uses the credential.
pub(crate) struct OwnTokenOptions {
    /// `keychain` or `file`.
    pub secret_store: String,
    /// The GitHub REST API.
    pub api_url: String,
    /// Install and start the service that serves jobs.
    pub service: bool,
}

async fn setup(
    a: SetupArgs,
    paths: &CiPaths,
    json: bool,
    out: &mut dyn Write,
    token: Option<String>,
) -> Result<i32, CuaError> {
    let scope = match (&a.org, &a.repo) {
        (Some(o), _) => Scope::parse(&format!("org:{o}")),
        (_, Some(r)) => Scope::parse(&format!("repo:{r}")),
        _ => unreachable!("clap requires one"),
    }
    .map_err(err)?;
    let name = a.pool.clone().unwrap_or_else(|| scope.default_pool_name());
    if name == crate::ci_hosted::HOSTED_POOL {
        return Err(CuaError::InvalidArgument(format!(
            "the pool name {name:?} is reserved for Cua CI; choose another with --pool"
        )));
    }
    let auth = if a.token_stdin {
        AuthKind::Token
    } else {
        AuthKind::App
    };
    let service = match auth {
        AuthKind::App => APP_KEY_SERVICE,
        AuthKind::Token => TOKEN_SERVICE,
    };
    let secret_store = StoreKind::parse(&a.secret_store).map_err(err)?;
    let keys = store(paths, secret_store);
    // The credential: given now, or the one setup stored before.
    let given = if let Some(p) = &a.key_file {
        Some(cua_ci::secret::read_private_file(p).map_err(err)?)
    } else if let Some(p) = &a.private_key_file {
        Some(Secret::new(std::fs::read_to_string(p).map_err(|e| {
            CuaError::InvalidArgument(format!("{}: {e}", p.display()))
        })?))
    } else if a.token_stdin && token.is_some() {
        token.map(|t| Secret::new(t.trim().to_string()))
    } else if a.private_key_stdin || a.token_stdin {
        let s = read_stdin()?;
        Some(Secret::new(if a.token_stdin {
            s.trim().to_string()
        } else {
            s
        }))
    } else {
        None
    };
    let secret = match given {
        Some(s) => s,
        None => keys.get(service, &name).map_err(err)?.ok_or_else(|| {
            CuaError::InvalidArgument(
                "give the GitHub App's key (--private-key-file PATH or --private-key-stdin, with \
                 --app-id) or a token (--token-stdin)"
                    .into(),
            )
        })?,
    };
    if secret.expose().trim().is_empty() {
        return Err(CuaError::InvalidArgument("the credential is empty".into()));
    }
    if auth == AuthKind::App && a.app_id.is_none() {
        return Err(CuaError::InvalidArgument(
            "--app-id: the GitHub App's id (its settings page, \"App ID\")".into(),
        ));
    }
    let mut cfg = PoolConfig {
        name: name.clone(),
        scope: scope.clone(),
        runner_group: a.group.clone(),
        labels: a
            .labels
            .iter()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        image: a.image.clone(),
        os: match a.os.as_deref() {
            Some("macos") => Some(cua_ci::GuestOs::Macos),
            Some("linux") => Some(cua_ci::GuestOs::Linux),
            _ => None,
        },
        on: a.on.clone(),
        max_runners: a.max,
        warm: a.warm,
        cpus: a.cpus,
        memory_mb: a.memory_mb,
        disk_gb: a.disk_gb,
        job_timeout: a.job_timeout.clone(),
        idle_timeout: a.idle_timeout.clone(),
        register_timeout: "10m".into(),
        egress: if a.egress == "off" {
            EgressMode::Off
        } else {
            EgressMode::DenyPrivate
        },
        egress_deny: vec![],
        egress_allow: vec![],
        require_egress: a.require_egress,
        name_prefix: None,
        log_retention_days: 7,
        github: GitHubConfig {
            api_url: a.api_url.clone(),
            auth,
            app_id: a.app_id,
            installation_id: a.installation_id,
            secret_service: service.into(),
            secret_account: name.clone(),
            secret_store,
            private_key_file: a.key_file.clone(),
        },
    };
    cfg.validate().map_err(err)?;
    let creds = credentials(auth, a.app_id, a.installation_id, secret.clone())?;
    let mut warnings = Vec::new();
    if !a.no_verify {
        let gh = GitHubClient::new(&cfg.github.api_url, scope.clone(), creds).map_err(err)?;
        if auth == AuthKind::App {
            cfg.github.installation_id = Some(gh.installation_id().await.map_err(err)?);
        }
        gh.runner_group_id(cfg.runner_group.as_deref())
            .await
            .map_err(err)?;
        gh.list_runners().await.map_err(err)?;
        if scope.is_org() {
            let groups = gh.runner_groups().await.map_err(err)?;
            let group = match &cfg.runner_group {
                Some(n) => groups.iter().find(|g| g.name.eq_ignore_ascii_case(n)),
                None => groups.iter().find(|g| g.default),
            };
            if let Some(g) = group
                && g.visibility == "all"
            {
                warnings.push(format!(
                    "runner group {:?} is open to every repository of {}{}. Restrict it to \
                     selected repositories (organization Settings, Actions, Runner groups).",
                    g.name,
                    scope,
                    if g.allows_public_repositories {
                        ", public ones included"
                    } else {
                        ""
                    }
                ));
            }
        }
    }
    if cfg.guest_os() == cua_ci::GuestOs::Macos && cfg.max_runners > cfg.effective_max() {
        warnings.push(format!(
            "--max {}: a Mac runs at most 2 macOS VMs (Apple's license), so this pool uses {} at \
             once here; use --on auto to spread runners over your Macs",
            cfg.max_runners,
            cfg.effective_max()
        ));
    }
    if cfg.guest_os() == cua_ci::GuestOs::Macos
        && cfg.egress != EgressMode::Off
        && !cfg.require_egress
    {
        warnings.push(
            "macOS VMs cannot enforce an egress policy yet: runner Spaces can reach your local \
             network and tailnet. See https://cua.ai/docs/spaces/guides/cua-ci#security-on-public-repositories"
                .into(),
        );
    }
    // A key file stays where it is; anything else goes into the store.
    if a.key_file.is_none() {
        keys.set(service, &name, &secret).map_err(err)?;
    }
    let config_path = paths.config(&name);
    cfg.save(&config_path).map_err(err)?;
    let service_path = if a.no_service {
        None
    } else {
        let exe = std::env::current_exe()
            .map_err(|e| CuaError::Internal(format!("this cua's path: {e}")))?;
        let spec = cua_ci::service::ServiceSpec {
            pool: name.clone(),
            exe,
            cua_home: std::env::var_os("CUA_HOME")
                .filter(|v| !v.is_empty())
                .map(|_| cua_home::cua_home()),
            log: paths.serve_log(&name),
        };
        Some(cua_ci::service::install(&spec).map_err(err)?)
    };
    let st = cua_ci::pool::status(&cfg, paths, cua_ci::unix_ms());
    if json {
        line(
            out,
            serde_json::json!({
                "pool": name,
                "config": config_path,
                "credential": keys.describe(service, &name),
                "service": service_path,
                "runs_on": st.labels,
                "warnings": warnings,
            })
            .to_string(),
        );
        return Ok(0);
    }
    line(out, format!("Runner pool {name} ({scope}) is set up."));
    line(out, format!("  configuration  {}", config_path.display()));
    line(
        out,
        format!("  credential     {}", keys.describe(service, &name)),
    );
    match &service_path {
        Some(p) => line(out, format!("  service        {} (running)", p.display())),
        None => line(
            out,
            format!("  service        none: run `cua ci serve --pool {name}`"),
        ),
    }
    if let Some(p) = &a.private_key_file {
        line(
            out,
            format!(
                "The key is stored now; delete {} (it is not needed again).",
                p.display()
            ),
        );
    }
    if let Some(p) = &a.key_file {
        line(
            out,
            format!(
                "The pool reads the key from {} on every start; keep it there, mode 0600.",
                p.display()
            ),
        );
    }
    line(out, "Use it from a workflow:");
    line(out, format!("  runs-on: [{}]", st.labels.join(", ")));
    for w in warnings {
        line(out, format!("Warning: {w}"));
    }
    Ok(0)
}

async fn serve(
    paths: &CiPaths,
    pool: &str,
    cua: Arc<cua_sdk::Cua>,
    once: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let cfg = load(paths, pool)?;
    let gh = Arc::new(github(&cfg, paths)?);
    let tarballs = Arc::new(cua_ci::cache::TarballCache::new(paths.cache(), gh.clone()));
    let deps = PoolDeps {
        spaces: Arc::new(crate::ci_sdk::SdkSpaces::new(cua)),
        github: gh,
        tarballs,
        clock: Arc::new(SystemClock),
        host: HostResources::detect(),
    };
    // The performance core of an own-app pool (`<pool>/perf.toml`): on unless
    // the file says otherwise (or `CUA_CI_PERF=off`); warm VMs register ahead
    // of time.
    cua_ci::perf::job_runner::ensure_default_perf_file(paths, pool).map_err(err)?;
    let perf = {
        let class = cua_ci::perf::job_runner::PerfClass {
            image: cua_ci::job_runner::ClassImage {
                class: if cfg.os == Some(cua_ci::GuestOs::Linux) {
                    cua_ci::service_api::Class::Linux
                } else {
                    cua_ci::service_api::Class::Macos
                },
                image: cfg.image.clone(),
                cpus: cfg.cpus,
                memory_mb: cfg.memory_mb,
                disk_gb: cfg.disk_gb,
            },
            arch: if cfg!(target_arch = "aarch64") {
                "arm64"
            } else {
                "x64"
            }
            .to_string(),
            slots: cfg.max_runners,
            warm: cfg.warm.unwrap_or(0),
        };
        cua_ci::perf::job_runner::pool_runtime(paths, pool, &class, deps.tarballs.as_ref(), true)
            .await
            .map_err(err)?
    };
    let mut opened = Pool::open(cfg.clone(), paths.clone(), deps).map_err(err)?;
    if let Some(rt) = perf {
        opened = opened.with_perf(rt);
    }
    let p = Arc::new(opened);
    if once {
        p.tick_and_provision().await;
        let st = cua_ci::pool::status(&cfg, paths, cua_ci::unix_ms());
        line(
            out,
            format!(
                "{pool}: {} runner(s): {}",
                st.slots.len(),
                st.counts
                    .iter()
                    .map(|(k, v)| format!("{v} {k}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
        return Ok(0);
    }
    line(
        out,
        format!(
            "Serving runner pool {pool} ({}); events in {}. Ctrl-C to stop.",
            cfg.scope,
            paths.events(pool).display()
        ),
    );
    let _ = out.flush();
    p.serve(shutdown_signal()).await.map_err(err)?;
    Ok(0)
}

/// `cua ci cache clear`: deletes `<pool>/perf/cache` of every pool (or one)
/// and, for every pool, the shared repository mirrors. Only folders under
/// the CI home are touched.
fn cache_clear(
    paths: &CiPaths,
    pool: Option<String>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let mut cleared: Vec<String> = Vec::new();
    let mut remove = |p: std::path::PathBuf, what: String| -> Result<(), CuaError> {
        if p.is_dir() {
            cua_home::guard_write(&p).map_err(|e| CuaError::Internal(e.to_string()))?;
            std::fs::remove_dir_all(&p)
                .map_err(|e| CuaError::Internal(format!("{}: {e}", p.display())))?;
            cleared.push(what);
        }
        Ok(())
    };
    let names: Vec<String> = match &pool {
        Some(p) => {
            if p.contains('/') || p.contains("..") || p.is_empty() {
                return Err(CuaError::InvalidArgument(format!(
                    "{p:?} is not a pool name"
                )));
            }
            vec![p.clone()]
        }
        None => std::fs::read_dir(paths.root())
            .map(|d| {
                d.filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default(),
    };
    for n in &names {
        remove(
            paths.pool_dir(n).join("perf").join("cache"),
            format!("{n}: dependency caches"),
        )?;
    }
    if pool.is_none() {
        remove(
            paths.root().join("perf").join("mirrors"),
            "repository mirrors".into(),
        )?;
    }
    if json {
        line(out, serde_json::json!({"cleared": cleared}).to_string());
    } else if cleared.is_empty() {
        line(out, "Nothing to clear: no caches here.");
    } else {
        for c in &cleared {
            line(out, format!("Cleared {c}."));
        }
    }
    Ok(0)
}

pub(crate) async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                match term.as_mut() {
                    Some(t) => { t.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn dur(ms: Option<u64>) -> String {
    match ms {
        None => "-".into(),
        Some(ms) if ms < 60_000 => format!("{:.1}s", ms as f64 / 1000.0),
        Some(ms) if ms < 3_600_000 => format!("{}m{:02}s", ms / 60_000, (ms / 1000) % 60),
        Some(ms) => format!("{}h{:02}m", ms / 3_600_000, (ms / 60_000) % 60),
    }
}

fn status(
    paths: &CiPaths,
    pool: Option<String>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let pools = pools_for(paths, pool)?;
    let now = cua_ci::unix_ms();
    let mut all = Vec::new();
    for p in &pools {
        let cfg = load(paths, p)?;
        let st = cua_ci::pool::status(&cfg, paths, now);
        let running = cua_ci::service::is_running(p);
        all.push((st, running));
    }
    if json {
        let v: Vec<serde_json::Value> = all
            .iter()
            .map(|(st, running)| {
                let mut v = serde_json::to_value(st).unwrap_or_default();
                v["service_running"] = serde_json::Value::Bool(*running);
                v
            })
            .collect();
        line(out, serde_json::Value::Array(v).to_string());
        return Ok(0);
    }
    if all.is_empty() {
        line(out, "No runner pools. Connect with `cua ci connect`.");
        return Ok(0);
    }
    for (st, running) in all {
        let s = &st.last_7_days;
        line(
            out,
            format!(
                "Pool {} ({})  service: {}",
                st.pool,
                st.scope,
                if running { "running" } else { "stopped" }
            ),
        );
        line(
            out,
            format!(
                "  {} on {}, at most {} runner(s), {} warm",
                st.image, st.on, st.max_runners, st.warm
            ),
        );
        line(out, format!("  runs-on  [{}]", st.labels.join(", ")));
        line(out, format!("  egress   {}", st.egress));
        let counts: Vec<String> = st.counts.iter().map(|(k, v)| format!("{v} {k}")).collect();
        line(
            out,
            format!(
                "  runners  {}",
                if counts.is_empty() {
                    "none".into()
                } else {
                    counts.join(", ")
                }
            ),
        );
        if let Some(until) = st.backoff_until_ms {
            line(
                out,
                format!(
                    "  backing off for {} after failures",
                    dur(Some(until.saturating_sub(now)))
                ),
            );
        }
        line(
            out,
            format!(
                "  last 7 days: {} job(s) ({} not succeeded), {} failure(s), {} wait(s) for a free \
                 VM, {} timeout(s), {} orphan(s) removed",
                s.jobs_finished,
                s.jobs_not_succeeded,
                s.failures,
                s.capacity_waits,
                s.job_timeouts,
                s.orphans_removed
            ),
        );
        line(
            out,
            format!(
                "  create to ready p50 {} p95 {}; idle wait p50 {}; job p50 {} p95 {}",
                dur(s.create_to_ready.p50_ms),
                dur(s.create_to_ready.p95_ms),
                dur(s.idle_wait.p50_ms),
                dur(s.job_duration.p50_ms),
                dur(s.job_duration.p95_ms)
            ),
        );
        if let Some((at, e)) = &s.last_error {
            line(
                out,
                format!(
                    "  last error ({} ago): {e}",
                    dur(Some(now.saturating_sub(*at)))
                ),
            );
        }
    }
    Ok(0)
}

fn ls(
    paths: &CiPaths,
    pool: Option<String>,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let now = cua_ci::unix_ms();
    let mut rows = Vec::new();
    for p in pools_for(paths, pool)? {
        let st = cua_ci::pool::PoolState::load(&paths.state(&p));
        for s in st.slots {
            rows.push((p.clone(), s));
        }
    }
    if json {
        let v: Vec<serde_json::Value> = rows
            .iter()
            .map(|(p, s)| {
                let mut v = serde_json::to_value(s).unwrap_or_default();
                v["pool"] = serde_json::Value::String(p.clone());
                v
            })
            .collect();
        line(out, serde_json::Value::Array(v).to_string());
        return Ok(0);
    }
    if rows.is_empty() {
        line(out, "No runners.");
        return Ok(0);
    }
    line(
        out,
        format!(
            "{:<10} {:<9} {:<24} {:<9} {:>8}  SPACE",
            "POOL", "SLOT", "RUNNER", "STATE", "AGE"
        ),
    );
    for (p, s) in rows {
        line(
            out,
            format!(
                "{:<10} {:<9} {:<24} {:<9} {:>8}  {}",
                p,
                s.id,
                s.name,
                s.phase.as_str(),
                dur(Some(now.saturating_sub(s.created_ms))),
                s.space_id.as_deref().unwrap_or("-")
            ),
        );
    }
    Ok(0)
}

fn logs(
    paths: &CiPaths,
    pool: Option<String>,
    slot: Option<String>,
    n: usize,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let pool = pick_pool(paths, pool)?;
    if let Some(slot) = slot {
        let dir = paths.slot_logs(&pool, &slot);
        if !dir.is_dir() {
            return Err(CuaError::NotFound(format!(
                "no logs for runner {slot} of {pool} ({}): they are copied when its Space is \
                 deleted, and kept {} days",
                dir.display(),
                load(paths, &pool)
                    .map(|c| c.log_retention_days)
                    .unwrap_or(7)
            )));
        }
        let mut files: Vec<PathBuf> = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    files.push(p);
                }
            }
        }
        files.sort();
        if json {
            line(
                out,
                serde_json::json!({ "dir": dir, "files": files }).to_string(),
            );
            return Ok(0);
        }
        let console = dir.join("cua-run.log");
        if let Ok(t) = std::fs::read_to_string(&console) {
            line(out, format!("== {}", console.display()));
            let _ = out.write_all(t.as_bytes());
        }
        line(
            out,
            format!("Runner diagnostics in {}:", dir.join("_diag").display()),
        );
        for f in files.iter().filter(|f| **f != console) {
            line(out, format!("  {}", f.display()));
        }
        return Ok(0);
    }
    let events = cua_ci::EventLog::new(paths.events(&pool)).tail(n);
    if json {
        line(out, serde_json::to_string(&events).unwrap_or_default());
        return Ok(0);
    }
    if events.is_empty() {
        line(
            out,
            format!(
                "No events yet for {pool} ({}).",
                paths.events(&pool).display()
            ),
        );
        return Ok(0);
    }
    for e in events {
        let mut parts = vec![
            humantime::format_rfc3339_seconds(
                std::time::UNIX_EPOCH + std::time::Duration::from_millis(e.ts_ms),
            )
            .to_string(),
            serde_json::to_value(e.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
        ];
        if let Some(s) = &e.slot {
            parts.push(format!("slot={s}"));
        }
        if let Some(r) = &e.runner {
            parts.push(format!("runner={r}"));
        }
        if let Some(r) = &e.reason {
            parts.push(format!("reason={r}"));
        }
        if let Some(j) = &e.job {
            parts.push(format!("job={j:?}"));
        }
        if let Some(r) = &e.result {
            parts.push(format!("result={r}"));
        }
        if e.ms.is_some() {
            parts.push(dur(e.ms));
        }
        if let Some(d) = &e.detail {
            parts.push(d.clone());
        }
        line(out, parts.join("  "));
    }
    line(out, format!("Runner logs: {}", paths.logs(&pool).display()));
    Ok(0)
}

async fn remove(
    paths: &CiPaths,
    pool: &str,
    cua: Arc<cua_sdk::Cua>,
    yes: bool,
    json: bool,
    out: &mut dyn Write,
) -> Result<i32, CuaError> {
    let cfg = load(paths, pool)?;
    if !yes {
        return Err(CuaError::Internal("remove runs after its preflight".into()));
    }
    let service_removed = cua_ci::service::uninstall(pool).map_err(err)?;
    let gh: Arc<dyn cua_ci::RunnerApi> = match github(&cfg, paths) {
        Ok(g) => Arc::new(g),
        Err(e) => {
            eprintln!("Warning: {e}; its runners stay registered at GitHub (remove them there).");
            Arc::new(cua_ci::github::Unavailable(e.to_string()))
        }
    };
    let deps = PoolDeps {
        spaces: Arc::new(crate::ci_sdk::SdkSpaces::new(cua)),
        github: gh,
        tarballs: Arc::new(cua_ci::cache::TarballCache::new(
            paths.cache(),
            Arc::new(cua_ci::github::Unavailable("not needed".into())),
        )),
        clock: Arc::new(SystemClock),
        host: None,
    };
    let p = Pool::open(cfg.clone(), paths.clone(), deps).map_err(err)?;
    let report = p.remove_all().await.map_err(err)?;
    let keys = store(paths, cfg.github.secret_store);
    let _ = keys.delete(&cfg.github.secret_service, &cfg.github.secret_account);
    let config = paths.config(pool);
    cua_home::guard_write(&config).map_err(|e| CuaError::PermissionDenied(e.to_string()))?;
    let _ = std::fs::remove_file(config);
    let dir = paths.pool_dir(pool);
    cua_home::guard_write(&dir).map_err(|e| CuaError::PermissionDenied(e.to_string()))?;
    let _ = std::fs::remove_dir_all(&dir);
    if json {
        line(
            out,
            serde_json::json!({
                "pool": pool,
                "service_removed": service_removed,
                "slots": report.slots,
                "jobs_stopped": report.busy_stopped,
                "orphans": report.orphans,
            })
            .to_string(),
        );
        return Ok(0);
    }
    line(
        out,
        format!(
            "Removed runner pool {pool}: {} runner(s){}{}, its configuration and credential.",
            report.slots,
            if report.busy_stopped > 0 {
                format!(" ({} job(s) stopped)", report.busy_stopped)
            } else {
                String::new()
            },
            if report.orphans > 0 {
                format!(", {} leftover Space(s) or runner(s)", report.orphans)
            } else {
                String::new()
            }
        ),
    );
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[derive(clap::Parser)]
    struct T {
        #[command(subcommand)]
        cmd: CiCmd,
    }

    fn parse(args: &[&str]) -> Result<CiCmd, clap::Error> {
        let mut v = vec!["t"];
        v.extend_from_slice(args);
        T::try_parse_from(v).map(|t| t.cmd)
    }

    fn connect_args(args: &[&str]) -> ConnectArgs {
        let mut v = vec!["connect"];
        v.extend_from_slice(args);
        let CiCmd::Connect(a) = parse(&v).unwrap() else {
            panic!()
        };
        *a
    }

    #[test]
    fn connect_defaults_to_the_hosted_service() {
        let a = connect_args(&[]);
        assert!(!a.own_app && !a.open_pr && !a.yes);
        assert!(a.allow.is_empty() && a.team.is_none());
        assert_eq!(a.own.on, "local");
        assert!(own_app_flags(&a.own).is_empty());
        let a = connect_args(&[
            "--allow",
            "acme/app",
            "--allow",
            "acme/api",
            "--open-pr",
            "--workflows",
            ".github/workflows/ci.yml",
            "--team",
            "acme",
            "--macos-slots",
            "1",
            "--org",
            "acme",
        ]);
        assert_eq!(a.allow, ["acme/app", "acme/api"]);
        assert!(a.open_pr);
        assert_eq!(a.workflows, [".github/workflows/ci.yml"]);
        assert_eq!(a.macos_slots, Some(1));
        assert_eq!(a.own.org.as_deref(), Some("acme"));
        assert!(
            own_app_flags(&a.own).is_empty(),
            "--org is the install hint"
        );
    }

    #[test]
    fn own_app_keeps_the_scope_and_credential_rules() {
        assert!(parse(&["connect", "--own-app", "--org", "a", "--repo", "a/b"]).is_err());
        assert!(
            parse(&[
                "connect",
                "--own-app",
                "--org",
                "a",
                "--private-key-stdin",
                "--token-stdin"
            ])
            .is_err()
        );
        let a = connect_args(&[
            "--own-app",
            "--org",
            "trycua",
            "--labels",
            "cua-ci,mac",
            "--app-id",
            "7",
            "--private-key-file",
            "k.pem",
        ]);
        assert!(a.own_app);
        assert_eq!(a.own.labels, ["cua-ci", "mac"]);
        assert_eq!(a.own.max, 2);
        assert_eq!(a.own.image, cua_ci::config::DEFAULT_MACOS_IMAGE);
        assert_eq!(a.own.on, "local");
        assert_eq!(
            own_app_flags(&a.own),
            ["--labels", "--app-id", "--private-key-file"]
        );
    }

    #[test]
    fn config_flags_parse_and_check_their_words() {
        let CiCmd::Config(c) = parse(&[
            "config",
            "--allow",
            "a/b",
            "--fork-prs",
            "deny",
            "--max-jobs",
            "4",
            "--budget",
            "50",
            "--on-budget-limit",
            "warn",
            "--egress",
            "off",
            "--yes",
            "--repo",
            "a/b",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!(c.allow, ["a/b"]);
        assert_eq!(c.fork_prs.as_deref(), Some("deny"));
        assert_eq!(c.max_jobs.as_deref(), Some("4"));
        assert_eq!(c.budget.as_deref(), Some("50"));
        assert!(c.yes);
        assert!(parse(&["config", "--fork-prs", "open"]).is_err());
        assert!(parse(&["config", "--on-budget-limit", "explode"]).is_err());
        assert!(parse(&["config", "--budget-scope", "x"]).is_err());
        assert!(parse(&["config", "--egress", "x"]).is_err());
        let CiCmd::Config(none) = parse(&["config"]).unwrap() else {
            panic!()
        };
        assert!(none.allow.is_empty() && none.budget.is_none() && !none.yes);
    }

    #[test]
    fn disconnect_logs_and_status_parse() {
        let CiCmd::Disconnect {
            repo,
            all_machines,
            unlink,
            uninstall,
            pool,
            yes,
        } = parse(&[
            "disconnect",
            "--repo",
            "a/b",
            "--all-machines",
            "--unlink",
            "-y",
        ])
        .unwrap()
        else {
            panic!()
        };
        assert_eq!(repo, ["a/b"]);
        assert!(all_machines && unlink && yes && !uninstall && pool.is_none());
        let CiCmd::Logs {
            job,
            follow,
            limit,
            pool,
            ..
        } = parse(&["logs", "99/27", "-f"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(job.as_deref(), Some("99/27"));
        assert!(follow && pool.is_none());
        assert_eq!(limit, 30);
        assert!(matches!(
            parse(&["status", "--pool", "trycua"]).unwrap(),
            CiCmd::Status { pool: Some(p) } if p == "trycua"
        ));
        // `setup`, `remove` and `connect --pool` never shipped: gone.
        assert!(parse(&["setup", "--org", "a"]).is_err());
        assert!(parse(&["remove", "--pool", "a"]).is_err());
    }

    #[test]
    fn add_and_remove_take_a_target_and_one_optional_address() {
        let CiCmd::Add(a) = parse(&["add", "aws"]).unwrap() else {
            panic!()
        };
        assert_eq!(a.target, "aws");
        assert!(a.budget.is_none() && a.max_per_hour.is_none() && !a.yes && !a.macos);
        assert!(
            a.warm.is_none() && a.goal.is_none(),
            "the autopilot chooses unless told"
        );
        let CiCmd::Add(a) = parse(&["add", "aws", "--warm", "2"]).unwrap() else {
            panic!()
        };
        assert_eq!(a.warm, Some(2));
        let CiCmd::Add(a) = parse(&["add", "aws", "--goal", "cheapest", "--warm", "0"]).unwrap()
        else {
            panic!()
        };
        assert_eq!((a.goal.as_deref(), a.warm), (Some("cheapest"), Some(0)));
        assert!(
            parse(&["add", "aws", "--goal", "quick"]).is_err(),
            "only the three goals"
        );
        assert!(matches!(
            parse(&["why"]).unwrap(),
            CiCmd::Why { target: None }
        ));
        let CiCmd::Why { target } = parse(&["why", "aws"]).unwrap() else {
            panic!()
        };
        assert_eq!(target.as_deref(), Some("aws"));
        let CiCmd::Add(a) = parse(&["add", "aws"]).unwrap() else {
            panic!()
        };
        assert_eq!(a.target, "aws");
        let CiCmd::Add(a) = parse(&[
            "add",
            "aws",
            "--budget",
            "80",
            "--max-per-hour",
            "3.5",
            "--macos",
            "--region",
            "us-west-2",
            "-y",
            "--account-id",
            "123456789012",
        ])
        .unwrap() else {
            panic!()
        };
        assert_eq!((a.budget, a.max_per_hour), (Some(80.0), Some(3.5)));
        assert!(a.macos && a.yes);
        let add = a.into_add();
        assert_eq!(add.region.as_deref(), Some("us-west-2"));
        assert_eq!(add.account_id.as_deref(), Some("123456789012"));
        let CiCmd::Add(a) = parse(&["add", "host", "root@box.example.com"]).unwrap() else {
            panic!()
        };
        assert_eq!(a.target, "host");
        assert_eq!(a.address.as_deref(), Some("root@box.example.com"));
        assert!(parse(&["add"]).is_err());
        assert!(parse(&["add", "aws", "--budget", "lots"]).is_err());
        let CiCmd::Remove {
            target,
            address,
            yes,
        } = parse(&["remove", "host", "root@box", "--yes"]).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            (target.as_str(), address.as_deref(), yes),
            ("host", Some("root@box"), true)
        );
        assert!(parse(&["remove"]).is_err());
    }

    #[test]
    fn connect_can_chain_a_cloud() {
        let a = connect_args(&["--cloud", "aws"]);
        assert_eq!(a.cloud.as_deref(), Some("aws"));
        assert!(needs_spaces(
            &parse(&["connect", "--cloud", "aws"]).unwrap()
        ));
        assert!(!needs_spaces(
            &parse(&["connect", "--cloud", "hetzner"]).unwrap()
        ));
        assert!(!needs_spaces(&parse(&["connect"]).unwrap()));
        assert!(needs_spaces(&parse(&["add", "gcp"]).unwrap()));
        assert!(!needs_spaces(&parse(&["add", "hetzner"]).unwrap()));
        assert!(!needs_spaces(&parse(&["add", "host", "root@box"]).unwrap()));
        assert!(!needs_spaces(&parse(&["remove", "aws"]).unwrap()));
    }

    #[test]
    fn only_serve_and_a_full_disconnect_need_spaces() {
        assert!(needs_spaces(&parse(&["serve"]).unwrap()));
        assert!(needs_spaces(&parse(&["disconnect"]).unwrap()));
        assert!(!needs_spaces(
            &parse(&["disconnect", "--repo", "a/b"]).unwrap()
        ));
        assert!(!needs_spaces(&parse(&["status"]).unwrap()));
        assert!(!needs_spaces(&parse(&["config"]).unwrap()));
        assert!(!needs_spaces(&parse(&["connect"]).unwrap()));
    }

    #[test]
    fn hosted_is_the_target_only_when_connected_or_named() {
        let d = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(d.path());
        assert!(!hosted_target(&paths, None));
        assert!(hosted_target(&paths, Some("hosted")));
        assert!(!hosted_target(&paths, Some("trycua")));
        let s = cua_ci::hosted::HostedState {
            account: "me".into(),
            installation: "inst_1".into(),
            github_login: String::new(),
            machine_id: "m".into(),
            machine_name: "n".into(),
            macos_slots: 1,
            linux_slots: 0,
            macos_image: None,
            linux_image: None,
            on: "local".into(),
            connected_at: String::new(),
        };
        s.save(&paths).unwrap();
        assert!(hosted_target(&paths, None));
        assert!(!hosted_target(&paths, Some("trycua")));
    }

    #[test]
    fn the_hosted_pool_name_cannot_be_taken_by_a_pool() {
        assert_eq!(crate::ci_hosted::HOSTED_POOL, "hosted");
    }

    #[test]
    fn durations_read_well() {
        assert_eq!(dur(Some(19_200)), "19.2s");
        assert_eq!(dur(Some(242_000)), "4m02s");
        assert_eq!(dur(Some(3 * 3_600_000 + 60_000)), "3h01m");
        assert_eq!(dur(None), "-");
    }

    #[test]
    fn a_pool_is_picked_by_name_or_alone() {
        let d = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(d.path());
        assert!(matches!(
            pick_pool(&paths, None),
            Err(CuaError::NotFound(_))
        ));
        std::fs::write(paths.config("a"), "").unwrap();
        assert_eq!(pick_pool(&paths, None).unwrap(), "a");
        std::fs::write(paths.config("b"), "").unwrap();
        assert!(
            matches!(pick_pool(&paths, None), Err(CuaError::InvalidArgument(m)) if m.contains("a, b"))
        );
        assert_eq!(pick_pool(&paths, Some("b".into())).unwrap(), "b");
        assert!(matches!(
            pick_pool(&paths, Some("c".into())),
            Err(CuaError::NotFound(_))
        ));
    }
}
