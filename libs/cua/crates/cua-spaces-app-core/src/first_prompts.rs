// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The example jobs the app hands a new user: the prompt to copy once a coding
//! app is connected (onboarding page 2 shows one fragment of the first as a sentence), and
//! the suggestion cards on an empty Home. One library, one capability filter:
//! every surface draws from [`LIBRARY`] through [`choose`], so no shell keeps a
//! sentence of its own and none can offer a job this machine cannot do.
//!
//! The point of an example is inspiration: each one is a job people really
//! have (see [`FAMILIES`] for where each need came from), written the way an
//! agent can finish it with Cua: make Spaces, work in them, compare, report
//! with screenshots, delete them. X
//!
//! The rules:
//! - Each entry names what it needs ([`Cap`]). A host gets an entry only when
//!   it has all of it, from the facts the New Space wizard already reads
//!   ([`crate::wizard::first_space_offers`], the runtime doctor, storage).
//! - A copyable example needs no repo and no account, uses a public page that
//!   needs no login, and names no issue number or repository.
//! - A Home card may end with ": " to be filled in through the box (the
//!   "Add a repo" pattern); it needs [`Cap::FillIn`], and [`Cap::Account`] when
//!   a site may ask for a login.
//! - [`FALLBACK`] needs only a Linux Space and works on every host (default
//!   secure network: internet and child Spaces, nothing else).

use crate::model::SpaceOs;
use crate::wizard::{FirstSpaceOffer, WizardEnv, first_space_offers, local_microvm_runtimes};
use serde::{Deserialize, Serialize};

/// What an example needs from the machine or the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cap {
    /// A Linux Space (every host can make one, or asks a machine that can).
    LinuxSpace,
    /// A macOS Space, made here now (Apple silicon, runtime ready, room).
    MacosSpace,
    /// A Windows Space, made here now (QEMU ready, room).
    WindowsSpace,
    /// MicroVMs: many small Spaces at once (KVM or Apple's container services).
    Kvm,
    /// A fill-in the person completes in the box: a project, a link, a site or a place.
    FillIn,
    /// The person's own account, signed in once with their approval.
    Account,
}

/// What this machine can do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCaps {
    pub macos: bool,
    pub windows: bool,
    pub kvm: bool,
    /// The macOS image is already downloaded here.
    #[serde(default)]
    pub macos_cached: bool,
    /// The Windows image is already downloaded here.
    #[serde(default)]
    pub windows_cached: bool,
}

impl HostCaps {
    /// Whether the entry needs a macOS or Windows image that is not here yet,
    /// so its first run downloads one.
    fn downloads(&self, e: &Entry) -> bool {
        (e.needs.contains(&Cap::MacosSpace) && !self.macos_cached)
            || (e.needs.contains(&Cap::WindowsSpace) && !self.windows_cached)
    }

    fn has(&self, c: Cap) -> bool {
        match c {
            Cap::LinuxSpace | Cap::FillIn | Cap::Account => true,
            Cap::MacosSpace => self.macos,
            Cap::WindowsSpace => self.windows,
            Cap::Kvm => self.kvm,
        }
    }
}

/// One example in the library.
pub struct Entry {
    pub id: &'static str,
    /// The workflow family ([`FAMILIES`]).
    pub family: &'static str,
    pub needs: &'static [Cap],
    /// One plain line: why this is useful.
    pub why: &'static str,
    /// The example sentence onboarding shows ("things like ..."): only the bug-on-platform entries have one.
    pub lede: &'static str,
    /// The card's title.
    pub title: &'static str,
    /// The card's line under the title.
    pub detail: &'static str,
    /// The prompt. A card's ends with ": " when the person fills in the rest.
    pub text: &'static str,
    /// Home card order (higher first); 0 is never a card.
    pub card: u8,
}

