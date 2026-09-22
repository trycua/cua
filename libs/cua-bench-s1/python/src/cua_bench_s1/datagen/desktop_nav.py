"""Synthetic desktop-application command navigation -> CuaTask.

The decision shape this family generates:

    "the user asked for <some outcome in plain English>; here are sixteen
     terse desktop controls (ribbon commands, menu items, spreadsheet cells);
     click the ONE whose function achieves that outcome"

It is a different skill from the form families in `generator.py` and from a
phone-screen tap. A form task is "this label matches a source value, so fill
it"; a step instruction on a captured phone trajectory usually names the
widget fairly literally. Here the goal states an EFFECT ("add a symbol to the
sheet", "make the toolbar stay visible") while the control carries a COMMAND
NAME ("Insert", "Ribbon Display Options"), so the model has to map intent ->
command function against fifteen same-role distractors drawn from the same
command vocabulary.

Everything here -- the applications, their command catalogues, and the
request phrasings -- is written for this module against an invented, generic
productivity-app vocabulary. No real product's UI and no captured dataset's
episodes, screenshots, control labels or request strings are read, copied or
paraphrased, so a real desktop-trajectory split stays genuinely held out and
what a model learns here is the general skill, not that split's content.

Design choices, stated:

  * The request phrasing is generated from the command's EFFECT description,
    never from its label, so a pure string-match shortcut fails. Some
    commands' labels do overlap their effect wording -- deliberate, and true
    of real requests, so the model should learn to use lexical overlap when
    it is there without depending on it.
  * Distractors are sampled from the SAME app's command list, so they are
    same-role, same-register, genuinely plausible alternatives rather than
    random noise.
  * One distractor is promoted to a hard distractor: it gets the same real
    `click` option the gold control has, with gold `skip`. It is the
    catalogue's own declared `confusable_with` sibling where one exists (e.g.
    "Paste Special" for "Paste"), otherwise the nearest label by similarity
    -- the same hard-distractor rule `gui360.py` uses.
  * A configurable share of a spreadsheet-style screen is filled with terse
    grid-cell labels ("C14", "AB3"), because that is what such a control tree
    is mostly made of and a model that has never seen those labels treats
    them as meaningful words.
"""
from __future__ import annotations

import difflib
import random
from dataclasses import dataclass, field

from ..task import CuaTask, OptionSpec


@dataclass(frozen=True)
class Command:
    """One invocable command in a fictional desktop app.

    `effects` are plain-English descriptions of what invoking it accomplishes,
    phrased the way a user would state a goal -- several per command so the
    request text varies. `confusable_with` names sibling command labels that
    make good hard distractors.
    """
    label: str
    effects: tuple[str, ...]
    confusable_with: tuple[str, ...] = ()


@dataclass
class DesktopApp:
    app_id: str
    title: str
    commands: list[Command] = field(default_factory=list)
    grid_like: bool = False  # spreadsheet-style: pad the control tree with cell buttons


def _c(label, *effects, confusable=()):
    return Command(label, tuple(effects), tuple(confusable))


# --- Fictional apps + their command catalogues -----------------------------
# The app names are invented so nothing collides with a real product or with
# the `app` namespace any real-capture split uses.

_SHEET = DesktopApp("gridwright_sheet", "Gridwright Sheet", grid_like=True, commands=[
    _c("Insert", "add a symbol to the spreadsheet", "put a new chart into the sheet",
       "place an image in the worksheet", confusable=("Insert Function", "Insert Sheet Row")),
    _c("Insert Function", "add a calculation to this cell", "build a formula in the selected cell",
       confusable=("Insert",)),
    _c("Insert Sheet Row", "add an extra row above the current one",
       "make room for another row in the table", confusable=("Insert", "Delete Sheet Row")),
    _c("Delete Sheet Row", "remove the current row from the sheet",
       "get rid of a row in the table", confusable=("Insert Sheet Row",)),
    _c("Freeze Panes", "keep the header row visible while scrolling",
       "lock the top row in place when moving down the sheet", confusable=("Split View",)),
    _c("Split View", "look at two parts of the sheet at once",
       "view separate regions of the worksheet side by side", confusable=("Freeze Panes",)),
    _c("Sort Ascending", "order the rows from smallest to largest",
       "arrange this column low to high", confusable=("Sort Descending", "Filter")),
    _c("Sort Descending", "order the rows from largest to smallest",
       "arrange this column high to low", confusable=("Sort Ascending",)),
    _c("Filter", "show only the rows matching a condition",
       "hide rows that do not meet a criterion", confusable=("Sort Ascending",)),
    _c("Conditional Formatting", "colour cells automatically based on their value",
       "highlight the cells that exceed a threshold", confusable=("Cell Styles",)),
    _c("Cell Styles", "apply a consistent look to the selected cells",
       "format these cells using a preset appearance", confusable=("Conditional Formatting",)),
    _c("Merge Cells", "combine several cells into one",
       "join the selected cells together", confusable=("Wrap Text",)),
    _c("Wrap Text", "make long text fit inside its cell",
       "show the whole entry on multiple lines within the cell", confusable=("Merge Cells",)),
    _c("Number Format", "display these values as currency",
       "change how the numbers in this column are shown", confusable=("Cell Styles",)),
    _c("Ribbon Display Options", "keep the toolbar permanently on screen",
       "stop the command bar from hiding itself", confusable=("Zoom",)),
    _c("Zoom", "make everything on screen larger", "change the magnification of the sheet"),
    _c("Freeze First Column", "keep the leftmost column visible while scrolling sideways",
       confusable=("Freeze Panes",)),
    _c("Paste Special", "paste only the values without the formatting",
       "bring in the copied data but drop its styling", confusable=("Paste",)),
    _c("Paste", "put the copied content into the sheet", confusable=("Paste Special",)),
    _c("Define Name", "give this cell range a reusable name",
       "label a block of cells so formulas can refer to it"),
    _c("Data Validation", "restrict what can be typed into these cells",
       "stop invalid entries being entered in this column"),
    _c("Remove Duplicates", "strip out repeated rows", "keep only unique entries in this table"),
    _c("Page Break Preview", "see where the sheet will split across printed pages",
       "check how this will paginate when printed"),
    _c("Recalculate", "force the formulas to update now",
       "refresh every computed value in the workbook"),
])

