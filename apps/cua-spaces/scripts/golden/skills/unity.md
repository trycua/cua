# Unity work in a Cua Space

What changes when Unity work happens in a Space instead of on the user's own
desktop. Read this to interpret the request; the Unity MCP's own tool docs cover
how to drive the Editor.

A **Space** is a fresh, disposable sandbox desktop, separate from the user's
machine. It starts clean: their apps are installed, but none of their accounts,
files or projects are in it. Things arrive by **teleport**: copying a
logged-in app session, or files, from their machine into the Space.

**"My Unity Hub" means the logged-in session, not the application.** Unity is
already installed in the Space. What the user is asking to move is their
*identity*: teleporting Unity Hub brings the signed-in account and its Unity
Version Control credentials, so the Space can act as them.

**The Space is already set up for agent work, with a clean desktop.** Its apps
come with their MCP servers and CLIs installed and configured, exposed to you as
tools, but the applications themselves are not running until something needs
them. Use what is already wired up rather than installing or configuring a
toolchain. If a tool for the job seems absent, look for the one the Space
provides before building your own path to it.

**Unity Hub is installed and signed out until you teleport.** Do not launch it
before a teleport: the teleport stops the Hub if it is running, imports the
session, and launches it itself. That launch is the signed-in one, and a Hub
you started and then let the teleport kill can flush its signed-out session over
the freshly imported account. When you need the Hub *without* a teleport, launch
it yourself with `launch_app` via `cua-driver`, or `open -a "Unity Hub" --args
--disable-gpu`. The `--disable-gpu` flag is not optional in a Space: without it
the paravirtual Metal GPU process hangs the Hub.

**Teleport the session; fetch the project inside the Space.** A Unity project is
large, and copying one across is slow and usually unnecessary. The teleported
login can pull it directly from Unity Version Control, from inside the Space,
over the network, far faster than moving it from the user's disk. Prefer that
whenever the project is under version control. Teleport project *files* only for
work that is genuinely local and uncommitted.

**An empty disk is the expected starting state, not a blocker.** When the user
names a project, scene or asset, it is normally in their cloud account rather
than on this machine. Its absence is the first step of the task (clone it),
never a reason to report the request as impossible.

**Unity only imports when it notices.** The Editor picks up files written from
outside on focus, or on an explicit refresh, so an asset another tool just
wrote may simply not exist yet as far as the Editor is concerned, and nothing
will say so. Its log and asset database just stop moving. If something you wrote
has not appeared, bring the Editor to the front or trigger a refresh before
concluding the write failed.

**Unity needs a focus *transition*, and `bring_to_front` on an app that is
already frontmost is a silent no-op.** It returns success, nothing moves, and no
activation event fires, so the refresh you were trying to provoke never
happens. A run lost ~90 seconds watching a frozen `Editor.log` this way after
changing a file on disk and calling `bring_to_front` on an Editor that already
had focus. Focus something else first (any other app), then bring the Editor
back, so Unity sees an actual deactivate/activate pair. If you cannot tell
whether the Editor is already frontmost, check before calling rather than
assuming the call did something.

**The Unity CLI reaches a running Editor through a package that lives in the
project, not on the machine.** If it reports no reachable Editor ("No Unity
Editor instances found with reachable Pipeline servers"), the project is missing
that package rather than the Editor being down. `unity pipeline install` adds it
to the open project. Do not sit in a wait loop for a server that cannot start.

Install it **before** opening the project, or expect to reopen. The install only
edits the project manifest; the Editor picks a manifest change up when it next
scans, and an Editor busy with a first import will miss it entirely: no
"Packages were changed" line, no reload, and the tools never appear. If you
installed it into an already-open project, verify the reload happened rather
than waiting on it.

**`unity cmd` argument shape: global flags BEFORE the subcommand, positionals
AFTER `--`.** The canonical form is

```
unity cmd --timeout 300 <subcommand> -- <positional> [<positional> ...]
```

Getting this wrong is not obvious from the errors, which each look like a
different problem. All three of these are the same mistake (wrong shape, not a
wrong value):

- a JSON payload as the argument is taken as a literal string, so the path is
  pasted into a filename: `No scene asset at 'Assets/{"path":"…"}.unity'`;
- `key=value` after the subcommand: `is not a parameter. Use -- <value>
  instead.`;
- a global flag placed after the subcommand: `<subcommand> has no parameter
  --timeout`.

One subcommand took four attempts to invoke for exactly this reason. The shape
is uniform across subcommands that take a path or other positional (the same
form applies to scene-opening, view-capture and their siblings), so fix the
shape once and reuse it rather than re-deriving it per command. When a
subcommand's arguments are genuinely unclear, ask the CLI for its own help
instead of guessing payload formats.

**Names in the prompt describe the user's mental model, not this filesystem.**
They will say a scene path, an object, a project the way they think of it. Paths
and names in the Space may differ. Look for what they mean (search the project,
inspect the hierarchy) instead of concluding something does not exist because
one exact string did not match.

**Electron dropdowns take keyboard, not clicks.** In the Hub's dialogs a
combobox will open on click and then ignore clicks on its options. Nothing
happens, and nothing reports a failure. Open it, then arrow down and press
return. The same applies to writing a value directly: an Electron UI ignores
synthetic value writes.

**Drive the real GUI when the work is a GUI action.** Signing in, picking a
project to clone, anything that lives in the Hub's interface: drive the actual
window through the `cua-driver` MCP, addressing elements by `element_token` from
`get_window_state`. That is what makes the Space a desktop the user can watch,
and it is often the only path: an Electron UI like the Hub ignores synthetic
value writes and blind coordinate clicks.

**Never drive windows with `osascript` / `tell application "System Events"`.**
Not to activate an app, not to hide one, not to enumerate processes. Your Bash
runs under the SSH identity, and macOS attributes an Apple Event to the
*responsible* process, so the Automation prompt that appears is titled
"sshd-keygen-wrapper", not your command. It blocks for about two minutes and
then stays on screen for the rest of the run, and no synthetic click can dismiss
it. This one is not pre-grantable: the image grants Automation to the processes
that need it, but a grant keyed to the SSH identity is not honoured, so the only
thing that keeps it off screen is you not issuing the event. Use `bring_to_front`
and `launch_app` from `cua-driver`, or `open -a "<App>"`, which need no Apple
Event at all.

`cua-driver` is the MCP named `cua-driver` (a stdio server already registered
for you). It is the only GUI surface that can address elements: `list_windows`,
`get_window_state`, `click` by `element_token`. If your tool list has no
`get_window_state`, you are missing the `cua-driver` MCP; say so and stop. Do
not go looking for another HTTP server on a port to poke by hand, and do not
calibrate pixel offsets: screenshot pixels map 1:1 to its input coordinates.

**The Unity MCP server is the official CLI, and it is already registered.** It
is `unity mcp` (a stdio server, from Unity's own CLI at `/usr/local/bin/unity`),
exposed to you as the `unity` MCP. There is **no port and no port file**: do not
look for `~/.unity-mcp/unity-mcp-port.json` or `unity-mcp-status-*.json` and do
not wait for one to appear. Those belong to a different, third-party bridge and
will never be written here. Its tool list is empty until an Editor is running
with a project open, which is the signal that the Editor is not up yet, not that
the integration is broken.
