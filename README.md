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
    {
      name = "laptop";
      accent = "#166534";
      inventory = [ "rlaunch" "--json" "laptop" ];
      launch = [ "{}" ];                                  # explicit local execution
    }
    {
      name = "workstation";
      aliases = [ "work" ];
      accent = "#B45309";
      inventory = [ "rlaunch" "--json" "work" ];
      inventory_timeout_ms = 5000;
      launch = [ "waypipe@work" ];                         # argv prefix for remote execution
    }
  ];
  terminal = [ "foot" "-e" ];
};
```

`machines` and `folders` are **lists, not attrsets**, and that is load-bearing rather than
stylistic. Column order decides which machine you open on; row order decides which row an app lands
in, because grouping upstream is first-match-wins. An attrset would alphabetise both and silently
change behaviour. Frecency may reorder apps and appsets within a cell, but it never moves configured
folders or subrows: each category remains one contiguous block, with its heading rendered once.

`inventory` is a list, never a shell string — a string gets re-split on spaces by whatever runs it,
and a path containing one would fail in a way that looks like an unreachable machine rather than a
quoting bug.

`launch` is deliberately explicit. An empty list makes a machine read-only; `[ "{}" ]` substitutes
the inventory entry's `Exec` directly for local execution. A template such as
`[ "ssh" "work" "{}" ]` substitutes it at that position, while a list without `{}` is an argv
prefix. Programs declaring `Terminal=true` are refused unless `terminal` names an emulator, so a
launch never succeeds invisibly.

The Home Manager module also exposes typed `theme`, `surface`, `keyboard`, `keys`, `subrows`, and
focus-loss options. Key overrides extend the defaults, `null` unbinds one chord, and the explicit
`launch-line` and `launch-cell` actions retain their meaning in either focus mode. The default
`launch-selection` remains contextual: cell outside, line inside.

Folders are appset grids by default. A folder that represents a catalogue rather than groups to
start together can opt into a library layout:

```nix
nixlaunch.folderModes.Games = "library";
```

Each named row then stays one horizontal vector instead of wrapping at `theme.line_width`; moving
the selection pans the viewport. A bulk-launch gesture outside enters the shelf, and inside it
starts only the selected item, so a long genre shelf cannot accidentally launch every title it
contains.

With `daemon.enable = true`, the service starts hidden. A reveal maps the cached grid immediately,
re-reads `config.json`, and refreshes every inventory in the background; the newest refresh wins,
and each command is bounded by `inventory_timeout_ms`. Model inputs — folders, subrows, machines,
launch prefixes, and inventory line width — update on that reveal. Settings bound to the existing
GTK process or window — CSS/theme, key bindings, terminal wrapper, focus policy, and surface mode —
take effect at the next ordinary process start instead of half-reconfiguring a mapped window.

## Status

Working and in daily use against real inventories. The flake provides the Nix package and Home
Manager module; `packaging/PKGBUILD` provides the native Arch build used when sharing the distro's
already-resident GTK libraries matters more than a Nix store closure.

Known gaps, stated rather than hidden:

- Dragging **across** machines is refused, deliberately and permanently. Filing is per machine and
  "Firefox on one box" is not the same object as "Firefox on another" — a launcher cannot move an
  application between machines, so a drag that looked like it did would be lying about what
  happened.

## Speed

A launcher is judged on the gap between the keystroke and the window, so the startup path is
measured rather than assumed. On the 191-application reference inventory, a distro-linked GTK+cairo
build reached its first draw in about 0.10 s when inventory was served locally. Remote inventory is
the variable cost:

| Phase | Cost | Why it is what it is |
|---|---|---|
| exec, dynamic link, GTK init, first draw | ~0.10 s locally | Distro-linked GTK+cairo, measured headlessly. |
| cold remote inventory | bounded by the **slowest** machine | One worker per machine; sequentially this was 66 + 269 + 324 ms. |
| resident reveal | cached grid first | Inventory refresh happens asynchronously and cannot block GTK. |

Three things follow from those numbers, and they are the reason the code looks the way it does:

**Concurrency buys exactly one thing.** Asking the machines is the only part that waits on something
external, so it is the only part threaded. Both output pipes are drained while each command runs,
responses are size-capped, and a timeout kills its process group. Everything after it is GTK, which
is single-threaded by construction.

**The cairo renderer, by default.** GTK4 picks its GSK renderer by probing the GPU and chooses Vulkan
where one is available. For a program that draws boxes, labels and icons that is device init, shader
compilation and a driver thread pool spent on nothing — entirely on the latency path. Measured here,
cairo settles in 0.52–0.55 s every run; one Vulkan run was still burning CPU at 4.6 s. `GSK_RENDERER`
in the environment still overrides it.

**Moving the cursor is not rebuilding the grid.** The two updates are separate functions: `render`
tears the grid down and builds it again, `paint` moves the selection classes from where they were to
where they are now. A repaint touches at most eight widgets no matter how large the grid is, because
it remembers what it highlighted last time. Arrow keys, Tab and Enter-into-a-cell take the cheap
path — which is where a spatial launcher spends most of its life.

If your inventory command reaches other machines, cache its answer. The reference one
(`rlaunch --json`) does, and the TTL is worth setting to minutes rather than seconds: nobody opens a
launcher twice within a minute, so a short TTL means paying the full round trip on essentially every
launch.

## Built on

[`gtk4-rs`](https://gtk-rs.org) and [`gtk4-layer-shell`](https://github.com/wmww/gtk4-layer-shell)
for the surface, and [`nucleo`](https://github.com/helix-editor/nucleo) — Helix's matcher — for
search. A hand-rolled substring filter was tried first and thrown away: typing one common letter
matched nearly every application, because nearly every name contains one.

## Licence

MIT.