_DOC = DesktopApp("quillbank_writer", "Quillbank Writer", commands=[
    _c("Track Changes", "record every edit so they can be reviewed later",
       "start marking up revisions for a reviewer", confusable=("Accept Change", "Comment")),
    _c("Accept Change", "keep a suggested revision", "approve the pending edit",
       confusable=("Reject Change", "Track Changes")),
    _c("Reject Change", "discard a suggested revision", "turn down the proposed edit",
       confusable=("Accept Change",)),
    _c("Comment", "leave a note for another reader", "annotate this passage for a colleague",
       confusable=("Track Changes",)),
    _c("Insert Table of Contents", "generate a contents listing from the headings",
       "build an index of sections at the front", confusable=("Insert Index",)),
    _c("Insert Index", "build an alphabetical term listing at the end",
       confusable=("Insert Table of Contents",)),
    _c("Page Numbers", "number the pages of the document",
       "put a running page count in the footer", confusable=("Header and Footer",)),
    _c("Header and Footer", "put the same text at the top of every page",
       "add repeating content to each page margin", confusable=("Page Numbers",)),
    _c("Line Spacing", "put more space between the lines of text",
       "make the paragraphs less tightly packed", confusable=("Paragraph Spacing",)),
    _c("Paragraph Spacing", "add a gap between paragraphs", confusable=("Line Spacing",)),
    _c("Styles Pane", "apply a consistent heading look throughout",
       "manage the named formats used in this document"),
    _c("Word Count", "find out how long the document is",
       "check how many words have been written"),
    _c("Spelling and Grammar", "check the document for mistakes",
       "proofread the text automatically"),
    _c("Find and Replace", "swap every occurrence of a term for another",
       "change all instances of a word throughout"),
    _c("Mail Merge", "produce a personalised copy per recipient",
       "generate one letter for each row of a contact list"),
    _c("Columns", "lay the text out in two side-by-side columns",
       "split the page into newspaper-style columns"),
    _c("Watermark", "stamp DRAFT faintly behind the text",
       "put a translucent mark across every page"),
    _c("Protect Document", "stop anyone else editing this file",
       "make the document read-only for others"),
    _c("Compare Documents", "see what differs between two versions",
       "diff this draft against an earlier one"),
    _c("Insert Citation", "add a reference to a source", "cite a work in the bibliography"),
])