/// The workflows the examples come from, with where each need came from. The
/// starters of other assistant apps could not be read verbatim (their pages
/// were not reachable), so a note says what is known of them, not their words.
pub const FAMILIES: &[(&str, &str)] = &[
    (
        "shopping-research",
        "Muse's starter tasks are buying groceries and comparing options (Meta's Muse post, mobile-rn REFERENCES 1.2); Dots research with visible updates (1.3); the first-run price-research jobs people ask coding apps for",
    ),
    (
        "trip-planning",
        "Muse books restaurant tables and plans from live availability (REFERENCES 1.2); Dots plans end to end and stops for approval (1.3)",
    ),
    (
        "orders-claims-forms",
        "FRICTION item 1 (site sign-in through the Keyvault, not a personal browser profile); Muse's approval card before checkout and its client-owned password entry (REFERENCES 1.2)",
    ),
    (
        "paperwork",
        "Keyvault and Cua Volume guides (files an agent collects and keeps); Muse's inspect and download files (REFERENCES 1.2)",
    ),
    (
        "research-brief",
        "Dots research with sources and visible updates (REFERENCES 1.3); the provenance-review retro (clone, scan, report with evidence)",
    ),
    (
        "watch-and-tell",
        "Schedules guide (recurring work whether or not an app is open); Keep an agent running guide; Dots always-on agents (REFERENCES 1.3)",
    ),
    (
        "two-space-dev",
        "Boat head-to-head retro (many jobs, two kinds of computer); Child Spaces guide (a Space makes Spaces); E2B-migration prospect running many environments",
    ),
    (
        "cross-platform-verify",
        "Public issues that name their platform (Windows 11 Edge, macOS 27, Fedora KDE Wayland); the Home Safari card; release-verification retro (clean Mac, clean Windows)",
    ),
    (
        "bug-on-platform",
        "Public bug reports each name the platform they happen on; the Hermes plugin retro (read the report, reproduce, fix, test, release)",
    ),
    (
        "untrusted-code",
        "Cua CI guide (a fresh Space per job, deleted afterwards); the provenance-review retro; the secure default Space (no host, no LAN)",
    ),
    (
        "long-job",
        "Keep an agent running guide; FRICTION item 7 (a laptop's network change pausing a task); the durable-agent battle tests",
    ),
    (
        "fan-out-research",
        "Child Spaces guide; Boat head-to-head retro (80 jobs at once); E2B-migration (256 environments)",
    ),
];

const L: Cap = Cap::LinuxSpace;
const M: Cap = Cap::MacosSpace;
const W: Cap = Cap::WindowsSpace;

