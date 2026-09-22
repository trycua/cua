# Provenance: what came from where

## Task schema and scoring harness

The `CuaTask`/`OptionSpec` schema (state + typed bounded-option question +
expected + provenance + split), the dataset-hash pre-registration mechanism,
the model-adapter abstraction for plugging in an architecture under a
uniform interface, the fail-closed probability-distribution validation, and
the accuracy/calibration/speed composite scoring formula are this package's
core, reusable design. They generalize a "typed bounded-decision" task
pattern (state + fixed option set + expected answer, frozen before any model
sees it) from a pure-text setting to real GUI/computer-use actions.

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

Note on GPL-licensed dependencies: `python-chess` and `Stockfish` are both
GPL-3.0. They are declared as an optional extra (`cua-bench-s1[chess]`) and
are not required to install or use the rest of the package; a downstream
project that cannot accept GPL-3.0 obligations for a combined work should
avoid installing the `chess` extra and skip the `chess` family.

## AndroidControl mirror choice

This package's `datagen/androidcontrol.py` reads a community mirror of
AndroidControl's canonical TFRecords (`leosltl/Android-Control` on the
Hugging Face Hub), verified against the official schema (field names,
action-type vocabulary, license) before use. A second mirror
(`smolagents/android-control`, parquet) was evaluated and rejected because it
drops the `accessibility_trees` field entirely (screenshots + actions only)
-- this package's reason for using AndroidControl is that it has both
modalities per step, so a mirror missing one defeats the point.

AndroidControl's `accessibility_trees` field is a serialized protobuf
message. Rather than take on the `android_env` package's dependency tree (and
a `protoc` toolchain requirement) just to decode it, `datagen/_pbwire.py`
implements a small, dependency-free, generic protobuf wire-format reader:
tag/varint/length-delimited parsing only, using field numbers copied verbatim
from the public upstream `.proto` files. It is a one-purpose reader, not a
general protobuf library, and is only as correct as the field-number mapping
the caller supplies.

## Chess and game_control: held-out only, by design

Both `chess` and `game_control` are used only for evaluation in this
package's own intended workflow -- never mixed into a training split.
This is a deliberate train/eval separation: it lets a benchmark user measure whether a model trained on GUI
form/login/consent/pagination/search tasks generalizes to a structurally
different decision domain (real chess positions; a live first-person game
loop), rather than only measuring in-distribution accuracy.

## What was not ported from any research precursor

This package does not include: any training code, any live-capture-session
tooling, or any research/debugging narrative log. Everything here is
reusable schema, generation, and evaluation code plus the real, checkable
methodology notes above.
