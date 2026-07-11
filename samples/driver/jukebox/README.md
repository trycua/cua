# CUA Jukebox — coordinated multi-cursor computer-use, as music

A MIDI file (or a built-in demo song) becomes a **grid of miniwob-style
minigame windows** — one per track. Each window gets its **own cua-driver
session = its own uniquely-coloured agent cursor**. While the song plays, every
note steers that track's cursor onto its widget and **clicks it in the
background** — and the click is what makes the sound. One cursor per part, one
colour per agent, all driven off a single clock: the dumbest possible orchestra,
performed entirely by background computer-use.

It's the native-windows sibling of the multi-cursor "National Records System"
demo: same no-z-raise, no-real-mouse-movement background actuation, but here the
*timing* is the point.

```
┌───────────────────────────────────────────────────────────────┐
│  CUA  JUKEBOX — Transport          ▶ PLAY      ███████░░░░░░░░  │  ← you click PLAY
├──────────────┬──────────────┬──────────────┬──────────────┬────┤
│ Bass  ◣crimson│ Kick  ◣amber │ Hat   ◣aqua  │ Pad ◣mint    │ …  │
│ [pitch strip] │ [ KICK pad ] │ [ HAT pad ]  │ [pitch strip]│    │  ← one agent
│               │              │              │              │    │     cursor each
└──────────────┴──────────────┴──────────────┴──────────────┴────┘
```

## What each window is

| Window | Role | Widget | cursor actuation |
|---|---|---|---|
| Transport (controller) | foreground; the one human action | real Win32 `PLAY/STOP` button + playhead | you click it |
| Kick / Snare / Hat | drum pad | one big pad; any click = a fixed hit | background click → drum voice |
| Bass / Lead / Pad / Arp / … | melodic | a **pitch strip**: the click's X selects the semitone | background click → that pitch |

Each instrument owns its **own `rodio` output stream**, so notes from the
separate instrument processes mix at the OS mixer — genuine polyphony across the
whole fleet. The track's name picks its minigame + synth voice (kick/snare/hat →
pad; bass/lead/pad/arp/… → pitch strip), mirroring the original HTML jukebox's
`inferKind`.

**Visual feedback:** a melodic strip lights the struck **key**, and each press
fades on its own clock — so a chord lights several keys at once, individually. A
drum pad instead has a single **brightness that gets an impulse per hit and
constantly fades**, so the faster it's triggered the brighter it glows.

## Cursor pools (one colour, many hands)

A track normally has one cursor, but a cursor is "busy" for the whole click —
the glide **plus** dispatch (≈ glide + 130 ms). When a track's notes fall closer
together than that — a **chord** (simultaneous), or just a line faster than one
cursor can service at the current glide — the first cursor is still busy, so the
next free cursor in the track's **pool** takes the note, and the pool grows on
demand (try the first, else the next, else spawn one; capped at 6). Every pool
member is forced to the **same colour**, so a triad fans out into three
identically-coloured cursors stabbing three keys at once, and a fast hi-hat line
splits across two. Sizing the pool to the *real* click duration (not just the
glide) is what keeps every cursor on the beat — otherwise one cursor would fall
progressively behind on a track whose notes outpace its glide. The built-in demo
plays Pad triads and dense hats/arps to show this. Slow / sparse tracks keep one
cursor. Each pool cursor is its own cua-driver session + its own persistent
connection + its own thread, so they actuate concurrently.

## How the timing stays on the beat

A background click only makes its sound once the cursor has *glided onto* the
widget and tapped it, so the actuation lags the dispatch. Three things make it
land on the beat anyway:

1. **Fixed-duration glide.** Every cursor is pinned to a known, constant flight
   time via `set_agent_cursor_motion {"glide_duration_ms": 200}` — so a 3-pixel
   nudge and a cross-strip leap both arrive in the same 200 ms, making the
   latency *predictable enough to sequence*.

   > `glide_duration_ms` is honoured identically on macOS, Windows, and Linux —
   > it lives in the shared cursor-overlay render core (`tick_motion` /
   > `tick_swift_constants`). `0` (the default) keeps the original speed-based
   > glide; any value `50–5000` forces that fixed flight time. No platform drift.

2. **Persistent daemon connection.** The orchestrator holds **one named-pipe
   connection per voice** to `cua-driver serve` and pipelines every click over
   it — no `cua-driver call` *process spawn per note*. That spawn (tens of ms,
   wildly variable) was the original timing-jitter source; removing it dropped
   the per-note jitter from **sd ≈ 490 ms → ≈ 35 ms**.

3. **Per-voice adaptive lead + throughput-sized pools + 1 ms timer.** Each click
   is fired early by an adaptive lead (a per-voice EMA tuned from the measured
   error, warm-started at the dispatch overhead) so the mean error → 0 without
   one congested track skewing another. Pools are sized to the real click
   duration (see above) so no single cursor outruns its glide. The system timer
   is raised to 1 ms so `thread::sleep` schedules each click precisely. **And the
   daemon must be built `--release`** — the overlay's per-frame pixel pipeline is
   ~5× slower in debug (≈12 fps vs ≈60 fps under the full demo), and a slow
   overlay = late, jittery glide-arrivals.

The orchestrator **tracks the diff** itself: it records `(actual − scheduled)`
for every note and prints a report at song end, e.g.