/// The library. Order matters inside a family and for copyable examples of
/// equal reach: earlier is preferred. No entry names an issue number, a
/// repository, or a system its `needs` do not list; a login is always asked
/// for through the Keyvault or the visible browser window; none merely opens a
/// public page and describes it (tests hold all of this).
pub const LIBRARY: &[Entry] = &[
    Entry {
        id: "price-compare",
        lede: "",
        family: "shopping-research",
        needs: &[L],
        why: "Real research with prices, reviews and screenshots, done while you do something else.",
        title: "Research a purchase across several stores",
        detail: "It compares prices and reviews and writes you a buying guide with sources.",
        text: "Make a fresh Linux Space with Cua and use its browser to research the three best-reviewed noise cancelling headphones under 300 dollars right now. Compare their prices at four stores, including the maker's own site, read the most recent reviews, and screenshot each price you rely on. Then write me a short buying guide with the sources linked. Do not buy anything, and do not sign in anywhere. Delete the Space when you are done.",
        card: 0,
    },
    Entry {
        id: "trip-weekend",
        lede: "",
        family: "trip-planning",
        needs: &[L],
        why: "A trip planned from live availability, with a stop before anything costs money.",
        title: "Plan a weekend from live availability",
        detail: "It compares hotels on several sites and writes a one page itinerary.",
        text: "Plan a long weekend in Lisbon for two, three weeks from now. In a fresh Linux Space with a browser, compare live hotel prices and availability for three nights on at least three booking sites, check which neighborhoods have the best recent reviews, and screenshot your top three options. Write me a one page itinerary with links and a total cost estimate. Stop and ask me before anything that costs money. Delete the Space when you are done.",
        card: 0,
    },
    Entry {
        id: "two-space-load",
        lede: "",
        family: "two-space-dev",
        needs: &[L],
        why: "A server and its users in two separate Spaces, which is how it fails for real.",
        title: "Load test a web app from a second Space",
        detail: "One Space runs the app, another one uses it and pushes it until it slows.",
        text: "Make two fresh Linux Spaces with Cua. In the first, write and start a small web app that serves a page with a thousand generated rows and a search box, and tell me its address inside the network. From the second Space, open it in a browser and use the search box, then load test it with a growing number of parallel requests until responses slow down. Send me the screenshots, a table of response times for each load level, and the point where it started to struggle. Delete both Spaces when you are done.",
        card: 0,
    },
    Entry {
        id: "fan-out-pricing",
        lede: "",
        family: "fan-out-research",
        needs: &[L, Cap::Kvm],
        why: "Ten Spaces at once, one tool each, one table back.",
        title: "Compare ten tools at once, one Space each",
        detail: "Ten Spaces open ten pricing pages side by side and you get one table.",
        text: "Compare the pricing of ten popular project management tools at the same time. Make ten fresh Linux Spaces with Cua, one per tool: Asana, Trello, Monday, ClickUp, Notion, Linear, Jira, Basecamp, Smartsheet and Wrike. In each, open the tool's pricing page, screenshot every plan and note its price per person per month, what the free plan includes, and any limit that matters for a team of ten. Give me one table with a link to each source and tell me which three fit a team of ten best. Do not sign up for anything. Delete the Spaces when you are done.",
        card: 0,
    },
    Entry {
        id: "cross-os-prices-macos",
        lede: "",
        family: "cross-platform-verify",
        needs: &[L, M],
        why: "The same shopping page on two systems, side by side, with screenshots.",
        title: "Compare store pages on Linux and macOS",
        detail: "It searches the same stores on both systems and shows what differs.",
        text: "Make a fresh Linux Space and a fresh macOS Space with Cua. In the browser of each (Safari on the Mac), open the same three large electronics stores, search for wireless earbuds under 100 dollars and screenshot the first page of results. Tell me where the prices, the ordering or the page itself differ between the two systems, and which store has the best deal today. Do not sign in or buy anything. Delete both Spaces when you are done.",
        card: 0,
    },
    Entry {
        id: "cross-os-prices-windows",
        lede: "",
        family: "cross-platform-verify",
        needs: &[L, W],
        why: "The same shopping page on two systems, side by side, with screenshots.",
        title: "Compare store pages on Linux and Windows",
        detail: "It searches the same stores on both systems and shows what differs.",
        text: "Make a fresh Linux Space and a fresh Windows Space with Cua. In the browser of each (Edge on Windows), open the same three large electronics stores, search for wireless earbuds under 100 dollars and screenshot the first page of results. Tell me where the prices, the ordering or the page itself differ between the two systems, and which store has the best deal today. Do not sign in or buy anything. Delete both Spaces when you are done.",
        card: 0,
    },
    Entry {
        id: "order-tracking",
        lede: "",
        family: "orders-claims-forms",
        needs: &[L, Cap::FillIn, Cap::Account],
        why: "Finds out where your order is without you opening a thing, and signs in without seeing a password.",
        title: "Find out where my order is",
        detail: "It signs in the safe way, reads the tracking page and tells you if it is late.",
        text: "Find out where my latest order is. In a fresh Linux Space with a browser, open the store site below. If it needs me to sign in, ask for the login through the Keyvault, or show me the browser window so I can sign in myself, and never ask me to type a password to you. Then tell me the order's status and expected delivery date, screenshot the tracking page, and say if anything looks late. The store: ",
        card: 85,
    },
    Entry {
        id: "claim-status",
        lede: "",
        family: "orders-claims-forms",
        needs: &[L, Cap::FillIn, Cap::Account],
        why: "Reads a claim or case page for you and lists what it is waiting on.",
        title: "Check on my claim or case",
        detail: "It reads the status and tells you what it is waiting for from you.",
        text: "Check on my open claim. In a fresh Linux Space with a browser, open the claims site below. If it needs me to sign in, ask for the login through the Keyvault, or show me the browser window so I can sign in myself, and never ask me to type a password to you. Then list what the claim is waiting on from me, the dates it shows and who to contact, with a screenshot of each page. Do not submit or change anything. The site: ",
        card: 45,
    },
    Entry {
        id: "fill-form",
        lede: "",
        family: "orders-claims-forms",
        needs: &[L, Cap::FillIn, Cap::Account],
        why: "Does the typing and stops before the part that cannot be undone.",
        title: "Fill in a form and stop before sending",
        detail: "It fills every page, shows you each one, and waits for your yes.",
        text: "Fill in the form at the address below with the details I give you in this chat. Use a fresh Linux Space with a browser. If it needs me to sign in, ask for the login through the Keyvault, or show me the browser window so I can sign in myself, and never ask me to type a password to you. Screenshot every page as you go, stop before the final submit, and ask me to confirm. The form: ",
        card: 40,
    },
    Entry {
        id: "receipts",
        lede: "",
        family: "paperwork",
        needs: &[L, Cap::FillIn, Cap::Account],
        why: "Gathers a year of receipts into one place and one spreadsheet.",
        title: "Collect my receipts into one place",
        detail: "It downloads your last receipts into your Cua Volume and lists them.",
        text: "Collect my receipts. In a fresh Linux Space with a browser, open the account page below. If it needs me to sign in, ask for the login through the Keyvault, or show me the browser window so I can sign in myself, and never ask me to type a password to you. Download the last twelve receipts or invoices into my Cua Volume, and make a spreadsheet with the date, vendor, amount and file name of each. The account: ",
        card: 55,
    },
    Entry {
        id: "book-table",
        lede: "",
        family: "trip-planning",
        needs: &[L, Cap::FillIn, Cap::Account],
        why: "Compares real availability and stops before booking.",
        title: "Find a table for Friday night",
        detail: "It compares booking sites and waits for your pick before booking.",
        text: "Find a table for four on Friday evening near the place below. In a fresh Linux Space with a browser, compare availability on at least two booking sites and pick the three best-reviewed places that have a table around 7 pm, with a screenshot of each. If it needs me to sign in, ask for the login through the Keyvault, or show me the browser window so I can sign in myself, and never ask me to type a password to you. Stop before booking anything and ask me which one. The place: ",
        // Not a Home card: restaurant booking is a cliché demo, not the dev work Spaces is for.
        card: 0,
    },
    Entry {
        id: "price-watch",
        lede: "",
        family: "watch-and-tell",
        needs: &[L, Cap::FillIn],
        why: "Keeps watching while your laptop is closed, and tells you only when it matters.",
        title: "Watch a page and tell me when it changes",
        detail: "It checks twice a day and messages you with a screenshot.",
        text: "Watch the product page below for me in a Linux Space that keeps running when I close my laptop. Check the price twice a day, and message me with a screenshot as soon as it falls below the price I tell you, or when something else that matters changes, like it going out of stock or a new model replacing it. The page: ",
        card: 65,
    },
    Entry {
        id: "research-brief",
        lede: "",
        family: "research-brief",
        needs: &[L, Cap::FillIn],
        why: "A sourced brief instead of ten open tabs.",
        title: "Research a topic with sources",
        detail: "It reads at least six sources and writes a two page brief with links.",
        text: "Research the topic below. In a fresh Linux Space with a browser, read at least six sources, screenshot the passage you rely on in each, and write me a two page brief that says what is agreed, what is disputed and what you could not confirm, with every claim linked to its source. The topic: ",
        card: 50,
    },
    Entry {
        id: "server-ui-load",
        lede: "",
        family: "two-space-dev",
        needs: &[L, Cap::FillIn],
        why: "Finds out how your app behaves under real users, from a separate Space.",
        title: "Run my app in one Space and load test it from another",
        detail: "It drives the screens in a browser and pushes the server until it slows.",
        text: "Run my app's server in one Space and test it from another. Make two fresh Linux Spaces, install what the project needs and start the server in the first, then from the second drive its main screens in a browser and load test it with a growing number of parallel users. Give me screenshots, a table of response times for each level, and where it starts to break. My project: ",
        card: 80,
    },
    Entry {
        id: "tests-linux-macos",
        lede: "",
        family: "cross-platform-verify",
        needs: &[L, M, Cap::FillIn],
        why: "Finds what only breaks on one system, in clean Spaces of each kind.",
        title: "Verify a change on Linux and on macOS",
        detail: "Clean Spaces of each kind run the same steps and you see what differs.",
        text: "Bring up my project's services in a clean Linux Space and run its tests there, then verify the same change on a fresh macOS Space with the same steps. Compare the two runs and show me anything that behaves differently, with logs and a screenshot where it matters. My project: ",
        card: 95,
    },
    Entry {
        id: "tests-linux-windows",
        lede: "",
        family: "cross-platform-verify",
        needs: &[L, W, Cap::FillIn],
        why: "Finds what only breaks on one system, in clean Spaces of each kind.",
        title: "Verify a change on Linux and on Windows",
        detail: "Clean Spaces of each kind run the same steps and you see what differs.",
        text: "Bring up my project's services in a clean Linux Space and run its tests there, then verify the same change on a fresh Windows Space with the same steps. Compare the two runs and show me anything that behaves differently, with logs and a screenshot where it matters. My project: ",
        card: 94,
    },
    Entry {
        id: "site-linux-macos",
        lede: "",
        family: "cross-platform-verify",
        needs: &[L, M, Cap::FillIn],
        why: "See your site the way each system's visitors do.",
        title: "Check my site on Linux and in Safari",
        detail: "It clicks through your site on both and lists every difference.",
        text: "Check how my site looks and works on a fresh Linux Space and in Safari on a fresh macOS Space. Click through the main pages and forms in each, take screenshots, and list every difference you find. My site: ",
        card: 88,
    },
    Entry {
        id: "site-linux-windows",
        lede: "",
        family: "cross-platform-verify",
        needs: &[L, W, Cap::FillIn],
        why: "See your site the way each system's visitors do.",
        title: "Check my site on Linux and in Edge",
        detail: "It clicks through your site on both and lists every difference.",
        text: "Check how my site looks and works on a fresh Linux Space and in Edge on a fresh Windows Space. Click through the main pages and forms in each, take screenshots, and list every difference you find. My site: ",
        card: 87,
    },
    Entry {
        id: "installer-linux-macos",
        lede: "",
        family: "cross-platform-verify",
        needs: &[L, M, Cap::FillIn],
        why: "Learn the installer fails before your users do.",
        title: "Build my installer and test it on Linux and macOS",
        detail: "It installs and starts the app in a clean Space of each kind and compares.",
        text: "Build my app's installer on a fresh Linux Space and on a fresh macOS Space, install it on each, start the app and smoke test its main flow, then compare the behavior between the two and show me a screenshot of each. My project: ",
        card: 86,
    },
    Entry {
        id: "installer-linux-windows",
        lede: "",
        family: "cross-platform-verify",
        needs: &[L, W, Cap::FillIn],
        why: "Learn the installer fails before your users do.",
        title: "Build my installer and test it on Linux and Windows",
        detail: "It installs and starts the app in a clean Space of each kind and compares.",
        text: "Build my app's installer on a fresh Linux Space and on a fresh Windows Space, install it on each, start the app and smoke test its main flow, then compare the behavior between the two and show me a screenshot of each. My project: ",
        card: 85,
    },
    Entry {
        id: "bug-macos",
        lede: "Reproduce this bug on a fresh macOS Space",
        family: "bug-on-platform",
        needs: &[M, Cap::FillIn],
        why: "Proves a fix on the platform the bug was reported on.",
        title: "Reproduce a bug on macOS, fix it, prove it",
        detail: "It follows the report on a fresh macOS Space and shows the fix there.",
        text: "Reproduce a bug a user reported on their platform, then fix it and prove the fix there. Make a fresh macOS Space, follow the report's steps exactly and screenshot what happens, make the fix in my project, and run the same steps again to show it is gone. The report: ",
        card: 90,
    },
    Entry {
        id: "bug-windows",
        lede: "Reproduce this bug on a fresh Windows Space",
        family: "bug-on-platform",
        needs: &[W, Cap::FillIn],
        why: "Proves a fix on the platform the bug was reported on.",
        title: "Reproduce a bug on Windows, fix it, prove it",
        detail: "It follows the report on a fresh Windows Space and shows the fix there.",
        text: "Reproduce a bug a user reported on their platform, then fix it and prove the fix there. Make a fresh Windows Space, follow the report's steps exactly and screenshot what happens, make the fix in my project, and run the same steps again to show it is gone. The report: ",
        card: 89,
    },
    Entry {
        id: "bug-linux",
        lede: "Reproduce this bug on a fresh Linux Space",
        family: "bug-on-platform",
        needs: &[L, Cap::FillIn],
        why: "Proves a fix in a clean Space, not just on your own computer.",
        title: "Reproduce a bug in a clean Space, fix it, prove it",
        detail: "It follows the report on a fresh Linux Space and shows the fix there.",
        text: "Reproduce a bug a user reported, then fix it and prove the fix in a clean Space. Make a fresh Linux Space, follow the report's steps exactly and screenshot what happens, make the fix in my project, and run the same steps again to show it is gone. The report: ",
        card: 35,
    },
    Entry {
        id: "untrusted-pr",
        lede: "",
        family: "untrusted-code",
        needs: &[L, Cap::FillIn],
        why: "See what unfamiliar code touches before it gets near your machine.",
        title: "Try an unfamiliar change in a clean Space",
        detail: "It builds it with no access to your machine and reports what it touches.",
        text: "Try an unfamiliar change in a clean Space. Make a fresh Linux Space with no access to my machine, check out the pull request or package below, build it and run its tests, and tell me every network address it contacted and every file it wrote outside its own folder, so I can decide whether to merge or install it. The link: ",
        card: 75,
    },
    Entry {
        id: "long-job",
        lede: "",
        family: "long-job",
        needs: &[L, Cap::FillIn],
        why: "A long job keeps going when you close the laptop, and finds you when it is done.",
        title: "Run a long job and hand it off if I close my laptop",
        detail: "It keeps going on another machine of yours and messages you when done.",
        text: "Run the job below in a Linux Space that keeps going if I close my laptop, and hand it over to another of my machines if this one goes to sleep. Check on it now and then and message me when it finishes or fails, with the last lines of its log. The job: ",
        // Not a Home card: handing a job to another machine hasn't shipped, so the card would promise it.
        card: 0,
    },
];

