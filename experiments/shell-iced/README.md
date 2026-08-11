# 00N — what does a shell cost if it is not GTK?

## The question

GTK is the largest remaining cost in this program and the only part of it that cannot go to macOS.
Measured on one machine, 191 applications, cairo renderer, headless compositor:

| | value |
|---|---|
| distro-linked, visible | 24.9 MB private, 0.100 s to first draw |
| distro-linked, resident and hidden | 18.4 MB private |
| a near-empty GTK4 window (`gtk4-demo`) | 47 MB PSS — the floor before nixlaunch does anything |

Everything above that floor has now been optimised: icons decoded once per machine rather than per
launch, the allocator returning decode churn, the grid rebuilt only on structural change. What is
left **is** the toolkit.

So: does a portable Rust toolkit — Iced with `iced_layershell`, or Slint — draw the same grid for
less, and how much less? The answer decides two things at once, which is why it is worth a
prototype rather than an opinion:

1. **Whether Linux keeps GTK.** A shell that is materially cheaper is worth switching to; one that
   is within noise is not, because GTK's layer-shell support is mature and its drag-and-drop works.
2. **Whether macOS can share a shell.** There is no layer shell on macOS at all — the analogue is a
   non-activating floating panel, the thing Alfred and Raycast use — so a Mac port needs a different
   surface whatever the toolkit. If one portable shell covers both, the packaging story collapses
   from four recipes to two; if not, macOS gets a native AppKit shell over the same core.

## Why this is cheap now

`nixlaunch-core` has no toolkit dependency, so a prototype reuses the model, the placement store,
the frecency gate and the fuzzy filter unchanged. The prototype only has to *draw*, and only has to
draw one static screen: the grid, from a real inventory, with no interaction at all. Interaction
does not need to work to answer a memory-and-latency question.

## Method

The same harness the GTK numbers came from, so the comparison is like for like:

- `scratchpad/headless-mem.sh` and `headless-time.sh` — a throwaway `WLR_BACKENDS=headless scroll`,
  so nothing appears on a real display and no running instance can absorb the launches.
- The same 191-application inventory, served from a file so no SSH is on the path.
- Private memory (dirty + clean) as the metric, not PSS: PSS splits shared library pages across
  every process mapping them, which flatters a toolkit that other programs happen to be using.

## Result

Not yet run.