_SLIDES = DesktopApp("lumen_slides", "Lumen Slides", commands=[
    _c("Slide Master", "change the look of every slide at once",
       "edit the template all slides inherit from", confusable=("Slide Layout", "Theme")),
    _c("Slide Layout", "change the arrangement of this one slide", confusable=("Slide Master",)),
    _c("Theme", "recolour the whole deck consistently",
       "apply a different visual style across the presentation", confusable=("Slide Master",)),
    _c("Transition", "animate the move between slides",
       "add an effect when advancing to the next slide", confusable=("Animation",)),
    _c("Animation", "make an element appear gradually on the slide",
       "add motion to an object on this slide", confusable=("Transition",)),
    _c("Presenter View", "see my notes while the audience sees the slide",
       "show speaker notes on my screen only", confusable=("Rehearse Timings",)),
    _c("Rehearse Timings", "practise and record how long each slide takes",
       confusable=("Presenter View",)),
    _c("Speaker Notes", "write a reminder for what to say on this slide",
       "jot down talking points attached to the slide"),
    _c("Insert Chart", "add a graph of the quarterly figures",
       "put a data visualisation on this slide", confusable=("Insert Table", "Insert Picture")),
    _c("Insert Table", "put a grid of figures on the slide", confusable=("Insert Chart",)),
    _c("Insert Picture", "place a photo on the slide", confusable=("Insert Chart",)),
    _c("Align Objects", "line up the shapes neatly", "make the selected elements share an edge"),
    _c("Group Objects", "treat several shapes as one",
       "bind these elements so they move together"),
    _c("Reorder Slides", "move this slide earlier in the deck",
       "change the running order of the presentation"),
    _c("Hide Slide", "keep this slide in the file but skip it when presenting",
       "exclude a slide from the show without deleting it"),
    _c("Export as PDF", "produce a fixed-layout copy to send out",
       "save the deck in a portable read-only format"),
    _c("Print Handouts", "produce paper copies with several slides per sheet",
       "make audience handouts from the deck"),
])

_MAIL = DesktopApp("postmark_mail", "Postmark Mail", commands=[
    _c("Rules", "file incoming messages into folders automatically",
       "set up automatic sorting for new mail", confusable=("Filter Messages",)),
    _c("Filter Messages", "narrow the visible list to matching mail",
       confusable=("Rules", "Search")),
    _c("Out of Office", "send an automatic reply while I am away",
       "tell senders I am unavailable this week", confusable=("Rules",)),
    _c("Signature", "append my contact details to every message I send",
       "add a standard sign-off block to outgoing mail"),
    _c("Schedule Send", "have this message go out tomorrow morning",
       "delay delivery until a chosen time", confusable=("Send",)),
    _c("Send", "deliver this message now", confusable=("Schedule Send",)),
    _c("Recall Message", "take back a message I already sent",
       "undo the delivery of an earlier email"),
    _c("Mark as Unread", "make this look unopened again",
       "reset a message so it shows as new", confusable=("Flag",)),
    _c("Flag", "mark this message for follow-up",
       "highlight an email so I come back to it", confusable=("Mark as Unread",)),
    _c("Archive", "get this out of the inbox without deleting it",
       "move the message to long-term storage", confusable=("Delete",)),
    _c("Delete", "get rid of this message", confusable=("Archive",)),
    _c("Search", "find a message from last month", "look for mail matching a term",
       confusable=("Filter Messages",)),
    _c("Attach File", "send a document along with the message",
       "include a file with this email"),
    _c("Request Read Receipt", "find out whether the recipient opened it",
       "get confirmation when the message is read"),
    _c("Manage Folders", "reorganise where my mail is stored",
       "create and rearrange mailbox folders"),
])

_IDE = DesktopApp("forge_studio", "Forge Studio", commands=[
    _c("Go to Definition", "jump to where this function is declared",
       "open the place a symbol is defined", confusable=("Find References",)),
    _c("Find References", "see everywhere this symbol is used",
       "list all call sites of this function", confusable=("Go to Definition",)),
    _c("Rename Symbol", "change this variable's name everywhere",
       "safely rename an identifier across the project", confusable=("Find and Replace",)),
    _c("Find and Replace", "swap one piece of text for another in this file",
       confusable=("Rename Symbol",)),
    _c("Format Document", "tidy up the indentation and spacing",
       "apply the standard code style to this file"),
    _c("Toggle Breakpoint", "pause execution at this line when debugging",
       "make the debugger stop here", confusable=("Step Over",)),
    _c("Step Over", "advance the debugger one line", confusable=("Toggle Breakpoint",)),
    _c("Run Tests", "check that everything still passes",
       "execute the project's test suite"),
    _c("Build Project", "compile the code into an artifact",
       "produce a build of the application"),
    _c("Stage Changes", "mark these edits ready to commit",
       "add the modified files to the next commit"),
    _c("Commit", "record the staged changes in version history",
       "save a checkpoint of this work to the repository"),
    _c("Compare with Previous", "see what changed since the last version",
       "diff this file against its committed state"),
    _c("Split Editor", "show two files side by side", "open a second editor pane"),
    _c("Toggle Terminal", "get a shell inside the editor",
       "show the integrated command line"),
    _c("Extensions", "add a new capability to the editor", "install a plugin"),
])

DESKTOP_APPS: list[DesktopApp] = [_SHEET, _DOC, _SLIDES, _MAIL, _IDE]

