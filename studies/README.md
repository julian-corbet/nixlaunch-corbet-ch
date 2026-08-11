# studies

Things that stayed true after the question that prompted them was answered.

## Why a fixture hides layout bugs

Two defects survived every hand-written fixture and appeared the instant real inventories loaded,
both for the same reason: **fixtures are short**.

1. **One line per app.** Packing was never exercised, because no fixture cell held more than three
   applications. With real data every app became its own line, a cell grew as tall as its folder,
   and the 2D navigation inside a box collapsed — up/down stepped one app at a time and left/right
   did nothing at all.
2. **Size-to-content.** Correct for a fixture and catastrophic for hundreds of applications: the
   window outgrew the display, and a layer surface has no titlebar to drag it back with, so the top
   and bottom were simply unreachable.

The lesson is not "write bigger fixtures". It is that a fixture proves interaction and cannot prove
layout, so layout has to be checked against a real inventory at least once before it is believed.

## Where a launcher's startup time actually goes

Measured, not guessed, on a three-machine 191-application inventory. The method matters as much as
the numbers, because the first three attempts measured the wrong thing:

- **`strace -f` over everything** gives an accurate ORDER of events and accurate timings for the
  parts that are syscall-bound — but it inflates a futex-heavy phase enormously, and the renderer
  bring-up is exactly that. Good for "what happens when"; useless for "how long does it take".
- **Watching for the process to stop consuming CPU** looks right and is wrong: during an SSH round
  trip the process burns no CPU at all, so the detector fires mid-startup and reports 0.12 s.
  Requiring that every child has exited first fixes it.
- **A cheap syscall subset** (`-e trace=execve,sendmsg,ppoll`) gives usable wall-clock without the
  futex penalty, and the last Wayland write before a long `ppoll` is a decent first-frame marker —
  unless the renderer never settles, which is how the Vulkan finding turned up.

What the numbers said:

| Phase | Cost | Threadable |
|---|---|---|
| exec + dynamic link + GTK init | ~85 ms | no |
| inventory, 3 machines, sequential | 66 + 269 + 324 ms | **yes** — it is all waiting |
| grid construction + first frame | ~400 ms good case | no — GTK is single-threaded |
| icon theme lookups | negligible (193 failed probes total) | — |

Three things worth keeping:

**The obvious suspect was innocent.** Icon lookups are the classic launcher bottleneck and the
hypothesis was wrong here — 193 failed probes across the whole startup. Checking cost one query
against the trace; assuming would have cost a pointless caching layer.

**The renderer is a latency decision, not a quality one.** GTK4 probes the GPU and picks Vulkan
where it can. Cairo settled in 0.52–0.55 s on every run; one Vulkan run was still burning CPU at
4.6 s and had emitted 80 Wayland frames against cairo's 10. For a surface made of boxes, labels and
icons there is nothing for a GPU pipeline to do, and bringing one up sits directly between the
keypress and the window.

**Identical input, 4–5× spread.** The same config measured 550 ms of CPU on one run and 2500 ms on
the next. A bimodal result is not noise to average away — it means something occasionally spins, and
averaging is precisely how you hide it.

## A cache TTL is a latency budget

`rlaunch`'s inventory cache defaulted to 60 seconds, which is a sensible number for a launcher a
person clicks through and the wrong one for a launcher that asks every machine at startup. Nobody
opens a launcher twice within a minute, so a 60-second TTL means paying the full SSH round trip on
essentially every launch — 269 ms and 324 ms here, against 25–35 ms warm.

The general shape: when a cache's TTL is shorter than the interval between uses, it is not a cache.
It is a delay with a directory attached.