/// The example that works on every host: a Linux Space, one public page, no
/// repo, no account, nothing private.
pub const FALLBACK: &str = "price-compare";

/// The Windows image a Windows Space is made from.
const WINDOWS_IMAGE: &str = "ghcr.io/trycua/windows:2022";

/// What this machine can do now, from the facts the wizard reads.
pub fn host_caps(env: &WizardEnv, host_os: SpaceOs, offers: &[FirstSpaceOffer]) -> HostCaps {
    let macos = offers
        .iter()
        .any(|o| o.id == "macos" && o.blocked.is_none());
    // Windows runs in QEMU on a PC (a Mac does not offer it yet), once the
    // doctor and storage are read and there is room.
    let checked = env.storage.is_some() && env.local_backends.is_some();
    let local = WizardEnv {
        default_location: crate::model::Location::Local,
        ..env.clone()
    };
    let windows = checked
        && host_os != SpaceOs::Macos
        && crate::wizard::local_plan(&local, host_os, WINDOWS_IMAGE)
            .is_some_and(|(_, blocked)| blocked.is_none());
    let pulled = |r: &str| {
        env.storage
            .as_ref()
            .is_some_and(|st| st.pulled.iter().any(|p| p == r))
    };
    let macos_cached = macos
        && offers
            .iter()
            .any(|o| o.id == "macos" && o.size.as_deref() == Some("Already downloaded"));
    let windows_cached = windows && pulled(WINDOWS_IMAGE);
    let kvm = local_microvm_runtimes(env).is_some_and(|r| !r.is_empty());
    HostCaps {
        macos,
        windows,
        kvm,
        macos_cached,
        windows_cached,
    }
}

