#!/usr/bin/env bash
# checks/headless-session.sh — run the launcher against a real compositor with no screen attached.
#
# WHY THIS EXISTS. Every hard bug this program has had was a bug in how it talks to a compositor:
# which output it maps on, what size it asks for, how many times it resizes getting there. None of
# that is reachable from a unit test, and none of it is visible in the compositor's IPC either --
# a layer surface is not a window and never appears in `get_tree`. Before this, the only observable
# was pixels, which meant screenshots, which meant a test that fails when a font changes.
#
# So the program emits a dull `key=value` trace under NIXLAUNCH_TRACE and this asserts on it.
#
# WHAT IT RUNS AGAINST. `scroll`, the compositor these desktops actually use, on the headless
# wlroots backend with the software renderer -- no GPU, no seat, no screen. Testing against a
# different wlroots compositor would be testing a different implementation of exactly the thing
# under test: the keyboard-mode workaround in main.rs exists because of sway-fork `arrange_layers`
# behaviour, and the resize storm this file guards against was driven by how `enter-monitor` is
# delivered.
#
# Usage: headless-session.sh [nixlaunch-binary] [scroll-binary]
set -uo pipefail

LAUNCHER="${1:-nixlaunch}"
COMPOSITOR="${2:-scroll}"
REVEALS=3

command -v "$LAUNCHER" >/dev/null 2>&1 || [ -x "$LAUNCHER" ] || { echo "no launcher: $LAUNCHER" >&2; exit 2; }
command -v "$COMPOSITOR" >/dev/null 2>&1 || [ -x "$COMPOSITOR" ] || { echo "no compositor: $COMPOSITOR" >&2; exit 2; }

# A SHORT path, because a unix socket's sun_path is 108 bytes and the compositor's IPC socket
# silently fails to bind from a deep directory -- which costs the run its `scrollmsg` entirely.
RIG=$(mktemp -d /tmp/nlcheck.XXXXXX) || exit 2
chmod 700 "$RIG"
cleanup() {
  [ -n "${DAEMON:-}" ] && kill "$DAEMON" 2>/dev/null
  [ -n "${COMP:-}" ] && kill "$COMP" 2>/dev/null
  rm -rf "$RIG"
}
trap cleanup EXIT

# NO FLEET, NO NETWORK. The README's promise that this runs with `inventory` pointed at a script
# echoing a fixed string is what makes the check hermetic: the grid is identical every run, so
# `changed=false` below is a real assertion rather than a race with a remote machine.
cat > "$RIG/inventory.json" <<'JSON'
{"host":"fixture","error":null,"folders":[
 {"label":"Terminals","apps":[
   {"name":"Foot","id":"foot.desktop","icon":"utilities-terminal","exec":"true","terminal":false},
   {"name":"Zellij","id":"zellij.desktop","icon":"utilities-terminal","exec":"true","terminal":false}]},
 {"label":"Editors","apps":[
   {"name":"Helix","id":"helix.desktop","icon":"accessories-text-editor","exec":"true","terminal":false}]}]}
JSON

cat > "$RIG/config.json" <<JSON
{"surface":"layer","keyboard":"exclusive","exit_on_focus_loss":true,
 "folders":["Terminals","Editors"],"subrows":{},"keys":{},"terminal":["true"],"outputs":[],
 "theme":{},"layout":{},
 "machines":[
  {"name":"one","aliases":[],"accent":"#22C55E","inventory":["cat","$RIG/inventory.json"],"inventory_timeout_ms":5000,"launch":["{}"]},
  {"name":"two","aliases":[],"accent":"#B45309","inventory":["cat","$RIG/inventory.json"],"inventory_timeout_ms":5000,"launch":["{}"]},
  {"name":"three","aliases":[],"accent":"#166534","inventory":["cat","$RIG/inventory.json"],"inventory_timeout_ms":5000,"launch":["{}"]},
  {"name":"four","aliases":[],"accent":"#B91322","inventory":["cat","$RIG/inventory.json"],"inventory_timeout_ms":5000,"launch":["{}"]}]}
JSON

cat > "$RIG/scroll.conf" <<'CONF'
output HEADLESS-1 mode 2560x1440
output HEADLESS-2 mode 640x480
CONF

# The compositor's own environment, and every socket variable pointed at THIS rig. Leaving any of
# SWAYSOCK / SCROLLSOCK / I3SOCK inherited is not a harmless mistake: the client silently talks to
# whatever session the caller is sitting in, and the run reports on the wrong compositor.
export XDG_RUNTIME_DIR="$RIG"
export WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=2 WLR_LIBINPUT_NO_DEVICES=1
unset WAYLAND_DISPLAY SWAYSOCK SCROLLSOCK I3SOCK DISPLAY

