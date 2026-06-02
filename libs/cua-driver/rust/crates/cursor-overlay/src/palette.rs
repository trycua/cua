//! Nine named colour palettes — 1:1 port of `AgentCursorPalette.cs`.
//!
//! Colours stored as `[u8; 4]` = [R, G, B, 255].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub name: String,
    /// Tip colour (lightest, gradient position 0.0).
    pub cursor_start: [u8; 4],
    /// Mid-gradient colour (position 0.53).
    pub cursor_mid: [u8; 4],
    /// Tail colour (position 1.0).
    pub cursor_end: [u8; 4],
    /// Outer bloom layer.
    pub bloom_outer: [u8; 4],
    /// Inner bloom layer (brighter core).
    pub bloom_inner: [u8; 4],
}

// Raw palette data as (&name, start, mid, end, bloom_outer, bloom_inner).
type PaletteData = (&'static str, [u8;4], [u8;4], [u8;4], [u8;4], [u8;4]);

const fn rgba(r: u8, g: u8, b: u8) -> [u8; 4] { [r, g, b, 255] }

const PALETTE_DATA: &[PaletteData] = &[
    ("default_blue",  rgba(219,238,255), rgba( 94,192,232), rgba( 84,205,160), rgba(188,232,252), rgba(238,248,255)),
    ("soft_purple",   rgba(238,226,255), rgba(178,132,255), rgba(118,194,255), rgba(214,188,255), rgba(246,238,255)),
    ("rose_gold",     rgba(255,231,238), rgba(247,132,170), rgba(255,181,108), rgba(255,190,211), rgba(255,243,232)),
    ("mint_lime",     rgba(226,255,240), rgba( 96,218,174), rgba(178,229, 72), rgba(178,245,217), rgba(241,255,231)),
    ("amber",         rgba(255,244,214), rgba(244,178, 66), rgba(255,126, 92), rgba(255,219,140), rgba(255,248,225)),
    ("aqua",          rgba(221,252,255), rgba( 76,204,224), rgba( 63,222,166), rgba(172,241,249), rgba(236,255,251)),
    ("orchid",        rgba(252,228,255), rgba(221,113,236), rgba(255,139,196), rgba(237,181,246), rgba(255,239,252)),
    ("crimson",       rgba(255,226,226), rgba(232, 82, 98), rgba(150, 94,255), rgba(255,168,178), rgba(255,240,241)),
    ("chartreuse",    rgba(247,255,218), rgba(184,220, 54), rgba( 72,190,119), rgba(224,247,128), rgba(249,255,232)),
    ("cobalt",        rgba(226,235,255), rgba( 80,126,236), rgba( 91,219,222), rgba(170,195,255), rgba(239,246,255)),
    // Added to widen the hue spread (the originals clustered around blue/teal/
    // green); these fill green / yellow / indigo / orange / pink / teal gaps so a
    // larger fleet of concurrent cursors stays visually distinguishable.
    ("emerald",       rgba(225,255,236), rgba( 46,204,113), rgba( 16,185,129), rgba(190,245,215), rgba(240,255,247)),
    ("gold",          rgba(255,250,220), rgba(255,205, 30), rgba(255,170, 40), rgba(255,235,160), rgba(255,252,232)),
    ("indigo",        rgba(231,229,255), rgba( 99, 82,235), rgba(139, 92,246), rgba(199,194,255), rgba(242,240,255)),
    ("tangerine",     rgba(255,238,224), rgba(255,130, 40), rgba(255, 94, 58), rgba(255,205,170), rgba(255,244,234)),
    ("hot_pink",      rgba(255,228,242), rgba(255, 55,150), rgba(236, 72,153), rgba(255,190,222), rgba(255,240,248)),
    ("teal",          rgba(220,252,250), rgba( 20,184,166), rgba( 13,148,136), rgba(175,240,234), rgba(236,255,253)),
];

fn from_data(d: &PaletteData) -> Palette {
    Palette {
        name:         d.0.to_owned(),
        cursor_start: d.1,
        cursor_mid:   d.2,
        cursor_end:   d.3,
        bloom_outer:  d.4,
        bloom_inner:  d.5,
    }
}

impl Palette {
    pub fn default_blue() -> Self { from_data(&PALETTE_DATA[0]) }

    /// Select a palette for an instance id using the same stable-hash
    /// logic as the C# `AgentCursorPalette.ForInstance`.
    pub fn for_instance(instance_id: &str) -> Self {
        if instance_id.is_empty() || instance_id == "default" {
            return Self::default_blue();
        }
        // Exact name match.
        if let Some(d) = PALETTE_DATA.iter().find(|d| d.0 == instance_id) {
            return from_data(d);
        }
        // Stable hash into alternates (all except default_blue at index 0).
        let alternates = &PALETTE_DATA[1..];
        let idx = stable_index(instance_id, alternates.len());
        from_data(&alternates[idx])
    }

    pub fn all_names() -> Vec<&'static str> {
        PALETTE_DATA.iter().map(|d| d.0).collect()
    }

    /// Like [`Self::for_instance`], but **avoids colliding** with palettes already
    /// in use by other live cursors. The hash still chooses the *preferred*
    /// palette (so a given id is stable when nothing conflicts), then we linear-
    /// probe forward to the first palette not in `in_use`. This guarantees every
    /// concurrent cursor gets a distinct colour for up to `PALETTE_DATA[1..].len()`
    /// of them — fixing the case where ids sharing a numeric/letter suffix (e.g.
    /// `mil-005253`, `rom-005253`) all hashed to the same index and rendered the
    /// same colour. Beyond capacity we fall back to the hash pick (reuse begins).
    pub fn for_instance_distinct(instance_id: &str, in_use: &[String]) -> Self {
        if instance_id.is_empty() || instance_id == "default" {
            return Self::default_blue();
        }
        // An exact palette-name id is an explicit request — honour it even if
        // it collides (the caller asked for that specific colour).
        if let Some(d) = PALETTE_DATA.iter().find(|d| d.0 == instance_id) {
            return from_data(d);
        }
        let alternates = &PALETTE_DATA[1..];
        let start = stable_index(instance_id, alternates.len());
        for off in 0..alternates.len() {
            let idx = (start + off) % alternates.len();
            if !in_use.iter().any(|n| n == alternates[idx].0) {
                return from_data(&alternates[idx]);
            }
        }
        // More live cursors than palettes — unavoidable reuse; keep it stable.
        from_data(&alternates[start])
    }

    /// Lerp along cursor_start → cursor_mid → cursor_end at `t ∈ [0,1]`.
    pub fn gradient_at(&self, t: f64) -> [u8; 4] {
        let t = t.clamp(0.0, 1.0);
        if t <= 0.53 {
            lerp_rgba(self.cursor_start, self.cursor_mid, t / 0.53)
        } else {
            lerp_rgba(self.cursor_mid, self.cursor_end, (t - 0.53) / 0.47)
        }
    }
}