fn fits(e: &Entry, caps: &HostCaps) -> bool {
    e.needs.iter().all(|c| caps.has(*c))
}

fn platforms(e: &Entry) -> usize {
    e.needs
        .iter()
        .filter(|c| matches!(c, Cap::LinuxSpace | Cap::MacosSpace | Cap::WindowsSpace))
        .count()
}

fn copyable(e: &Entry) -> bool {
    !e.needs
        .iter()
        .any(|c| matches!(c, Cap::FillIn | Cap::Account))
}

/// The system an entry's icon shows: the system for a one-system job, Linux
/// for one that uses several.
fn icon_os(e: &Entry) -> SpaceOs {
    match (
        platforms(e),
        e.needs.contains(&Cap::MacosSpace),
        e.needs.contains(&Cap::WindowsSpace),
    ) {
        (1, true, _) => SpaceOs::Macos,
        (1, _, true) => SpaceOs::Windows,
        _ => SpaceOs::Linux,
    }
}

/// One example to copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Example {
    pub id: String,
    pub family: String,
    pub text: String,
    /// "Why this is useful", one line.
    pub why: String,
    pub needs: Vec<Cap>,
}

/// One card on an empty Home.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub id: String,
    pub family: String,
    pub title: String,
    pub detail: String,
    /// The system the job runs on (its icon).
    pub os: SpaceOs,
    /// What goes into the box when the card is picked; ends with ": " when
    /// the person fills in the rest.
    pub prompt: String,
    pub why: String,
    pub needs: Vec<Cap>,
}

