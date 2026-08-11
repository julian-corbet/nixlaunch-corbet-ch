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