fn lerp_rgba(a: [u8; 4], b: [u8; 4], t: f64) -> [u8; 4] {
    let l = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2]), 255]
}

fn stable_index(id: &str, count: usize) -> usize {
    let suffix = id.rfind(|c| c == '-' || c == '_' || c == '.')
        .map(|i| &id[i + 1..]).unwrap_or(id);
    if let Ok(n) = suffix.parse::<usize>() { if n > 0 { return (n - 1) % count; } }
    if suffix.len() == 1 {
        if let Some(c) = suffix.chars().next() {
            if c.is_ascii_alphabetic() {
                return (c.to_ascii_lowercase() as usize - b'a' as usize) % count;
            }
        }
    }
    let mut hash: u32 = 2_166_136_261;
    for c in id.chars() { hash ^= c as u32; hash = hash.wrapping_mul(16_777_619); }
    (hash as usize) % count
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The regression that motivated `for_instance_distinct`: ids sharing a
    /// numeric suffix (`mil-005253`, `rom-005253`, …) all hash to the SAME index
    /// via `stable_index` (it keys off the suffix), so plain `for_instance` gave
    /// them one identical colour. The collision-free picker must hand out
    /// distinct palettes as each cursor is created.
    #[test]
    fn shared_suffix_ids_get_distinct_palettes() {
        let ids = ["mil-005253", "rom-005253", "nap-005253", "fir-005253"];
        // Plain hash collides — documents the bug.
        let hashed: HashSet<_> = ids.iter().map(|id| Palette::for_instance(id).name).collect();
        assert_eq!(hashed.len(), 1, "precondition: shared suffix collides under plain hash");

        // Collision-free picker: feed each new cursor the palettes already taken.
        let mut used: Vec<String> = vec![];
        for id in ids {
            let p = Palette::for_instance_distinct(id, &used);
            assert!(!used.contains(&p.name), "{id} reused an in-use palette {}", p.name);
            used.push(p.name);
        }
        assert_eq!(used.iter().collect::<HashSet<_>>().len(), ids.len());
    }

    /// An exact palette-name id is an explicit colour request and is honoured
    /// verbatim even against in-use (the caller asked for that specific colour).
    #[test]
    fn exact_name_id_is_honoured_even_if_in_use() {
        let p = Palette::for_instance_distinct("crimson", &["crimson".to_owned()]);
        assert_eq!(p.name, "crimson");
    }

    /// More live cursors than palettes degrades gracefully (no panic; reuse
    /// begins) rather than looping forever.
    #[test]
    fn more_cursors_than_palettes_does_not_panic() {
        let n = Palette::all_names().len();
        let used: Vec<String> = Palette::all_names().iter().map(|s| s.to_string()).collect();
        let p = Palette::for_instance_distinct("overflow-session", &used);
        assert!(Palette::all_names().contains(&p.name.as_str()));
        assert!(n >= 10, "palette should have grown beyond the original 10");
    }
}