```
[timing] n=256  mean=-4.0ms  |mean|=19.4ms  sd=35.8ms  median=-0.8ms  p10=-36  p90=+20  max=138
```

i.e. notes land on the beat to a sub-millisecond median with ~35 ms of jitter
(about as tight as a human drummer), with a 200 ms glide. Tune with env vars:
`JUKEBOX_GLIDE_MS` (default 200) and `JUKEBOX_LEAD_MS` (defaults to the glide,
the initial per-voice pre-roll the adaptive correction refines from).

## Build

```powershell
# from this directory
cargo build
# Build the driver RELEASE — the cursor overlay composites a full-virtual-screen
# bitmap (RGBA→BGRA) and blits it every frame, which is ~5× slower unoptimized.
# Debug: ~12 fps overlay under the full demo; release: ~60 fps (vsync-capped).
cargo build -p cua-driver --release --manifest-path ..\..\libs\cua-driver\rust\Cargo.toml
$env:CUA_DRIVER_EXE = "..\..\libs\cua-driver\rust\target\release\cua-driver.exe"
```

## Run

```powershell
.\target\debug\jukebox-orchestrator.exe                 # then click ▶ PLAY in the Transport window
.\target\debug\jukebox-orchestrator.exe --auto          # self-starts after warmup
.\target\debug\jukebox-orchestrator.exe song.mid        # drive any multitrack .mid (best with named tracks)
.\target\debug\jukebox-orchestrator.exe song.mid --auto
```

The orchestrator reaps any stale daemon, starts `cua-driver serve`, launches the
Transport + one instrument window per track, tiles them (a 600×80 bar over a grid
of 200×160 tiles), arms one coloured fixed-glide cursor pool per track, and on
**PLAY** fans beat-synced background clicks out to every instrument. Press **Esc**
on the Transport (or Ctrl-C the orchestrator) to tear everything down — a Windows
Job Object kills the whole tree.

## Bring your own song (MIDI)

- **Drag a `.mid` onto `jukebox-orchestrator.exe`** in Explorer — Windows passes
  it as the argument, so the file becomes the song.
- …or pass it on the command line: `jukebox-orchestrator.exe path\to\song.mid`.

It works best with **multitrack files that have named tracks** — the visualizer
reads each track's name to pick its minigame + voice (`kick`/`snare`/`hat` → drum
pad; `bass`/`lead`/`pad`/`arp`/`string`/… → pitch strip), and an unnamed track
falls back to a sine pitch-strip. General-MIDI pop/electronic arrangements (one
instrument per track, a drum track, a bass, a couple of leads/pads) map cleanly;
chords on a track fan out into that track's same-colour cursor pool.

Where to find MIDIs (always check each file's own licence):
- **Open / Creative-Commons** score libraries — [Mutopia Project](https://www.mutopiaproject.org/),
  [kunstderfuge](https://creativecommons.org/2008/03/07/kunstderfuge/) (CC BY-NC-SA),
  the [Classical Piano MIDI Page](http://piano-midi.de/copy.htm) (CC BY-SA) — these
  skew classical but are cleanly licensed and well-separated into tracks.
- **CC audio search**: [Openverse](https://openverse.org/) and the
  [Free Music Archive](https://freemusicarchive.org/curator/Creative_Commons/).
- **Large general archives** (free downloads; licensing varies per file, so use for
  personal/demo use): BitMidi (`bitmidi.com`), MidiWorld (`midiworld.com`),
  FreeMidi (`freemidi.org`) — good for finding multitrack electronic/pop tracks.

No `.mid`? The built-in generated 8-bar electronic loop (the default) is tuned to
exercise every part of the visualizer — drums, a bass line, an arp, and Pad
triads that show the cursor pool.

### Env overrides
`CUA_DRIVER_EXE`, `JUKEBOX_APP_EXE`, `JUKEBOX_GLIDE_MS` (default 200),
`JUKEBOX_LEAD_MS` (defaults to the glide). Set
`CUA_DRIVER_RS_OVERLAY_FPS_FILE=<path>` (read by the daemon) to log the agent
cursor overlay's measured render FPS once a second.

## How the coloured cursors work

Each track's session is a **cua-driver palette name** (`crimson`, `amber`,
`aqua`, `mint_lime`, `orchid`, …), so its overlay cursor renders in that palette
automatically (`Palette::for_instance(session)`) — the same trick the
multi-cursor demo uses. The instrument window's accent and the Transport legend
reuse that palette's colour, so the cursor, its window, and the legend all read
as one colour. (`cursor_color` on `set_agent_cursor_motion` only records a value;
it doesn't repaint the overlay — true on macOS and Windows alike — so we key by
palette name instead.)

## Honest caveats

- **Timing is groove-tight (median ~0 ms, jitter ~30 ms), not sample-accurate.**
  The residual jitter is the overlay render tick (~8 ms) + OS scheduling. Under
  heavy system load the daemon's single overlay render thread can starve, which
  shows up as occasional multi-hundred-ms outliers on dense tracks; on an idle
  machine all notes land in the ±30 ms band.
- **MIDI parsing uses a single tempo** (first tempo event wins) and ignores
  channel/program data — instrument inference leans on track names. Untitled
  tracks default to a sine pitch-strip.
- Up to **9 tracks** (one per cua-driver palette); extra tracks are dropped.
- Audio needs a default output device; with none, instruments still flash
  silently.
