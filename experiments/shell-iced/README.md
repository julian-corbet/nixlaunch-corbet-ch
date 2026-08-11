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

Same machine, same 191-application inventory, same headless compositor, text only on both sides,
software renderer on both sides:

| | private | dirty | clean | RSS | first draw | CPU |
|---|---|---|---|---|---|---|
| GTK + cairo | **25.3 MB** | 14.9 | 10.3 | 90.6 | **0.095 s** | 60 ms |
| Iced + tiny-skia | 59.0 MB | 20.3 | 38.6 | 69.3 | 0.098 s | 90 ms |
| Iced + wgpu (its default) | 86.0 MB | 33.1 | 52.9 | 163.0 | — | — |

**Time is a tie.** 0.095 s against 0.098 s is noise; neither toolkit is the reason this program feels
fast or slow.

**Memory is not, and the direction is the opposite of the expectation.** GTK — the heavier library
by any static measure, a 47 MB floor for an empty window — costs less than half what a Rust toolkit
does here.

### Why, and the condition it depends on

Look at `clean` against `RSS`. Iced's total footprint is SMALLER (69 MB vs 91 MB) and its private
cost is more than twice as large. GTK's library pages are shared: ironbar, the file manager and
everything else on this desktop map the same `/usr/lib/libgtk-4.so.1`, so those pages are already
resident and cost this process almost nothing. Iced is statically linked into an 11.7 MB binary
with no sharing partner anywhere on the system, so every page it touches is its own.

That is a fact about **this machine**, not about the toolkits. The conclusion holds wherever GTK is
already resident — which is every Linux desktop running a GTK bar, panel or file manager — and
inverts on a machine where nothing else uses it.

### What follows

**Linux keeps GTK.** Half the private memory, identical latency, and a mature layer-shell
implementation with working drag-and-drop, which the alternative would have to reimplement.

**macOS does not inherit that argument**, and this is the useful part. There is no GTK on a Mac to
share with, so the sharing advantage that wins here does not exist there — and GTK's macOS backend
is a second-class port besides. A Mac shell should be chosen on its own merits: either Iced (whose
57 MB would be unremarkable there, and which brings Windows with it) or native AppKit for the
non-activating panel that Alfred and Raycast use.

So the answer is not "one shell" or "two shells" but: **the core was worth separating, and the
right shell is a per-platform decision that the split now lets us take independently.** Which is
what the split was for, arrived at by measuring rather than by preferring.