# How a user states the goal. The `{effect}` slot receives one of the
# command's own effect phrasings. Several frames, in different registers
# (imperative, first-person, polite request, terse), so the goal text itself
# is not a fixed template a model can learn to strip off.
_REQUEST_FRAMES = (
    "{effect_cap}.",
    "I want to {effect}.",
    "Please {effect}.",
    "Can you {effect}?",
    "I need to {effect} in {title}.",
    "In {title}, {effect}.",
    "Help me {effect}.",
    "{effect_cap} -- that's what I'm trying to do.",
    "The goal is to {effect}.",
    "Using {title}, {effect}.",
)

_COLUMNS = ("A", "B", "C", "D", "E", "F", "G", "H", "J", "K", "L", "M", "N", "O", "P",
            "AA", "AB", "AC", "AD")

_MAX_ELEMENTS = 16


def _grid_label(rng: random.Random) -> str:
    return f"{rng.choice(_COLUMNS)}{rng.randint(1, 40)}"


def generate_task(app: DesktopApp, seed: int, *, grid_fill: float = 0.45) -> CuaTask:
    """One desktop command-navigation task.

    The gold action is always `click` on exactly one command; every other
    element is `skip`, and one same-app confusable sibling additionally gets a
    real `click` option whose gold is `skip` (the hard distractor).
    """
    rng = random.Random(seed)
    target = rng.choice(app.commands)
    effect = rng.choice(target.effects)
    frame = rng.choice(_REQUEST_FRAMES)
    goal = frame.format(effect=effect, effect_cap=effect[0].upper() + effect[1:], title=app.title)

    # Distractor commands from the SAME app -- plausible, same register.
    others = [c for c in app.commands if c.label != target.label]
    rng.shuffle(others)

    siblings = [c for c in others if c.label in target.confusable_with]
    if siblings:
        hard = rng.choice(siblings)
    else:
        hard = max(others, key=lambda c: difflib.SequenceMatcher(
            None, c.label.lower(), target.label.lower()).ratio())

    n_grid = int(round((_MAX_ELEMENTS - 1) * grid_fill)) if app.grid_like else 0
    n_cmd_distractors = _MAX_ELEMENTS - 1 - n_grid
    chosen = [c for c in others if c.label != hard.label][:max(0, n_cmd_distractors - 1)]

    labels: list[tuple[str, bool]] = [(target.label, True), (hard.label, False)]
    labels += [(c.label, False) for c in chosen]
    seen_grid = set()
    while len(labels) < _MAX_ELEMENTS:
        cell = _grid_label(rng)
        if cell in seen_grid:
            continue
        seen_grid.add(cell)
        labels.append((cell, False))
    rng.shuffle(labels)

    elements, options, expected = [], [], {}
    # Plausible on-screen geometry, so the state text carries the same
    # '- Button "X" @ [l, t, r, b]' shape a real desktop converter emits.
    for i, (label, is_target) in enumerate(labels):
        eid = f"el_{i}"
        left = 60 + (i % 8) * 118
        top = 90 + (i // 8) * 34
        elements.append({"id": eid, "role": "Button", "label": label,
                         "frame": [left, top, left + 110, top + 26]})
        options.append(OptionSpec(eid, "Button", label, "skip"))
        if is_target:
            options.append(OptionSpec(eid, "Button", label, "click"))
            expected[eid] = "click"
        elif label == hard.label:
            options.append(OptionSpec(eid, "Button", label, "click"))
            expected[eid] = "skip"
        else:
            expected[eid] = "skip"

    ax_tree = "\n".join(f"- {e['role']} \"{e['label']}\" @ {e['frame']}" for e in elements)

    return CuaTask(
        id=f"desktopnav_{app.app_id}_{seed}",
        family="desktop_command_nav",
        app=f"desktopnav_{app.app_id}",
        modality_available=["text"],
        screenshot=None,
        ax_tree=ax_tree,
        ax_tree_source="synthetic",
        elements=elements,
        elements_source="synthetic_spec",
        entities=[],
        options=options,
        expected=expected,
        provenance={
            "generator": "cua_bench_s1.datagen.desktop_nav",
            "seed": seed,
            # The goal is NOT rendered into `ax_tree` here (unlike the form
            # generator's pages): the request is the whole task, so it is read
            # through `task.goal` and printed by the prompt builder.
            "synthetic_goal": goal,
            "target_label": target.label,
            "hard_distractor_label": hard.label,
            "note": "invented command catalogue; contains no captured-dataset content",
        },
    )


def generate_dataset(n_per_app: int, seed: int = 0) -> list[CuaTask]:
    """`n_per_app` tasks for every app in `DESKTOP_APPS`.

    Seeds are derived from the app id with a fixed, process-independent hash
    (`task.stable_digest`, not the built-in `hash()`, which is salted per
    process for strings) so the same arguments always produce the same tasks.
    """
    from ..task import stable_digest

    tasks = []
    for app in DESKTOP_APPS:
        for i in range(n_per_app):
            tasks.append(generate_task(app, seed + stable_digest(app.app_id, i) % 1_000_000))
    return tasks
