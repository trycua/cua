//! Friendly default names for Spaces: `brave-otter`, `quiet-heron`.
//!
//! One generator for every surface (the SDK, the CLI, MCP `create_space`,
//! the app and a Home start), so a Space made without a name reads the same
//! everywhere. A name is an adjective and an animal from two curated lists
//! (short, lowercase, DNS-safe, nothing a person could take badly). Names
//! the caller gives are never touched, and Spaces that exist keep theirs.
//!
//! The web app has the same lists as a generated file
//! (`apps/cua-spaces-web/src/lib/pet-names.generated.ts`), written by the
//! `pet_names_lists_match_the_web_copy` test's `CUA_UPDATE_PET_NAMES=1`
//! mode; the test fails when the two drift.

/// Adjectives: short, kind, neutral.
pub const ADJECTIVES: &[&str] = &[
    "agile", "amber", "ample", "bold", "brave", "breezy", "bright", "brisk", "calm", "candid",
    "careful", "clever", "cosmic", "cozy", "crisp", "curious", "dapper", "daring", "dewy", "deft",
    "eager", "early", "easy", "epic", "fair", "fancy", "fast", "fine", "fleet", "fond", "frosty",
    "gentle", "giant", "glad", "golden", "grand", "happy", "hardy", "honest", "humble", "jolly",
    "jovial", "keen", "kind", "lively", "lucky", "lunar", "mellow", "merry", "mighty", "mild",
    "misty", "modest", "nifty", "nimble", "noble", "novel", "open", "patient", "peppy", "placid",
    "plucky", "polite", "proud", "quick", "quiet", "rapid", "ready", "rosy", "royal", "rustic",
    "sandy", "serene", "shiny", "silent", "silver", "simple", "sleek", "smart", "smooth", "snowy",
    "solar", "solid", "spry", "steady", "stellar", "sunny", "super", "swift", "tidy", "tiny",
    "topaz", "true", "trusty", "upbeat", "vivid", "warm", "wise", "witty", "zesty", "zippy",
    "balmy", "lush", "azure", "ivory", "jade", "coral", "olive", "velvet",
];

/// Animals: short, familiar.
pub const ANIMALS: &[&str] = &[
    "alpaca",
    "badger",
    "bat",
    "bear",
    "beaver",
    "bee",
    "bison",
    "bobcat",
    "camel",
    "cat",
    "civet",
    "cobra",
    "condor",
    "coyote",
    "crane",
    "crow",
    "deer",
    "dingo",
    "dolphin",
    "dove",
    "dragonfly",
    "eagle",
    "egret",
    "elk",
    "falcon",
    "ferret",
    "finch",
    "fox",
    "frog",
    "gecko",
    "gibbon",
    "goat",
    "goose",
    "gopher",
    "gull",
    "hare",
    "hawk",
    "hedgehog",
    "heron",
    "horse",
    "hound",
    "ibis",
    "iguana",
    "jackal",
    "jaguar",
    "jay",
    "kestrel",
    "kiwi",
    "koala",
    "lark",
    "lemur",
    "llama",
    "lynx",
    "magpie",
    "mallard",
    "marten",
    "meerkat",
    "mink",
    "mole",
    "moose",
    "moth",
    "mouse",
    "newt",
    "ocelot",
    "orca",
    "osprey",
    "otter",
    "owl",
    "panda",
    "parrot",
    "pelican",
    "penguin",
    "pika",
    "plover",
    "pony",
    "puffin",
    "puma",
    "quail",
    "rabbit",
    "raven",
    "robin",
    "salmon",
    "seal",
    "shrew",
    "skink",
    "sloth",
    "sparrow",
    "squid",
    "stoat",
    "stork",
    "swan",
    "tapir",
    "tern",
    "tiger",
    "toad",
    "trout",
    "turtle",
    "viper",
    "vole",
    "walrus",
    "weasel",
    "whale",
    "wolf",
    "wombat",
    "wren",
    "yak",
    "zebra",
];

/// A random 64-bit seed (the standard library's hasher keys are random).
fn random_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// The next value of a splitmix64 stream.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn pair(state: &mut u64) -> String {
    let a = ADJECTIVES[(next(state) % ADJECTIVES.len() as u64) as usize];
    let b = ANIMALS[(next(state) % ANIMALS.len() as u64) as usize];
    format!("{a}-{b}")
}

/// How many other pairs are tried before a suffix is added.
const PAIR_TRIES: usize = 64;

/// A friendly name that `taken` does not claim: a few other pairs are tried
/// first, and only a name that every try collided on gets a short suffix.
pub fn pet_name(taken: impl Fn(&str) -> bool) -> String {
    pet_name_seeded(random_seed(), taken)
}

