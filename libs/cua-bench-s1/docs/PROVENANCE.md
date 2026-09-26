# Provenance: what came from where

## Data sources

| Source | License | Used for |
| --- | --- | --- |
| Synthetic generator (this package) | — (original) | `form_filling`, `login_auth`, `consent_checkbox`, `multi_step_submit`, `pagination`, `search_filter`, `safety_gate` |
| [AndroidControl](https://github.com/google-research/google-research/tree/master/android_control) (Google Research) | Apache-2.0 | real mobile GUI trajectories, bucketed into the trained-on families above |
| [GUI-360](https://huggingface.co/datasets/vyokky/GUI-360) | MIT | real desktop (Word/Excel/PowerPoint) GUI trajectories, bucketed into the trained-on families above |
| [python-chess](https://github.com/niklasf/python-chess) | GPL-3.0 | legal chess position/move generation for the held-out `chess` family |
| [Stockfish](https://github.com/official-stockfish/Stockfish) | GPL-3.0 | gold-move oracle for the held-out `chess` family, when available on `PATH` |
| [ViZDoom](https://github.com/mwydmuch/ViZDoom) | MIT | live game episodes for the held-out `game_control` family |
| [fstandhartinger/jevbench](https://github.com/fstandhartinger/jevbench) | MIT | held-out `general_decision` family (pure text, out-of-domain probe) |
| [xlangai/ubuntu_osworld_verified_trajs](https://huggingface.co/datasets/xlangai/ubuntu_osworld_verified_trajs) | — (dataset terms) | held-out `osworld_next_action` family (real verified Ubuntu agent trajectories, out-of-domain probe) |
| [cua-bench](../../cua-bench) live environments (`datasets/cua-bench-basic/`) | MIT | trained-on `cua_bench_basic` family (real interactive GUI-widget environments) |

`python-chess` and `Stockfish` are GPL-3.0, declared as an optional extra
(`cua-bench-s1[chess]`), and not required to install or use the rest of the
package.

## AndroidControl mirror

`datagen/androidcontrol.py` reads `leosltl/Android-Control` (Hugging Face
Hub), which carries both screenshots and `accessibility_trees`.
`accessibility_trees` is a serialized protobuf message, decoded by
`datagen/_pbwire.py`, a small, dependency-free protobuf wire-format reader
(tag/varint/length-delimited parsing, field numbers from the public
upstream `.proto` files).

## Chess and game_control

Both `chess` and `game_control` are held-out only -- never mixed into a
training split.

## Scope

This package does not include any training code, live-capture-session
tooling, or research/debugging narrative log. It includes the reusable
schema, generation, and evaluation code plus the source table above.