# A session bus needs a config file, and a Nix-provided dbus has no /etc/dbus-1 to find one in --
# so the caller may name it. Empty means the system default, which is right everywhere else.
DBUS=(dbus-run-session)
[ -n "${DBUS_SESSION_CONF:-}" ] && DBUS+=(--config-file="$DBUS_SESSION_CONF")

"${DBUS[@]}" -- "$COMPOSITOR" -c "$RIG/scroll.conf" > "$RIG/compositor.log" 2>&1 &
COMP=$!

for _ in $(seq 100); do
  [ -S "$RIG/wayland-1" ] && break
  sleep 0.1
done
[ -S "$RIG/wayland-1" ] || { echo "compositor never came up:" >&2; tail -20 "$RIG/compositor.log" >&2; exit 1; }
export WAYLAND_DISPLAY=wayland-1

# The launcher and the client have to share one session bus, so both run inside a single
# dbus-run-session; otherwise the client cannot reach the resident instance and silently exits 0.
"${DBUS[@]}" -- bash -c '
  set -u
  NIXLAUNCH_TRACE=1 XDG_STATE_HOME="$1/state" NIXLAUNCH_CONFIG="$1/config.json" "$2" --daemon &
  D=$!
  sleep 4
  for _ in $(seq "$3"); do
    XDG_STATE_HOME="$1/state" NIXLAUNCH_CONFIG="$1/config.json" "$2" >/dev/null 2>&1
    sleep 1.5
    wtype -k Escape 2>/dev/null || true
    sleep 0.7
  done
  sleep 1
  kill $D 2>/dev/null
' _ "$RIG" "$LAUNCHER" "$REVEALS" > "$RIG/trace.log" 2>&1

grep -c 'nixlaunch-trace' "$RIG/trace.log" >/dev/null 2>&1 || true
cp "$RIG/trace.log" "${TRACE_OUT:-/dev/null}" 2>/dev/null || true

python3 - "$RIG/trace.log" <<'PY'
import re, sys
lines = [l.split("nixlaunch-trace ", 1)[1].strip()
         for l in open(sys.argv[1], errors="replace") if "nixlaunch-trace " in l]
if not lines:
    print("FAIL: the launcher emitted no trace at all", file=sys.stderr); sys.exit(1)

maps, settles, cur = 0, [], []
renders, refreshes, failures = [], [], []
for line in lines:
    if line == "map":
        if cur: settles.append(cur)
        cur = []; maps += 1
    elif line.startswith("settle "):
        f = dict(p.split("=", 1) for p in line.split()[1:])
        cur.append(f)
    elif line.startswith("render "):
        renders.append(int(line.split("ms=")[1]))
    elif line.startswith("inventory "):
        refreshes.append(dict(p.split("=", 1) for p in line.split()[1:]))
if cur: settles.append(cur)

if maps == 0:
    failures.append("the surface never mapped")

# THE RESIZE STORM. A window overlapping two outputs gets `enter-monitor` for both; sizing for one
# changes the overlap and delivers another. Unguarded this measured five resizes for one open,
# every one a full re-measure of the grid. Two is the honest ceiling: the entered output, then the
# dominant one confirming.
for i, group in enumerate(settles):
    if len(group) > 2:
        failures.append(f"map {i+1} settled {len(group)} times (max 2): "
                        + ", ".join(g.get("size", "?") for g in group))

# A size past its own cap means the cap is not being applied -- the launcher hanging off the edge
# of the screen it is on, which on a layer surface cannot be dragged back.
#
# UNLESS THE CONTENT CANNOT BE NARROWER. The window has an irreducible minimum: the search bar can
# be squeezed only so far, and below that the label column and padding are all that is left. On a
# screen small enough that the minimum exceeds the cap, exceeding it is the honest outcome and not
# a defect -- so the trace carries the measured minimum and the assertion asks the exact question.
for i, group in enumerate(settles):
    for f in group:
        w, _ = (int(x) for x in f["size"].split("x"))
        cw, _ = (int(x) for x in f["cap"].split("x"))
        floor = int(f.get("min", 0))
        if w > cw and w > floor:
            failures.append(
                f"map {i+1} width {w} exceeds cap {cw} on {f.get('output')} "
                f"while the content could have been {floor}")

if renders and max(renders) > 40:
    failures.append(f"cached render took {max(renders)}ms (budget 40ms): {renders}")

# The fixture is a constant, so a refresh that reports a change is comparing something incidental
# -- ordering, a timestamp -- and every open would pay a second full rebuild of the grid.
for r in refreshes:
    if r.get("changed") == "true":
        failures.append("a static inventory reported changed=true, forcing a second render")

print(f"maps={maps} settles={[len(g) for g in settles]} render_ms={renders} refresh={refreshes}")
if failures:
    for f in failures: print("FAIL:", f, file=sys.stderr)
    sys.exit(1)
print("headless session OK")
PY