/// [`pet_name`] from a given seed: the same seed and `taken` give the same
/// name (tests; a host that picks the name when a form opens).
pub fn pet_name_seeded(seed: u64, taken: impl Fn(&str) -> bool) -> String {
    let mut st = seed;
    for _ in 0..PAIR_TRIES {
        let name = pair(&mut st);
        if !taken(&name) {
            return name;
        }
    }
    loop {
        let name = format!("{}-{:04x}", pair(&mut st), next(&mut st) & 0xffff);
        if !taken(&name) {
            return name;
        }
    }
}

/// Whether `name` is in the form [`pet_name`] makes (with or without its
/// last-resort suffix): one the SDK, or an app on its behalf, generated.
pub fn is_pet_name(name: &str) -> bool {
    let mut parts = name.split('-');
    let (Some(a), Some(b), rest) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if !ADJECTIVES.contains(&a) || !ANIMALS.contains(&b) {
        return false;
    }
    match rest {
        None => true,
        Some(s) => {
            parts.next().is_none()
                && s.len() == 4
                && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
        }
    }
}

/// The lists as the web app's generated module.
pub fn web_module() -> String {
    let list = |v: &[&str]| {
        v.iter()
            .map(|w| format!("\"{w}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "// SPDX-License-Identifier: FSL-1.1-MIT\n// Copyright (c) 2026 Cua AI, Inc.\n\n// Generated from libs/cua/crates/cua-spaces-contract/src/names.rs (CUA_UPDATE_PET_NAMES=1 cargo test -p cua-spaces-contract pet_names). Do not edit.\n\
         export const PET_ADJECTIVES = [{}] as const;\n\
         export const PET_ANIMALS = [{}] as const;\n",
        list(ADJECTIVES),
        list(ANIMALS)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn pet_names_are_adjective_animal_and_dns_safe() {
        for _ in 0..500 {
            let n = pet_name(|_| false);
            assert!(is_pet_name(&n), "{n}");
            assert!(n.len() <= 24, "{n} is not short");
            assert!(
                n.bytes().all(|c| c.is_ascii_lowercase() || c == b'-')
                    && !n.starts_with('-')
                    && !n.ends_with('-'),
                "{n}"
            );
        }
    }

    #[test]
    fn pet_name_lists_are_sane() {
        for (what, l) in [("adjectives", ADJECTIVES), ("animals", ANIMALS)] {
            assert!(l.len() >= 100, "{what}: only {}", l.len());
            let set: HashSet<_> = l.iter().collect();
            assert_eq!(set.len(), l.len(), "{what} has duplicates");
            for w in l {
                assert!(
                    !w.is_empty() && w.len() <= 10 && w.bytes().all(|c| c.is_ascii_lowercase()),
                    "{what}: {w:?}"
                );
            }
        }
        // A word may not be both: `swift-swift` and `a-b` parse the same.
        for a in ADJECTIVES {
            assert!(!ANIMALS.contains(a), "{a} is in both lists");
        }
    }

    #[test]
    fn a_taken_name_is_retried_with_another_pair_before_any_suffix() {
        let first = std::cell::RefCell::new(None::<String>);
        let n = pet_name(|c| {
            let mut f = first.borrow_mut();
            if f.is_none() {
                *f = Some(c.to_string());
                true
            } else {
                false
            }
        });
        assert_ne!(Some(n.clone()), *first.borrow());
        assert_eq!(n.matches('-').count(), 1, "no suffix yet: {n}");
    }

    #[test]
    fn a_suffix_is_the_last_resort_when_every_pair_is_taken() {
        let n = pet_name(|c| c.matches('-').count() == 1);
        assert!(is_pet_name(&n) && n.matches('-').count() == 2, "{n}");
    }

    #[test]
    fn only_generated_forms_count_as_pet_names() {
        assert!(is_pet_name("brave-otter"));
        assert!(is_pet_name("brave-otter-00ff"));
        for no in [
            "my-space",
            "brave-otter-xyz1",
            "brave",
            "brave-otter-00ff-1",
            "Brave-Otter",
            "space-0123456789",
        ] {
            assert!(!is_pet_name(no), "{no}");
        }
    }

    /// The web app's copy of the lists is generated; it may not drift.
    #[test]
    fn pet_names_lists_match_the_web_copy() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../apps/cua-spaces-web/src/lib/pet-names.generated.ts");
        let want = web_module();
        if std::env::var_os("CUA_UPDATE_PET_NAMES").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        assert_eq!(
            std::fs::read_to_string(&path).unwrap_or_default(),
            want,
            "run CUA_UPDATE_PET_NAMES=1 cargo test -p cua-spaces-contract pet_names"
        );
    }
}
