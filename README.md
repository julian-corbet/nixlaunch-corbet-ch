# nixlaunch

**A launcher whose layout is a matrix: machines across, folders down, appsets within.**

Every other Wayland launcher is a search box over one list. That shape has a consequence people
stop noticing: the only way it can express *which machine* or *which kind of thing* is by making
you narrow a single flat set until one item survives. A screen is two-dimensional, and a launcher
that uses one axis is spending half of it on nothing.

nixlaunch uses both. Columns are machines. Rows are folders. A cell is that machine's applications
in that folder — and "the editors on that workstation" is a **position you move to**, not a query
you compose.

```
              laptop             workstation        console
            ┌────────────────┐ ┌────────────────┐ ┌──────────────┐
 Terminals  │ Foot  Client   │ │ Foot  Alacritty│ │ Foot         │
            └────────────────┘ └────────────────┘ └──────────────┘
            ┌────────────────┐ ┌────────────────┐ ┌──────────────┐
 Editors    │ Helix  Code    │ │ Helix  Zed     │ │ Helix  Nano  │
            └────────────────┘ └────────────────┘ └──────────────┘
```

## Navigation

`Tab` is the only mode key. That one split is what lets the same four arrow keys mean something
different at each level without a modifier soup.

| | `←→` | `↑↓` | `Enter` |
|---|---|---|---|
| **outside** | machine | folder | go inside (`Shift`: launch the whole cell) |
| **inside** | app on the line | which line | launch it (`Shift`: launch the line) |

Typing searches, fuzzily, across **every** machine at once — because "where does this thing exist"
is a question only a matrix can answer, and a filter scoped to the current column would throw that
away.

## Appsets

A **line** is an appset. The apps on it are meant to start together, and the fact that they sit on
one line is the whole declaration — no separate group concept, no naming ceremony. `Shift+Enter`
starts the line.

You build them by dragging: drop an app **on a line** to insert it at the position nearest your
pointer, or on a cell's background to give it a line of its own. Dropping an app back onto its own
line reorders it. Every arrangement is saved.

## "Other" is an inbox, not a category

It is the last row, always, and it is drawn even when empty. Anything the grouping table does not
recognise lands there — which is what happens the first time a newly-installed application appears.
It shows up somewhere known instead of silently joining a list of two hundred, and you file it with
one drag.

## It does not detect applications

Each machine carries an `inventory` **command** that prints JSON. That is the entire detection
story from this program's point of view: it runs the command and reads the answer. It knows nothing
about SSH, `.desktop` files, flatpaks, package managers or remote hosts, and it must not learn —
"what programs exist on a machine" is a question with owners elsewhere, and a launcher is the wrong
place to grow a second, competing answer to it.

It also means you can run this with no fleet at all: point `inventory` at a script that echoes a
fixed string and everything works.

```json
{
  "host": "laptop",
  "error": null,
  "folders": [
    { "label": "Terminals",
      "apps": [ { "name": "Foot", "icon": "foot", "exec": "foot", "terminal": false } ] }
  ]
}
```

`error` is **carried, not raised**. An unreachable machine is a normal state on a roaming laptop,
and a column drawn with a reason on it is honest — where an empty column is indistinguishable from
a machine that genuinely has nothing installed.

## Three kinds of data, kept apart

This is the design, and conflating any two of them is how launchers end up either forgetting your
arrangement on every rebuild or freezing an app list that has since moved.

| | owner | lifetime |
|---|---|---|
| **config** | declared in Nix, rendered to a file, read-only here | replaced by a deploy |
| **inventory** | discovered per machine, cached | disposable; may be replaced wholesale |
| **placement** | your rearranging, written by this program | survives both of the above |

Placement lives in `$XDG_STATE_HOME/nixlaunch/placement.json` — state, not config, so a rebuild
cannot overwrite it. It records an **arrangement** (machine → folder → lines of names) rather than
a folder per app, because a folder map cannot say *where* on a line, *which* line, or in what
order, and those are exactly what dragging decides.

## Configuration

```nix
nixlaunch = {
  enable  = true;
  folders = [ "Terminals" "Editors" "Browsers" "Chat" ];   # priority order
  machines = [                                             # column order
    { name = "laptop";      accent = "#166534"; inventory = [ "rlaunch" "--json" "laptop" ]; }
    { name = "workstation"; accent = "#B45309"; inventory = [ "rlaunch" "--json" "work" ]; }
  ];
};
```

`machines` and `folders` are **lists, not attrsets**, and that is load-bearing rather than
stylistic. Column order decides which machine you open on; row order decides which row an app lands
in, because grouping upstream is first-match-wins. An attrset would alphabetise both and silently
change behaviour.

`inventory` is a list, never a shell string — a string gets re-split on spaces by whatever runs it,
and a path containing one would fail in a way that looks like an unreachable machine rather than a
quoting bug.

## Status

Working and in daily use against real inventories. Not yet packaged for anything but Nix.

Known gaps, stated rather than hidden:

- No frecency ordering yet. It is easy to add and it fights muscle memory, which is the thing that
  makes a spatial launcher fast in the first place — so it needs a decision, not a default.
- Dragging **across** machines is refused. Filing is per machine, and "Firefox on one box" is not
  the same object as "Firefox on another"; whether a cross-machine drag should mean anything is
  genuinely open.
- The launch path is wired for the inventory's own `exec`; per-machine `launch` prefixes are
  declared but not yet exercised on every path.

## Built on

[`gtk4-rs`](https://gtk-rs.org) and [`gtk4-layer-shell`](https://github.com/wmww/gtk4-layer-shell)
for the surface, and [`nucleo`](https://github.com/helix-editor/nucleo) — Helix's matcher — for
search. A hand-rolled substring filter was tried first and thrown away: typing one common letter
matched nearly every application, because nearly every name contains one.

## Licence

MIT.