/// Everything chosen for this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstPrompts {
    /// What this machine can do now.
    pub caps: HostCaps,
    /// Onboarding's one example sentence, chosen from the bug-on-platform entries:
    /// Windows where a Windows Space can be made, else macOS where a macOS Space
    /// can, else Linux. Never a system this machine cannot make.
    pub lede: String,
    /// The examples to copy, best fit first, one per family, with the
    /// fallback somewhere in it. Never empty. The first is the one onboarding shows; the rest
    /// are what the Agents page offers after it.
    pub examples: Vec<Example>,
    /// The cards for an empty Home, best fit first.
    pub suggestions: Vec<Suggestion>,
}

/// What a first run downloading an image adds to the why-line.
const DOWNLOADS: &str = " The first run downloads an image.";

fn why_of(e: &Entry, caps: &HostCaps) -> String {
    if caps.downloads(e) {
        format!("{}{DOWNLOADS}", e.why)
    } else {
        e.why.into()
    }
}

fn example(e: &Entry, caps: &HostCaps) -> Example {
    Example {
        id: e.id.into(),
        family: e.family.into(),
        text: e.text.into(),
        why: why_of(e, caps),
        needs: e.needs.to_vec(),
    }
}

/// How many cards Home shows.
pub const CARDS: usize = 3;

/// The examples and cards for a machine with `caps`.
pub fn choose_with(caps: HostCaps) -> FirstPrompts {
    // Copyable: jobs that need no download first (the most systems first, then library order,
    // so the plain fallback leads), then the ones that download a macOS or Windows image on
    // their first run, which lead as soon as the image is here; one per family so the list varies.
    let mut order: Vec<usize> = (0..LIBRARY.len())
        .filter(|&i| copyable(&LIBRARY[i]) && fits(&LIBRARY[i], &caps))
        .collect();
    order.sort_by_key(|&i| {
        (
            caps.downloads(&LIBRARY[i]),
            std::cmp::Reverse(platforms(&LIBRARY[i])),
            i,
        )
    });
    let mut seen: Vec<&str> = vec![];
    let mut examples: Vec<Example> = vec![];
    for i in order {
        let e = &LIBRARY[i];
        if e.id != FALLBACK && seen.contains(&e.family) {
            continue;
        }
        // The fallback shares its family with the two-system page jobs: it does not hide them.
        if e.id != FALLBACK {
            seen.push(e.family);
        }
        examples.push(example(e, &caps));
    }
    // The fallback is always in the list, whatever else fits.
    if !examples.iter().any(|x| x.id == FALLBACK)
        && let Some(f) = LIBRARY.iter().find(|e| e.id == FALLBACK)
    {
        examples.push(example(f, &caps));
    }
    // Cards: highest priority first among what fits, one per family.
    let mut cards: Vec<&Entry> = LIBRARY
        .iter()
        .filter(|e| e.card > 0 && fits(e, &caps))
        .collect();
    cards.sort_by_key(|e| std::cmp::Reverse(e.card));
    let mut seen: Vec<&str> = vec![];
    let mut suggestions = vec![];
    for e in cards {
        if seen.contains(&e.family) {
            continue;
        }
        seen.push(e.family);
        suggestions.push(Suggestion {
            id: e.id.into(),
            family: e.family.into(),
            title: e.title.into(),
            detail: e.detail.into(),
            os: icon_os(e),
            prompt: e.text.into(),
            why: why_of(e, &caps),
            needs: e.needs.to_vec(),
        });
        if suggestions.len() == CARDS {
            break;
        }
    }
    let lede = ["bug-windows", "bug-macos", "bug-linux"]
        .iter()
        .filter_map(|id| LIBRARY.iter().find(|e| e.id == *id))
        .find(|e| fits(e, &caps))
        .map(|e| e.lede)
        .unwrap_or("Reproduce this bug on a fresh Linux Space");
    FirstPrompts {
        caps,
        lede: lede.into(),
        examples,
        suggestions,
    }
}

/// The examples and cards for the machine described by `env`, running `host_os`.
pub fn choose_for(env: &WizardEnv, host_os: SpaceOs) -> FirstPrompts {
    let offers = first_space_offers(env, host_os);
    choose_with(host_caps(env, host_os, &offers))
}
