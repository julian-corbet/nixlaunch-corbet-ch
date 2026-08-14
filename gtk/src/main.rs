// nixlaunch — a launcher whose layout is a MATRIX: machines across, folders down, appsets within.
//
// The shape is the point. Every other launcher on Wayland is a search box over ONE list, so the
// only way it can express "which machine" or "which kind of thing" is by making you narrow a
// single flat set. A screen is two-dimensional; a matrix uses both axes, so "the editors on
// that workstation" is a POSITION you move to rather than a query you compose.
//
// THREE LEVELS, TWO OF THEM SIMULTANEOUS ON SCREEN:
//
//   columns = machines            (outside: left/right)
//   rows    = folders             (outside: up/down)
//   a cell  = one machine's apps in one folder, as LINES
//   a line  = an APPSET           (inside: up/down picks the line)
//   an item = one app on that line (inside: left/right walks it)
//
// Tab is the only mode key. That single split is what lets the same four arrow keys mean
// something different at each level without a modifier soup, and it is what makes a LINE a
// first-class object: a line is a set you can start in one keystroke, which is a thing neither
// rofi nor fuzzel can express at all.
//
// ── THIS FILE IS THE GTK SHELL ──────────────────────────────────────────────────────────────
// It runs the inventory commands declared in config and draws the core model. Fixture data remains
// only as the no-config demo, so a fresh checkout is usable without carrying a private setup.
use gtk4 as gtk;

use gtk::gdk::{Key, ModifierType};
use gtk::prelude::*;
use gtk::{
    Align, Application, ApplicationWindow, Box as GBox, CssProvider, EventControllerKey, Image,
    Label, Orientation,
};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};
use std::cell::RefCell;
use std::rc::Rc;

mod icons;
// The launcher itself, from the crate that has no toolkit in it. `model::*` is glob-imported
// because this file speaks in its vocabulary throughout -- App, Line, Machine, Focus, State.
use model::*;
use nixlaunch_core::{config, keymap, model, usage};

type Callback = Rc<dyn Fn()>;
type CallbackSlot = Rc<RefCell<Option<Callback>>>;
type OutputPreference = Rc<dyn Fn(&[String])>;

struct World {
    folders: Vec<String>,
    machines: Vec<Machine>,
    theme: config::Theme,
    layout: config::Layout,
    terminal: Vec<String>,
    surface: String,
    keyboard: String,
    exit_on_focus_loss: bool,
    config: Option<config::Config>,
    error: Option<String>,
}

/// never has to land in the thin space between two widgets to mean something.
fn insert_index_at(container: &GBox, x: f64) -> usize {
    // `compute_bounds` rather than `allocation()`, which GTK deprecated in 4.12. It answers in the
    // coordinate space of the widget you pass, which is exactly the space the drop's own `x` is
    // already in -- so the two are directly comparable with no offset arithmetic to get wrong.
    let mut idx = 0usize;
    let mut child = container.first_child();
    while let Some(w) = child {
        let Some(b) = w.compute_bounds(container) else {
            break;
        };
        let mid = (b.x() + b.width() / 2.0) as f64;
        if x <= mid {
            break;
        }
        idx += 1;
        child = w.next_sibling();
    }
    idx
}

// ── styling ─────────────────────────────────────────────────────────────────────────────────
// Near-black ground, warm off-white ink -- the palette the rest of this desktop already uses.
// The per-machine accent on the column head is the same identity colour the window frames and
// forwarded-window badges use, so a column is recognisable before you read its label.
/// The stylesheet, generated from config values rather than written as a constant. See
/// `config::Theme` -- a colour nobody can reach is this repo carrying one setup's taste.
fn css(t: &config::Theme) -> String {
    format!(
        "
window {{ background-color: {ground}; color: {fg}; }}
.root {{ padding: 18px; }}
.search {{ font-size: 15px; padding: 8px 12px; margin-bottom: 14px;
          border: 1px solid #262626; border-radius: 6px; background-color: {surface}; }}
.search.empty {{ color: {dim}; }}
.colhead {{ font-weight: bold; font-size: 13px; padding: 4px 8px; margin-bottom: 6px;
           border-bottom: 2px solid #262626; }}
.rowhead {{ font-size: 13px; color: {muted}; padding-right: 12px; }}
.rowhead.active {{ color: {fg}; font-weight: bold; }}
.cell {{ border: 1px solid {border}; border-radius: 6px; padding: 5px; margin: 3px;
        background-color: {surface}; }}
.cell.cursor {{ border-color: {accent}; }}
.cell.inside {{ border-width: 2px; padding: 4px; }}
.cell.empty {{ border-style: dashed; }}
.line {{ border-radius: 4px; padding: 2px; }}
.line.sel {{ background-color: alpha({accent}, 0.10); }}
.app {{ padding: 3px 6px; border-radius: 4px; }}
.app.sel {{ background-color: alpha({accent}, 0.20); }}
/* HOVER IS NOT SELECTION. Weaker than .sel and a different weight, so the thing the keyboard is
   on and the thing the pointer is over can never be mistaken for each other. */
.app:hover {{ background-color: alpha({fg}, 0.10); }}
.line:hover {{ background-color: alpha({fg}, 0.04); }}
.cell:hover {{ border-color: alpha({accent}, 0.45); }}
.colhead:hover, .rowhead:hover, .subrow:hover {{ color: {fg}; }}
.hide-action {{ padding: 0; margin: 0; background-color: alpha({ground}, 0.90); }}
.appname {{ font-size: 12px; }}
.subrow {{ font-size: 10px; color: {muted}; padding-right: 6px; }}
.dim {{ color: {dim}; font-size: 12px; font-style: italic; }}
.hint {{ color: #666666; font-size: 11px; margin-top: 12px; }}
.hint b {{ color: {accent}; }}
",
        ground = t.ground,
        surface = t.surface,
        fg = t.fg,
        muted = t.muted,
        dim = t.dim,
        accent = t.accent,
        border = t.border,
    )
}

/// Where the highlight is. Everything a repaint needs to know, and nothing it does not.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Cursor {
    col: usize,
    row: usize,
    line: usize,
    item: usize,
    inside: bool,
}

struct LineW {
    bx: GBox,
    apps: Vec<GBox>,
}

struct CellW {
    bx: GBox,
    lines: Vec<LineW>,
    /// A long vector scrolls inside its own cell. Without this boundary the vector's
    /// natural width becomes the width of the entire machine column, leaving the same blank
    /// expanse in every ordinary row above it.
    rail: Option<gtk::ScrolledWindow>,
}

/// Handles for the widgets that carry selection state, so moving the cursor does not have to
/// rebuild the grid to find them again.
///
/// Deliberately NOT a mirror of the whole tree: nothing here is the icons, the labels or the drop
/// targets, because a repaint never touches those. It holds exactly the widgets whose CSS classes
/// change when the cursor moves, which is what keeps a repaint O(1) in the size of the grid.
#[derive(Default)]
struct Painted {
    rowheads: Vec<Label>,
    /// `[row][col]`, matching the grid's own indexing so a Cursor reads straight through.
    cells: Vec<Vec<CellW>>,
    /// What was highlighted at the last paint. The un-highlight step reads this rather than
    /// searching, which is the whole reason a repaint costs the same on a full grid as an empty one.
    last: Option<Cursor>,
}

impl Painted {
    fn reset(&mut self) {
        self.rowheads.clear();
        self.cells.clear();
        self.last = None;
    }

    /// Add (`on`) or remove (`!on`) the selection classes for one cursor position.
    ///
    /// Every lookup is fallible and simply does nothing when it misses. A cursor can legitimately
    /// point past the end -- an app filtered away by a query, a line emptied by a drag -- and the
    /// clamp that fixes it runs against the model, not against the widgets. Painting must not be
    /// the thing that panics on a state the model considers ordinary.
    fn mark(&self, c: Cursor, on: bool) {
        if let Some(rh) = self.rowheads.get(c.row) {
            class(rh, "active", on);
        }
        let Some(cell) = self.cells.get(c.row).and_then(|r| r.get(c.col)) else {
            return;
        };
        class(&cell.bx, "cursor", on);
        if !c.inside {
            return;
        }
        class(&cell.bx, "inside", on);
        if let Some(line) = cell.lines.get(c.line) {
            class(&line.bx, "sel", on);
            if let Some(app) = line.apps.get(c.item) {
                class(app, "sel", on);
            }
        }
    }
}

fn class<W: IsA<gtk::Widget>>(w: &W, name: &str, on: bool) {
    if on {
        w.add_css_class(name);
    } else {
        w.remove_css_class(name);
    }
}

// glibc's allocator, told to hand image buffers straight back to the kernel.
//
// Decoding 95 icons churns large blocks: a 1024x1024 source is 4MB, and even downscaled output is
// allocated after the full-size buffer exists. glibc frees those to its own arena rather than to
// the OS, so the process kept ~55MB that nothing referenced -- measured as the gap between the
// icon cache's 70MB private and the 15MB private of the same grid with icons switched off, which
// no amount of retained TEXTURE could explain: all 95 at full source size come to 12MB.
//
// M_MMAP_THRESHOLD pinned low makes every allocation this size mmap'd, so `free` unmaps it and the
// memory is gone immediately rather than pooled. The default is adaptive and grows to 32MB, which
// is right for a server recycling buffers and wrong for a program that decodes images once.
// M_TRIM_THRESHOLD makes the remaining arena give ground back rather than hold its high-water mark.
unsafe extern "C" {
    fn mallopt(param: i32, value: i32) -> i32;
    fn malloc_trim(pad: usize) -> i32;
}
const M_TRIM_THRESHOLD: i32 = -1;
const M_MMAP_THRESHOLD: i32 = -3;

unsafe extern "C" {
    fn madvise(addr: *mut core::ffi::c_void, length: usize, advice: i32) -> i32;
}
/// Linux's "reclaim this now, I am done with it" -- the volunteer form of what memory pressure
/// would eventually do anyway.
const MADV_PAGEOUT: i32 = 21;

/// How far the search bar may be squeezed to let a narrow output honour its width cap.
///
/// A floor rather than nothing at all: the point of giving width back is to keep the far machine
/// column on screen, and a search bar squeezed to a few characters would trade one unusable thing
/// for another. Below this the screen is simply too narrow for the configured grid, which is a
/// scrollbar's problem and not a sizing one.
const MIN_SEARCH_WIDTH: i32 = 200;

/// Hand the idle heap back to the kernel, which on this class of machine means handing it to a
/// compressor.
///
/// A resident launcher spends almost all of its life hidden. What it holds then is ~7MB of clean
/// file-backed pages -- GTK's own code, which the kernel can simply drop and re-read, so it costs
/// nothing to leave alone -- and ~15MB of dirty anonymous pages: the widget tree, the textures,
/// the model. Anonymous pages have no file behind them, so the only way to reclaim them is swap,
/// and they are therefore the entire cost of staying resident.
///
/// MADV_PAGEOUT offers them up at the moment the window hides, rather than waiting for the machine
/// to come under pressure and find them. Where swap is fronted by zswap or backed by zram -- zstd
/// here -- they land compressed in RAM.
///
/// MEASURED, because the estimate was wrong: 56 regions advised, resident 84.6MB -> 83.0MB. About
/// 1.6MB, not the ten-plus this was projected to save. `malloc_trim` above had already returned the
/// bulk of it (52MB of dirty down to 35MB when it was introduced), and what remains is genuinely
/// live -- the widget tree GTK is still holding, which is the whole point of staying resident.
/// Kept because it costs nothing and a hidden launcher should not sit on what it is not using, but
/// the case for residency rests on the 22MB it occupies, not on this.
///
/// Anonymous and writable only, and never the stack: this is about data the program is finished
/// with for now, not about pages it is standing on. A failure anywhere is ignored -- the kernel
/// declining to reclaim is not a reason for a launcher to misbehave.
/// The last thing every machine printed, kept so an unchanged answer can be recognised without
/// parsing it. See `inventory_bytes` for why the raw output is the right thing to compare.
struct Inventories {
    printed: Vec<Result<Vec<u8>, String>>,
    rows: Vec<String>,
    layout: config::Layout,
}

/// Whether to print the machine-readable trace, decided once from `NIXLAUNCH_TRACE`.
fn tracing_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("NIXLAUNCH_TRACE").is_some())
}

/// One line of machine-readable progress, for the headless session check.
///
/// A LAYER SURFACE IS INVISIBLE TO THE COMPOSITOR'S IPC -- it is not a window and never appears in
/// `get_tree` -- so a test has no way to ask where this one mapped, how large it is, or how often
/// it resized. Without something like this the only observable is pixels, and asserting on
/// screenshots means a test that fails whenever a font or a colour changes.
///
/// Off unless asked for, printed to stderr rather than stdout, and deliberately dull: `key=value`
/// pairs a shell can grep, never a format anything is expected to parse cleverly.
fn trace(fields: std::fmt::Arguments<'_>) {
    if tracing_on() {
        eprintln!("nixlaunch-trace {fields}");
    }
}

fn release_idle_pages() {
    unsafe {
        malloc_trim(0);
    }
    let Ok(maps) = std::fs::read_to_string("/proc/self/maps") else {
        return;
    };
    for line in maps.lines() {
        let mut parts = line.split_whitespace();
        let (Some(range), Some(perms)) = (parts.next(), parts.next()) else {
            continue;
        };
        // A fourth field beyond the offset/dev/inode means the mapping is file-backed, and those
        // are the ones already cheap to reclaim.
        if parts.clone().count() > 3 {
            continue;
        }
        if !perms.starts_with("rw") {
            continue;
        }
        let Some((lo, hi)) = range.split_once('-') else {
            continue;
        };
        let (Ok(lo), Ok(hi)) = (usize::from_str_radix(lo, 16), usize::from_str_radix(hi, 16))
        else {
            continue;
        };
        // The stack grows; advising it away is asking for a fault on the way back out of here.
        let sp = &lo as *const usize as usize;
        if sp >= lo && sp < hi {
            continue;
        }
        unsafe {
            madvise(lo as *mut core::ffi::c_void, hi - lo, MADV_PAGEOUT);
        }
    }
}

// The way back in, published so the single-instance path can reach the window that already
// exists. A `//` comment, not a doc comment: a doc comment on a macro invocation attaches to
// nothing and the compiler says so.
thread_local! {
    static REVEAL: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Start resident but invisible, so the FIRST press of the day is as fast as the second.
///
/// Residency only pays from the second open onwards, and the first is the one a person actually
/// notices -- it is the press after login, when nothing is warm. A session service runs
/// `nixlaunch --daemon` at start: the process builds everything, asks the machines, decodes what
/// the icon cache does not have, and then does not show itself. The first real press is already
/// a map.
///
/// Checked from argv directly rather than through GApplication's command line handling, because
/// the single-instance path must NOT inherit it: `nixlaunch --daemon` starts the primary hidden,
/// and a later plain `nixlaunch` fires `activate` on that same process, which presents it. Two
/// different meanings for two different invocations, and only the first one reads a flag.
fn start_hidden() -> bool {
    std::env::args().any(|a| a == "--daemon")
}

fn main() {
    // The renderer choice belongs to the PROGRAM, not to one packaging of it.
    //
    // It was set by the Nix wrapper, which is correct there and reaches nobody else: a distro
    // package, a cargo build, someone running it from a checkout all got GTK's own GPU probe and
    // therefore Vulkan -- device init, shader compilation and a driver thread pool spent on a
    // surface made of boxes and labels, all of it between the keypress and the window. Setting it
    // here means every way of building this program gets the same measured-best default.
    //
    // Only when unset, so GSK_RENDERER in the environment still wins and the comparison stays
    // reproducible without a rebuild.
    if std::env::var_os("GSK_RENDERER").is_none() {
        unsafe { std::env::set_var("GSK_RENDERER", "cairo") };
    }

    unsafe {
        mallopt(M_MMAP_THRESHOLD, 128 * 1024);
        mallopt(M_TRIM_THRESHOLD, 256 * 1024);
    }
    let application = Application::builder()
        .application_id("io.github.nixlaunch")
        .build();
    application.connect_activate(|app| {
        // GApplication is single-instance by default, so a second launch does not start a second
        // process -- it fires `activate` on the running one. Building unconditionally there
        // stacked a second layer surface over the first, leaked another style provider, and gave
        // the two windows independent snapshots of placement.json and usage.json that then
        // overwrote each other. Present what already exists.
        if let Some(existing) = app.windows().first() {
            // RESIDENT. The window is hidden rather than destroyed when the launcher is dismissed,
            // so this is a map and not a start: no exec, no dynamic link, no GTK init, no
            // inventory, no icon decode. Everything that made a cold open cost 117ms already
            // happened once.
            REVEAL.with(|r| {
                if let Some(f) = r.borrow().as_ref() {
                    f();
                }
            });
            existing.present();
            return;
        }
        build(app);
    });
    // OUR flags are ours, and GApplication must never see them. It parses argv itself and rejects
    // anything it does not recognise: `--daemon` produced "Unknown option --daemon" on stderr and
    // an immediate exit 0. The symptom was a daemon that started and vanished a moment later,
    // which reads exactly like a lifetime bug -- and sent me to `hold()` first, which was not the
    // problem at all. Filtering here is the fix; `start_hidden` still reads the real argv.
    let argv: Vec<String> = std::env::args().filter(|a| a != "--daemon").collect();

    // `run_with_args(&argv)`, NOT `run_with_args(&[])`. An EMPTY argv is not "no arguments" --
    // argv[0] is the program name and g_application_run treats the vector as malformed without
    // it, returning without ever emitting `activate`. The symptom is the worst kind: the process
    // starts, GTK initialises far enough to probe Vulkan, and then simply exits no window, no
    // error. Cost an iteration to find; noted here so it costs nobody another one.
    application.run_with_args(&argv);
}

/// The first configured output that is actually attached, if any.
///
/// TWO KINDS OF MATCH, because a monitor has two kinds of name and they behave differently.
///
/// The CONNECTOR (`DP-1`, `HDMI-A-1`) is an exact handle and is matched exactly: it is short,
/// there is one per output, and a substring rule on something that short would have `DP-1` claim
/// `DP-11`.
///
/// Everything else is matched as a case-insensitive SUBSTRING of the monitor's descriptive names,
/// and that is not laxity, it is the shape of the data. GDK fills `manufacturer` and `model` only
/// when the backend hands them over separately; on wlroots compositors both come back as the
/// literal string `Unknown` and the entire identity arrives in the DESCRIPTION instead:
///
///     connector "DP-1"   manufacturer "Unknown"   model "Unknown"
///     description "Dell Inc. DELL U4323QE DPMH1P3 (DP-1)"
///
/// A configured `DELL U4323QE` has to find that. Exact-matching a field that also carries the
/// vendor, the serial and the connector could only ever fail, and failing here is quiet -- it
/// falls back to the compositor's choice, which looks exactly like the option not being read.
fn preferred_monitor(outputs: &[String]) -> Option<gtk::gdk::Monitor> {
    if outputs.is_empty() {
        return None;
    }
    let display = gtk::gdk::Display::default()?;
    let monitors = display.monitors();
    let attached: Vec<gtk::gdk::Monitor> = (0..monitors.n_items())
        .filter_map(|i| monitors.item(i).and_downcast::<gtk::gdk::Monitor>())
        .collect();
    // ORDER COMES FROM THE CONFIG, not from the display list: the outer loop is the preference and
    // the inner one is merely what is plugged in. Iterating the monitors outside would return
    // whichever screen the compositor happens to list first among the matches, which is exactly the
    // arbitrary answer this option exists to replace.
    outputs.iter().find_map(|wanted| {
        let wanted = wanted.trim().to_lowercase();
        if wanted.is_empty() {
            return None;
        }
        attached
            .iter()
            .find(|monitor| monitor_matches(monitor, &wanted))
            .cloned()
    })
}

/// Whether one already-lowercased configured name identifies this monitor.
fn monitor_matches(monitor: &gtk::gdk::Monitor, wanted: &str) -> bool {
    let text = |value: Option<gtk::glib::GString>| {
        value.map(|s| s.trim().to_lowercase()).unwrap_or_default()
    };
    if text(monitor.connector()) == wanted {
        return true;
    }
    // "Unknown" is not a name, it is GDK saying it was told nothing -- and it is the SAME
    // non-answer on every output, so honouring it would make one configured word match whichever
    // screen happened to be enumerated first.
    let known = |value: String| {
        if value.is_empty() || value == "unknown" {
            None
        } else {
            Some(value)
        }
    };
    let manufacturer = known(text(monitor.manufacturer()));
    let model = known(text(monitor.model()));
    let described = known(text(monitor.description()));
    let full = match (&manufacturer, &model) {
        (Some(m), Some(n)) => Some(format!("{m} {n}")),
        _ => None,
    };
    [model, described, full, manufacturer]
        .into_iter()
        .flatten()
        .any(|name| name.contains(wanted))
}

fn build(application: &Application) {
    let World {
        folders,
        machines: base,
        theme,
        layout,
        terminal: terminal_cmd_outer,
        surface: surface_mode,
        keyboard: keyboard_mode,
        exit_on_focus_loss,
        config: loaded_config,
        error: config_error,
    } = load_world();
    // A placement that exists and does not parse is reported, never assumed empty: the next drag
    // rewrites whatever we decide it was, so guessing "nothing" would overwrite a real arrangement.
    let (placement, placement_error) = load_placement();
    let (visibility, visibility_error) = load_visibility();
    let (loaded_usage, usage_error) = usage::load();
    let placement_writable = placement_error.is_none();
    let visibility_writable = visibility_error.is_none();
    let usage_writable = usage_error.is_none();
    let startup_error = config_error
        .clone()
        .or(placement_error)
        .or(visibility_error)
        .or(usage_error);

    // NO default size. A launcher should be exactly as big as what it is showing: a fixed size
    // leaves dead space under a short grid and clips a tall one, and BOTH are wrong for a surface
    // whose whole content is known before it maps. Unanchored, a layer surface takes the natural
    // size GTK computes from the children, so the window hugs the matrix and grows with it -- one
    // more folder row makes it taller, a fourth machine makes it wider, with nothing to re-tune.
    //
    // The search entry carries the only explicit measurement, a minimum width, so an empty or
    // heavily-filtered grid cannot collapse the window to a sliver mid-keystroke.
    let window = ApplicationWindow::builder()
        .application(application)
        .build();

    // Layer shell: an overlay that OWNS the keyboard while open. Exclusive rather than OnDemand
    // because every key here is a navigation key -- a launcher that only half-takes the keyboard
    // sends arrow keys to whatever was focused underneath it.
    //
    // NIXLAUNCH_NO_LAYER=1 drops back to an ordinary toplevel. That is not a fallback for hosts
    // without layer-shell (there are none here); it is for WORKING ON THIS FILE. A layer surface
    // is invisible to the compositor's window tree and grabs the keyboard exclusively, which makes
    // it exactly the wrong thing to iterate a layout inside -- a toplevel tiles, appears in
    // `get_tree`, and can be screenshotted and closed like anything else.
    if surface_mode == "layer" && std::env::var_os("NIXLAUNCH_NO_LAYER").is_none() {
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        // EXCLUSIVE by default, and this is a correction: on-demand reads like the polite choice
        // and does not work. On every released sway (1.10-1.12) and its forks, `handle_map` grants
        // a mapping layer surface focus and then the `arrange_layers` call at the end of the SAME
        // handler takes it straight back for anything that is not EXCLUSIVE -- so the launcher
        // maps and never receives a key. Every shipping launcher defaults to exclusive for this
        // reason. Configurable, because the day a compositor fixes it, on-demand is the nicer
        // behaviour and nobody should need a new build to use it.
        //
        // SET BEFORE `present()`, always: sway reads the mode at map time out of the surface's
        // INITIAL commit, and gtk4-layer-shell only puts it there if it was set on the window
        // before the surface was created. A mode applied after presenting is silently ignored.
        window.set_keyboard_mode(match keyboard_mode.as_str() {
            "ondemand" => KeyboardMode::OnDemand,
            "none" => KeyboardMode::None,
            _ => KeyboardMode::Exclusive,
        });
    }

    let provider = CssProvider::new();
    provider.load_from_string(&css(&theme));
    if let Some(display) = gtk::gdk::Display::default() {
        // APPLICATION (600), not 800. 800 is GTK_STYLE_PROVIDER_PRIORITY_USER -- the slot GTK
        // reserves for the user's OWN ~/.config/gtk-4.0/gtk.css. An application sheet sitting
        // there outranks the one escape hatch a consumer has left, so a palette they could not
        // otherwise reach also could not be overridden by the mechanism GTK provides for exactly
        // that. This is a correctness bug, not a preference: an app must never occupy the user's
        // priority slot.
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    if let Some(e) = &config_error {
        eprintln!("nixlaunch: {e}");
    }

    // One theme handle and one texture cache for the life of the process. The active theme and its
    // search path are part of the cache stamp, so they must exist before the persisted pixels load.
    let icon_theme = gtk::gdk::Display::default()
        .map(|d| gtk::IconTheme::for_display(&d))
        .unwrap_or_default();
    let icon_cache = Rc::new(RefCell::new(icons::Icons::load(
        theme.icon_size,
        &icon_theme,
    )));

    let state = Rc::new(RefCell::new(State {
        folders,
        layout: layout.clone(),
        usage: loaded_usage,
        usage_writable,
        // Two standard errors, ~95% confidence. Lower and the grid twitches; higher and a real
        // preference takes weeks to show up.
        z: 2.0,
        half_life_days: usage::HALF_LIFE_DAYS,
        base,
        placement,
        placement_writable,
        visibility,
        visibility_writable,
        machines: Vec::new(),
        view: Vec::new(),
        col: 0,
        row: 0,
        line: 0,
        item: 0,
        item_goal: 0,
        focus: Focus::Outside,
        query: String::new(),
    }));
    let layout = Rc::new(RefCell::new(layout));
    // Carry arrangements written before ids existed. Runs before the first rebuild, because a
    // rebuild against unmigrated state would find nothing and draw the computed grouping -- which
    // looks exactly like "my arrangement is gone".
    state.borrow_mut().migrate_names_to_ids();

    // Applies saved filings, then populates `view` with an empty query, i.e. everything.
    state.borrow_mut().rebuild();

    // Once the grid exists, give back what building it borrowed. This runs after the first frame
    // rather than before it, because the point is to return decode scratch AFTER the decoding is
    // done -- and it is deliberately not on the rebuild path: walking the heap on every keystroke
    // would trade the memory back for the latency this program is judged on.
    {
        // After the first frame, not before it: writing the cache is worth nothing until the
        // decoding that fills it has happened, and doing either ahead of the draw would be paying
        // latency to save latency.
        let icon_cache = icon_cache.clone();
        gtk::glib::idle_add_local_once(move || {
            icon_cache.borrow().save();
            unsafe {
                malloc_trim(0);
            }
        });
    }

    let root = GBox::new(Orientation::Vertical, 0);
    root.add_css_class("root");

    // The search bar begins where the FIRST MACHINE COLUMN begins, not at the window edge: it
    // searches the machines, not the folder labels, so starting it over the label gutter would
    // line it up with the one thing it has nothing to do with. `spacer` is an empty widget put in
    // the same size group as the folder labels each render, so it tracks that column's real width
    // instead of guessing a margin that goes wrong the moment a folder is renamed.
    let searchrow = GBox::new(Orientation::Horizontal, 0);
    // THE CORNER, and the search box starting where the MACHINES start.
    //
    // The search row reserved the width of one label column, which was right while there was one.
    // With a folder column and a subcategory column beside it the box began under the
    // subcategories instead of under the first machine, so the thing you type into did not line up
    // with the thing it filters.
    //
    // Two spacers, one in each label column's size group, so the corner is exactly as wide as both
    // -- computed rather than guessed, and it stays right when a longer folder name changes the
    // first column's width.
    let spacer = GBox::new(Orientation::Horizontal, 0);
    let spacer_folder = Label::new(None);
    let spacer_sub = Label::new(None);
    spacer.append(&spacer_folder);
    spacer.append(&spacer_sub);

    // And since that corner is now a real space rather than a gap, it can hold something. Empty
    // unless configured: a launcher shipping someone else's mark would be wearing it.
    if !theme.logo.is_empty() {
        let logo = if std::path::Path::new(&theme.logo).is_absolute() {
            Image::from_file(&theme.logo)
        } else {
            Image::from_icon_name(&theme.logo)
        };
        logo.set_pixel_size(theme.logo_size);
        logo.set_halign(Align::Start);
        spacer.append(&logo);
    }
    searchrow.append(&spacer);

    let search = Label::new(None);
    search.set_xalign(0.0);
    search.set_hexpand(true);
    search.add_css_class("search");
    search.set_width_request(theme.width);
    searchrow.append(&search);
    root.append(&searchrow);

    // A real inventory is hundreds of applications, and "size to content" -- correct for the
    // fixture -- turns that into a window taller than the display, clipped at both ends with no
    // way to reach the middle. The grid keeps its natural size until it hits a ceiling and then
    // scrolls, so a handful of machines still gets a window that hugs its content.
    let scroller = gtk::ScrolledWindow::new();
    // BOTH AXES. Horizontal was Never, which is only safe while the content happens to fit: add
    // machines until the grid is wider than the display and the far columns become unreachable by
    // any means at all -- no scrollbar, no keyboard, and a layer surface has no titlebar to drag.
    // The same failure the height cap already existed to prevent, on the axis nobody had hit yet.
    scroller.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    scroller.set_propagate_natural_height(true);
    scroller.set_propagate_natural_width(true);
    // Relative to the DISPLAY, not a magic number. A launcher that is 820px tall is fine on this
    // panel and wrong on the next one, and the failure is not cosmetic -- overrun the screen and
    // the top and bottom are simply unreachable, since a layer surface has no titlebar to drag.
    // Two thirds leaves the session visible behind it, which is most of why this is an overlay
    // rather than a window.
    // Before the surface maps GTK cannot tell us which output the compositor will choose, so the
    // opening cap is the SMALLEST attached one on BOTH axes. Overflowing a panel is the failure
    // with no way out -- a layer surface has no titlebar to drag, so whatever went past the edge
    // is simply gone -- where a window that maps too small corrects itself one signal later, as
    // soon as the compositor names the output it actually put us on.
    let smallest = |pick: fn(&gtk::gdk::Rectangle) -> i32, fallback: i32| {
        gtk::gdk::Display::default()
            .map(|d| {
                let ms = d.monitors();
                (0..ms.n_items())
                    .filter_map(|i| ms.item(i).and_downcast::<gtk::gdk::Monitor>())
                    .map(|m| pick(&m.geometry()))
                    .filter(|v| *v > 0)
                    .min()
                    .unwrap_or(fallback)
            })
            .unwrap_or(fallback)
    };
    let screen_h = smallest(|g| g.height(), 1080);
    let screen_w = smallest(|g| g.width(), 1920);
    scroller.set_max_content_height((screen_h as f64 * theme.max_height_fraction) as i32);
    // The width cap is the same rule as the height one and exists for the same reason: content is
    // allowed to decide the window's size right up to the point where it would put part of itself
    // off the screen, and past that it scrolls instead.
    scroller.set_max_content_width((screen_w as f64 * theme.max_width_fraction) as i32);

    // THE OUTPUT WE ARE ACTUALLY ON -- on every map, on both axes, in both directions.
    //
    // A resident process makes this decision repeatedly, and it used to be made once and then
    // remembered. Whichever output the FIRST resolution happened to name sized the window for the
    // rest of the process, which on a desk with screens of different sizes is a launcher that opens
    // with machine columns hanging off the right edge and never recovers:
    //
    //   * The cap was applied with `set_default_width`, and a GTK default size is REMEMBERED. It
    //     survives the hide, and every later size negotiation is computed from it rather than from
    //     what the content measures, so one narrow answer outlives the situation that produced it.
    //   * Raising the ScrolledWindow's maximum again on a wider output did nothing, because that
    //     maximum only bounds a natural width the window had stopped consulting.
    //
    // So the caps are re-derived from the entered output every map, height included -- deriving it
    // from the smallest attached screen was the same mistake one axis over -- and the size is
    // measured and requested outright rather than left to be remembered.
    let apply_monitor: Rc<dyn Fn(&gtk::gdk::Monitor)> = Rc::new({
        let scroller = scroller.clone();
        let root = root.clone();
        let search = search.clone();
        let window = window.clone();
        let search_width = theme.width;
        let width_fraction = theme.max_width_fraction;
        let height_fraction = theme.max_height_fraction;
        move |monitor: &gtk::gdk::Monitor| {
            let t_settle = std::time::Instant::now();
            let geometry = monitor.geometry();
            if geometry.width() <= 0 || geometry.height() <= 0 {
                return;
            }
            let width_cap = (geometry.width() as f64 * width_fraction) as i32;
            scroller.set_max_content_width(width_cap);
            scroller.set_max_content_height((geometry.height() as f64 * height_fraction) as i32);

            // THE SEARCH BAR DOES NOT GET TO OUTVOTE THE SCREEN.
            //
            // `theme.width` is a minimum, and a minimum is a floor no cap can reach under: at the
            // default 560, plus the row's padding and the label column beside it, the window cannot
            // measure much under 700px however small the fraction makes the cap. On a narrow panel
            // that quietly turns the cap off -- on precisely the screen it exists to protect, since
            // a wide one was never in danger. Hand the excess back by asking the search bar for
            // less: it is the only widget here whose width is a preference rather than content, and
            // a shorter search bar is a smaller thing to lose than the right-hand machine column.
            //
            // Reset first, so returning to a large screen restores the configured width instead of
            // inheriting whatever the smallest screen visited so far settled on.
            search.set_width_request(search_width);
            let (minimum, _, _, _) = root.measure(gtk::Orientation::Horizontal, -1);
            let overrun = minimum - width_cap;
            if overrun > 0 {
                search.set_width_request((search_width - overrun).max(MIN_SEARCH_WIDTH));
            }

            // MEASURE, THEN ASK FOR EXACTLY THAT -- rather than clearing the size and hoping the
            // window follows its content down.
            //
            // It will not. Dropping the default size lets a toplevel GROW, because GTK has to
            // honour a minimum that just increased, and that half works on its own. Shrinking is
            // not symmetric: a mapped toplevel keeps whatever size it has, and a smaller cap
            // merely leaves the ScrolledWindow with room to spare, so the window sails on at its
            // old width with most of it hanging off the side of a smaller screen. Only an explicit
            // request moves it in both directions, and the size to request is the one the content
            // would have chosen anyway, now that the caps describe the output we are really on.
            let (_, width, _, _) = root.measure(gtk::Orientation::Horizontal, -1);
            let (_, height, _, _) = root.measure(gtk::Orientation::Vertical, width);
            window.set_default_size(width, height);
            trace(format_args!(
                "settle us={} output={} screen={}x{} size={}x{} cap={}x{} min={}",
                t_settle.elapsed().as_micros(),
                monitor.connector().unwrap_or_default(),
                geometry.width(),
                geometry.height(),
                width,
                height,
                width_cap,
                (geometry.height() as f64 * height_fraction) as i32,
                minimum
            ));
        }
    });

    {
        let apply_monitor = apply_monitor.clone();
        window.connect_realize(move |w| {
            let Some(surface) = w.surface() else { return };
            // ONCE PER MAP, and this guard is load-bearing rather than tidiness.
            //
            // A window large enough to overlap two outputs gets `enter-monitor` for both. Sizing
            // for the second changes which outputs it overlaps, which delivers another enter, which
            // resizes it back: measured on a two-output session, one open resized the window five
            // times, alternating between the two caps before settling. Every one of those is a full
            // re-measure and relayout of the whole grid, and it is visible.
            //
            // The surface does not migrate while it is up -- a launcher is opened, used and
            // dismissed -- so the first output named after a map is the answer, and later enters
            // for the same showing have nothing to add. The flag reopens on the next map.
            let settled = Rc::new(std::cell::Cell::new(false));
            surface.connect_enter_monitor({
                let apply_monitor = apply_monitor.clone();
                let settled = settled.clone();
                move |_, monitor| {
                    if settled.replace(true) {
                        return;
                    }
                    apply_monitor(monitor);
                }
            });
            // A BACKSTOP FOR BACKENDS THAT ENTER EARLY. Some deliver the first `enter` while the
            // surface is still being realised -- before the handler above exists -- so that map
            // would keep the pre-map guess with no signal ever arriving to correct it. Probing
            // shortly after each map covers that ordering, and it is guarded on being mapped
            // because an unmapped surface is exactly the question that produced the wrong answer
            // above.
            w.connect_map({
                let apply_monitor = apply_monitor.clone();
                let settled = settled.clone();
                move |w| {
                    // A new showing is a new question: the launcher may well be opened on a
                    // different screen than it was last time, and the guard above must not answer
                    // for a map it never saw.
                    settled.set(false);
                    trace(format_args!("map"));
                    let window = w.clone();
                    let apply_monitor = apply_monitor.clone();
                    gtk::glib::timeout_add_local_once(
                        std::time::Duration::from_millis(100),
                        move || {
                            if !window.is_mapped() {
                                return;
                            }
                            let (Some(surface), Some(display)) =
                                (window.surface(), gtk::gdk::Display::default())
                            else {
                                return;
                            };
                            if let Some(monitor) = display.monitor_at_surface(&surface) {
                                apply_monitor(&monitor);
                            }
                        },
                    );
                }
            });
        });
    }

    // WHICH SCREEN IT OPENS ON, when the compositor's answer is not the wanted one.
    //
    // Nothing configured is the default and means the compositor decides, which is right: it knows
    // where you are working and a launcher that overrides that uninvited is worse than one that
    // never tries. `outputs` is for the desk where that answer is reliably wrong -- a big screen
    // beside a small vertical one, where the launcher wants to be on the big one every time
    // regardless of which window happened to have focus.
    //
    // Applied on EVERY reveal rather than once, and before `present`, because both halves are what
    // make it work: a layer surface's output is decided at map time, and the size we want is the
    // one for the screen we are about to appear on -- computing it here means the first frame is
    // already the right size instead of being corrected a frame later, in front of you.
    let prefer_output: OutputPreference = Rc::new({
        let window = window.clone();
        let apply_monitor = apply_monitor.clone();
        let layered = surface_mode == "layer" && std::env::var_os("NIXLAUNCH_NO_LAYER").is_none();
        move |outputs: &[String]| {
            let Some(monitor) = preferred_monitor(outputs) else {
                // Either nothing is configured, or what is configured is not plugged in right now.
                // Hand the decision back rather than pinning the launcher to a screen that is not
                // there -- which is how a docked-desk preference would otherwise open a window
                // nobody can see once the laptop is carried away.
                if layered {
                    window.set_monitor(None);
                }
                return;
            };
            if layered {
                window.set_monitor(Some(&monitor));
            }
            apply_monitor(&monitor);
        }
    });

    let grid = gtk::Grid::new();
    // NOT column-homogeneous. That makes EVERY column the same width including column 0, which
    // holds nothing but the folder labels -- so the label column gets sized like a machine column
    // and leaves a wide empty gutter down the left with the labels shoved into its far edge.
    // Instead the machine columns carry `hexpand` (set per cell below) and share the spare width
    // equally between themselves, while column 0 takes only the width its longest label needs.
    grid.set_column_homogeneous(false);
    scroller.set_child(Some(&grid));
    root.append(&scroller);

    let hint = Label::new(None);
    hint.set_xalign(0.0);
    hint.set_use_markup(true);
    hint.add_css_class("hint");
    root.append(&hint);

    window.set_child(Some(&root));

    // CLOSE WHEN THE KEYBOARD GOES ELSEWHERE. This is the half that makes an exclusive grab
    // acceptable, and leaving it out is what let this thing swallow real typing: a launcher that
    // holds the seat and stays open is a lock screen with an app list on it.
    //
    // Gated on having been active at least once, because a surface is briefly inactive between
    // mapping and being focused -- closing on that first false would make it flash and vanish.
    // DISMISS, not exit. Hiding keeps the window, the widget tree, the inventory and the decoded
    // icons alive, so the next open is a map rather than a start. GtkApplication only exits when
    // the last window is DESTROYED, so a hidden one keeps the process resident with no explicit
    // hold. What the process does NOT keep is the memory it is not using: see release_idle_pages.
    let dismiss: Rc<dyn Fn(&ApplicationWindow)> = Rc::new(|w: &ApplicationWindow| {
        w.set_visible(false);
        release_idle_pages();
    });

    // A GRACE PERIOD ON EVERY REVEAL, not merely after process startup. A daemon starts hidden and
    // may wait hours for its first map; arming once during build means every real open has already
    // expired the guard and one focus bounce from a bar or dock dismisses it immediately.
    // A deadline rather than a timer. Replacing it on every reveal really restarts the grace;
    // overlapping one-shot timers let an older reveal re-arm dismissal too early.
    let focus_ready_at = Rc::new(std::cell::Cell::new(None::<std::time::Instant>));
    let arm_focus: Rc<dyn Fn()> = Rc::new({
        let focus_ready_at = focus_ready_at.clone();
        move || {
            focus_ready_at
                .set(std::time::Instant::now().checked_add(std::time::Duration::from_millis(400)));
        }
    });
    if exit_on_focus_loss {
        let dismiss_on_blur = dismiss.clone();
        let focus_ready_at = focus_ready_at.clone();
        window.connect_is_active_notify(move |w| {
            let ready = focus_ready_at
                .get()
                .is_some_and(|deadline| std::time::Instant::now() >= deadline);
            if !w.is_active() && ready {
                dismiss_on_blur(w);
            }
        });
    }

    // ── TWO KINDS OF UPDATE, AND WHY THEY ARE NOT THE SAME FUNCTION ──────────────────────────
    //
    // Moving the cursor and changing the query are utterly different amounts of work, and treating
    // them alike is what made this slow. The grid is ~900 widgets on a real three-machine
    // inventory; rebuilding it costs hundreds of milliseconds, and it used to happen on EVERY
    // keypress -- including the arrow keys, whose entire effect is to move one highlight.
    //
    //   `render`  STRUCTURAL. Tears the grid down and builds it again. Needed only when the set of
    //             things on screen changes: a new query filters apps out, a drag re-files one, a
    //             launch reorders by frecency.
    //   `paint`   COSMETIC. Moves the selection classes from where they were to where they now
    //             are, and updates the two text labels. Touches at most eight widgets regardless
    //             of how many are on screen, because it remembers what it highlighted last time.
    //
    // Arrow keys, Tab and Enter-into-a-cell take the second path, which is the one the user is in
    // for most of a session -- this launcher's whole premise is that you navigate a grid rather
    // than type at it.
    // The bindings, resolved once. Defaults unless configuration says otherwise -- see the keymap
    // module on why overriding beats replacing wholesale.
    let keys_map: Rc<keymap::Keymap> = Rc::new(
        loaded_config
            .as_ref()
            .map(|c| keymap::Keymap::from_overrides(&c.keys))
            .unwrap_or_default(),
    );

    let painted: Rc<RefCell<Painted>> = Rc::new(RefCell::new(Painted::default()));

    // `render` must be callable from inside a drop handler that `render` itself installed, so it
    // needs a handle to itself. The holder is that indirection -- filled in immediately after
    // construction, and only ever read while no other borrow of it is live.
    let render_holder: CallbackSlot = Rc::new(RefCell::new(None));

    let paint: Rc<dyn Fn()> = Rc::new({
        let state = state.clone();
        let layout = layout.clone();
        let painted = painted.clone();
        let search = search.clone();
        let hint = hint.clone();
        let theme_error = theme.error.clone();
        let config_err = startup_error.clone();
        let scroller = scroller.clone();
        let grid_for_scroll = grid.clone();
        move || {
            let s = state.borrow();

            // A config that EXISTS but does not parse is shown IN THE WINDOW, not just on stderr.
            // This surface is launched from a compositor keybind, so nothing is watching stderr,
            // and the fallback is fixture data -- inventing machines while the real ones failed to
            // load is precisely the outcome config.rs's own comment says must not happen silently.
            if let Some(err) = &config_err {
                search.set_markup(&format!(
                    "<span foreground=\"{}\">startup problem:</span> {}",
                    theme_error,
                    escape(err)
                ));
                search.remove_css_class("empty");
            } else if s.query.is_empty() {
                search.set_text("type to search\u{2026}");
                search.add_css_class("empty");
            } else {
                search.set_text(&s.query);
                search.remove_css_class("empty");
            }

            let rail = s.folders.get(s.row).is_some_and(|row| {
                layout
                    .borrow()
                    .is_rail(row, s.cell().iter().map(|line| line.apps.len()).sum())
            });
            let base_hint = match (s.focus, rail) {
                (Focus::Outside, false) => {
                    "<b>\u{2190}\u{2192}</b> machine   <b>\u{2191}\u{2193}</b> folder   <b>Tab</b>/<b>Enter</b> inside   <b>Shift+Enter</b> launch cell   <b>drag</b> file/reorder   <b>Esc</b> close"
                }
                (Focus::Inside, false) => {
                    "<b>\u{2190}\u{2192}</b> app   <b>\u{2191}\u{2193}</b> line   <b>Enter</b> launch   <b>Shift+Enter</b> launch line   <b>Tab</b>/<b>Esc</b> out"
                }
                (Focus::Outside, true) => {
                    "<b>\u{2190}\u{2192}</b> machine   <b>\u{2191}\u{2193}</b> rail   <b>Tab</b>/<b>Enter</b> browse   <b>drag</b> file/reorder   <b>Esc</b> close"
                }
                (Focus::Inside, true) => {
                    "<b>\u{2190}\u{2192}</b> title   <b>Enter</b> launch   <b>Tab</b>/<b>Esc</b> out"
                }
            };
            let hidden = s.hidden_count();
            if hidden == 0 {
                hint.set_markup(&format!(
                    "{base_hint}   <b>right-click</b> then click to hide"
                ));
            } else {
                hint.set_markup(&format!(
                    "{base_hint}   <b>right-click</b> then click to hide   <b>Ctrl+Shift+H</b> show all ({hidden})"
                ));
            }

            let now = Cursor {
                col: s.col,
                row: s.row,
                line: s.line,
                item: s.item,
                inside: s.focus == Focus::Inside,
            };
            let mut p = painted.borrow_mut();
            if p.last == Some(now) {
                return;
            }
            // Un-highlight exactly what was highlighted, then highlight the new place. Walking the
            // whole grid to clear stale classes would put the cost back that this exists to remove.
            if let Some(was) = p.last {
                p.mark(was, false);
            }
            p.mark(now, true);
            p.last = Some(now);

            // BRING THE CURSOR INTO VIEW. The grid is capped at a fraction of the display and
            // scrolls past that, but nothing was ever moving the viewport -- so arrowing down into
            // a clipped row moved the selection somewhere the user could not see, and the launcher
            // silently became a thing you navigate blind. A scrollbar reaches those rows; the
            // keyboard could not, which is the wrong way round for a keyboard-driven launcher.
            if let Some(cell) = p.cells.get(now.row).and_then(|r| r.get(now.col)) {
                // DEFERRED, because at this point the widget may have no allocation yet: a repaint
                // that follows a rebuild runs before layout, and `compute_bounds` on an
                // unallocated widget answers with nothing (or with zeroes, which would scroll to
                // the top and look like the viewport jumping on its own).
                // The OUTER scroller follows the whole rail cell; the title itself lives in
                // that cell's private horizontal viewport and is revealed separately below.
                // Letting its coordinates drive the outer scroller would pan the entire matrix
                // to follow content which no longer belongs to the matrix's width negotiation.
                let bx = if now.inside && cell.rail.is_none() {
                    cell.lines
                        .get(now.line)
                        .and_then(|line| line.apps.get(now.item))
                        .cloned()
                        .unwrap_or_else(|| cell.bx.clone())
                } else {
                    cell.bx.clone()
                };
                let rail_target = cell.rail.as_ref().and_then(|rail| {
                    cell.lines.get(now.line).and_then(|line| {
                        line.apps
                            .get(now.item)
                            .map(|app| (rail.clone(), line.bx.clone(), app.clone()))
                    })
                });
                let scroller = scroller.clone();
                let grid = grid_for_scroll.clone();
                gtk::glib::idle_add_local_once(move || {
                    let Some(b) = bx.compute_bounds(&grid) else {
                        return;
                    };
                    // ONE RULE, BOTH AXES. Moving left and right across machines runs off the
                    // edge exactly as moving down through folders runs off the bottom, and a fix
                    // that only followed the cursor vertically would be the same bug left half
                    // done. Nearest edge only: centring the selection would move the whole grid on
                    // every keypress, and a spatial launcher is fast precisely because things stay
                    // where they were learned -- scrolling only when the target is genuinely
                    // off-screen keeps everything else still.
                    let reveal = |adj: &gtk::Adjustment, near: f64, far: f64| {
                        let (view, page) = (adj.value(), adj.page_size());
                        if near < view {
                            adj.set_value(near);
                        } else if far > view + page {
                            adj.set_value(far - page);
                        }
                    };
                    reveal(
                        &scroller.vadjustment(),
                        b.y() as f64,
                        (b.y() + b.height()) as f64,
                    );
                    reveal(
                        &scroller.hadjustment(),
                        b.x() as f64,
                        (b.x() + b.width()) as f64,
                    );
                    if let Some((rail, line, app)) = rail_target
                        && let Some(b) = app.compute_bounds(&line)
                    {
                        reveal(
                            &rail.hadjustment(),
                            b.x() as f64,
                            (b.x() + b.width()) as f64,
                        );
                    }
                });
            }
        }
    });

    // The currently revealed inline action, if any. Only one is shown at a time so right-clicking
    // another application moves the affordance instead of leaving a trail of Hide markers.
    // Inline is important: a GtkPopover creates a second Wayland surface, which makes the layer
    // window report focus loss and correctly triggers this launcher's dismiss-on-blur policy.
    //
    // A MARKER, NOT A BUTTON, and that is the fix to a bug rather than a matter of taste. It was a
    // 20px GtkButton overlaid on the icon, inside an application box that already carries a drag
    // source and a click gesture of its own -- three handlers contending for one press on one small
    // square. The press was reliably eaten by one of the other two: the eye appeared, clicking it
    // did nothing at all, and nothing was launched either, so there was not even a wrong outcome to
    // notice. Making it inert and letting the ARMED application's own click gesture perform the
    // hide removes the contention instead of trying to win it -- the same gesture that has always
    // launched reliably, so the path is known good.
    let visible_hide_action = Rc::new(RefCell::new(None::<Image>));

    // WHICH application is armed, by identity rather than by widget: the grid is rebuilt whenever
    // the query changes or a drag lands, so a widget handle would outlive the thing it stood for.
    let armed_hide = Rc::new(RefCell::new(None::<(String, String)>));

    let render: Rc<dyn Fn()> = Rc::new({
        let state = state.clone();
        let layout = layout.clone();
        let grid = grid.clone();
        let spacer_folder = spacer_folder.clone();
        let spacer_sub = spacer_sub.clone();
        let holder = render_holder.clone();
        let theme_error = theme.error.clone();
        let painted = painted.clone();
        let paint = paint.clone();
        let icon_px = theme.icon_size;
        let icon_theme = icon_theme.clone();
        let icon_cache = icon_cache.clone();
        let visible_hide_action = visible_hide_action.clone();
        let armed_hide = armed_hide.clone();
        // For the click path: launching needs the window to dismiss, the terminal wrapper for
        // programs that draw none, and the state to record the launch against.
        let window = window.clone();
        let terminal_cmd = terminal_cmd_outer.clone();
        let dismiss = dismiss.clone();
        move || {
            let t_render = std::time::Instant::now();
            let s = state.borrow();

            // Every handle recorded below belongs to a widget that is about to be destroyed, so the
            // record is cleared FIRST. Leaving the old ones in place would have the next repaint
            // remove a class from a widget that is no longer in the tree -- harmless, and a silent
            // way for the real selection to keep a highlight it should have lost.
            painted.borrow_mut().reset();
            *visible_hide_action.borrow_mut() = None;
            // Disarmed with it. A rebuilt grid is a new set of widgets, and an application left
            // armed across one would hide on a click the user meant as a launch.
            *armed_hide.borrow_mut() = None;

            while let Some(c) = grid.first_child() {
                grid.remove(&c);
            }

            // Rebuilt every render rather than kept: the row-head widgets are recreated each time,
            // and a long-lived group would accumulate memberships for widgets that no longer exist.
            let labelcol = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
            labelcol.add_widget(&spacer_folder);
            // The subcategory column gets its own group, so the corner can be the sum of the two
            // rather than an approximation of either.
            let subcol = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
            subcol.add_widget(&spacer_sub);
            let machine_columns = layout
                .borrow()
                .equal_columns
                .then(|| gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal));

            for (c, m) in s.view.iter().enumerate() {
                let head = Label::new(None);
                head.set_xalign(0.0);
                head.add_css_class("colhead");
                // A machine that could not be asked says so IN ITS OWN HEADING, next to its name.
                // The alternative -- an empty column -- reads identically to a machine that simply
                // has nothing installed, which is the one thing it must not be confused with.
                head.set_markup(&match &m.error {
                    None => format!("<span foreground=\"{}\">{}</span>", m.accent, escape(&m.name)),
                    Some(e) => format!(
                        "<span foreground=\"{}\">{}</span>  <span foreground=\"{}\" size=\"small\">{}</span>",
                        m.accent,
                        escape(&m.name),
                        theme_error,
                        escape(e.lines().next().unwrap_or("unreachable"))
                    ),
                });
                if let Some(e) = &m.error {
                    head.set_tooltip_text(Some(e));
                }
                if let Some(group) = &machine_columns {
                    group.add_widget(&head);
                }
                grid.attach(&head, c as i32 + 2, 0, 1, 1);
            }

            let mut last_rendered_group: Option<String> = None;
            for (r, folder) in s.folders.iter().enumerate() {
                // A row nobody has anything on is a gap with a label. Skipped rather than removed,
                // so row indices stay aligned with the placement -- the alignment that has gone
                // wrong twice already.
                if !s.row_has_content(r) {
                    painted.borrow_mut().rowheads.push(Label::new(None));
                    painted.borrow_mut().cells.push(Vec::new());
                    continue;
                }
                // `folder/sub` splits across two label columns, and the folder name is drawn
                // only on the FIRST row that carries it -- repeating "Chat" down three rows would
                // be noise, and its absence is what makes the group read as a group.
                let (fname, sub) = match folder.split_once('/') {
                    Some((f, s)) => (f, Some(s)),
                    None => (folder.as_str(), None),
                };
                // Compare with the last row actually DRAWN. The preceding canonical row may have
                // been skipped as globally empty; comparing with it suppresses the only visible
                // folder label and leaves a block of subrows with no heading.
                let first_of_group = last_rendered_group.as_deref() != Some(fname);
                last_rendered_group = Some(fname.to_string());
                let rh = Label::new(if first_of_group { Some(fname) } else { None });
                rh.set_xalign(1.0);
                rh.set_valign(Align::Center);
                rh.add_css_class("rowhead");
                labelcol.add_widget(&rh);
                grid.attach(&rh, 0, r as i32 + 1, 1, 1);

                // The subcategory, in its own column OUTSIDE the machines, so it lines up across
                // every one of them instead of only with itself.
                let sublabel = Label::new(sub);
                sublabel.set_xalign(1.0);
                sublabel.set_valign(Align::Center);
                sublabel.add_css_class("subrow");
                subcol.add_widget(&sublabel);
                grid.attach(&sublabel, 1, r as i32 + 1, 1, 1);
                painted.borrow_mut().rowheads.push(rh.clone());
                let mut row_cells: Vec<CellW> = Vec::with_capacity(s.view.len());
                for (c, m) in s.view.iter().enumerate() {
                    let lines = &m.cells[r];
                    let items = lines.iter().map(|line| line.apps.len()).sum();
                    let rail = layout.borrow().is_rail(folder, items);
                    let cell = GBox::new(Orientation::Vertical, 2);
                    cell.add_css_class("cell");
                    if let Some(group) = &machine_columns {
                        group.add_widget(&cell);
                    }
                    let mut rail_viewport = None;
                    // Dropping onto a cell files the app into that folder, for that machine,
                    // permanently. Same column only -- see the drag payload's own note.
                    {
                        let tgt = gtk::DropTarget::new(
                            gtk::glib::Type::STRING,
                            gtk::gdk::DragAction::MOVE,
                        );
                        let st = state.clone();
                        let holder2 = holder.clone();
                        tgt.connect_drop(move |_, value, _, _| {
                            let Ok(p) = value.get::<String>() else {
                                return false;
                            };
                            let Some((from_col, name)) = p.split_once('\u{1}') else {
                                return false;
                            };
                            if from_col.parse::<usize>().ok() != Some(c) {
                                return false;
                            }
                            // Dropped on the cell's own background, not on a line: give it a
                            // line to itself. Joining an appset is what dropping ON a line means.
                            st.borrow_mut().place_app(c, name, r, None, None);
                            if let Some(rf) = holder2.borrow().as_ref() {
                                rf();
                            }
                            true
                        });
                        cell.add_controller(tgt);
                    }
                    // CLICKING A BOX MOVES THE KEYBOARD CURSOR INTO IT. Without this the mouse
                    // could launch and rearrange but never change where the keyboard was, so the
                    // two halves of the interface disagreed about where you are -- click a box,
                    // press an arrow, and the selection jumped back somewhere else entirely.
                    {
                        let st = state.clone();
                        let paint2 = paint.clone();
                        let pick = gtk::GestureClick::new();
                        pick.connect_released(move |_, _, _, _| {
                            {
                                let mut s = st.borrow_mut();
                                s.col = c;
                                s.row = r;
                                s.focus = if s.cell().is_empty() {
                                    Focus::Outside
                                } else {
                                    Focus::Inside
                                };
                                s.line = 0;
                                s.item = 0;
                                s.item_goal = 0;
                                s.clamp();
                            }
                            paint2();
                        });
                        cell.add_controller(pick);
                    }

                    let mut cell_lines: Vec<LineW> = Vec::with_capacity(lines.len());
                    if lines.is_empty() {
                        cell.add_css_class("empty");
                        let dash = Label::new(Some("\u{2014}"));
                        dash.set_xalign(0.0);
                        dash.add_css_class("dim");
                        cell.append(&dash);
                    }
                    for ln in lines.iter() {
                        let lb = GBox::new(Orientation::Horizontal, 2);
                        lb.add_css_class("line");
                        // Dropping ON a line inserts INTO it, at the gap nearest the pointer --
                        // which is simultaneously "join this appset", "put it third rather than
                        // first", and (when the app is already on this line) "reorder it".
                        {
                            let tgt = gtk::DropTarget::new(
                                gtk::glib::Type::STRING,
                                gtk::gdk::DragAction::MOVE,
                            );
                            let st = state.clone();
                            let holder2 = holder.clone();
                            let lb2 = lb.clone();
                            // WHAT is on this line, not WHICH line it is. A rendered line index is
                            // an index into a grid that is filtered and frecency-ordered -- two
                            // transformations away from the placement it would be written to. The
                            // names survive both; see `place_app`'s own account.
                            let names: Vec<String> = ln.apps.iter().map(|a| a.id.clone()).collect();
                            tgt.connect_drop(move |_, value, x, _| {
                                let Ok(payload) = value.get::<String>() else {
                                    return false;
                                };
                                let Some((from_col, name)) = payload.split_once('\u{1}') else {
                                    return false;
                                };
                                if from_col.parse::<usize>().ok() != Some(c) {
                                    return false;
                                }
                                // The gap the pointer is nearest, expressed as "goes before this
                                // app". Past the last gap there is no such app, and None is
                                // exactly right: it means the end of the line.
                                let at = insert_index_at(&lb2, x);
                                let before = names.get(at).cloned();
                                st.borrow_mut().place_app(
                                    c,
                                    name,
                                    r,
                                    Some(&names),
                                    before.as_deref(),
                                );
                                if let Some(rf) = holder2.borrow().as_ref() {
                                    rf();
                                }
                                true
                            });
                            lb.add_controller(tgt);
                        }
                        // The row's name, when it has one, at its head. Small and muted: it is a
                        // label for what follows, not an entry you can act on, and drawing it like
                        // the applications would invite clicking something that does nothing.
                        let mut line_apps: Vec<GBox> = Vec::with_capacity(ln.apps.len());
                        for app in ln.apps.iter() {
                            let b = GBox::new(Orientation::Horizontal, 4);
                            b.add_css_class("app");

                            let img = match icon_cache.borrow_mut().texture(&app.icon, &icon_theme)
                            {
                                Some(tex) => Image::from_paintable(Some(&tex)),
                                None => Image::from_icon_name(&app.icon),
                            };
                            img.set_pixel_size(icon_px);

                            // Overlay only the existing icon, never the whole application. The app
                            // box remains the exact same direct child of the line that it was before
                            // hiding existed, preserving its expand and natural-width negotiation.
                            // The action is explicitly excluded from measurement as a second guard:
                            // revealing it must consume exactly zero new layout space.
                            let icon_overlay = gtk::Overlay::new();
                            icon_overlay.set_child(Some(&img));
                            icon_overlay.set_hexpand(false);

                            // INERT. It states that this application is armed and takes no input of
                            // its own: `can_target(false)` puts it out of the picking pass
                            // entirely, so a press on the icon reaches the box's own gesture the
                            // way a press anywhere else on the application already does.
                            let hide_action = Image::from_icon_name("view-conceal-symbolic");
                            hide_action.add_css_class("hide-action");
                            hide_action.set_tooltip_text(Some(&format!("Hide {}", app.name)));
                            hide_action.set_pixel_size(16);
                            hide_action.set_can_target(false);
                            hide_action.set_size_request(icon_px, icon_px);
                            hide_action.set_halign(gtk::Align::Center);
                            hide_action.set_valign(gtk::Align::Center);
                            hide_action.set_visible(false);
                            icon_overlay.add_overlay(&hide_action);
                            icon_overlay.set_measure_overlay(&hide_action, false);
                            icon_overlay.set_clip_overlay(&hide_action, true);
                            // The payload carries the COLUMN it came from as well as the name, so
                            // the drop side can refuse a cross-machine drag without having to ask
                            // anyone: filing is per machine, and "Firefox on one machine" is not the
                            // same object as "Firefox on another".
                            {
                                let src = gtk::DragSource::new();
                                src.set_actions(gtk::gdk::DragAction::MOVE);
                                let payload = format!("{}\u{1}{}", c, app.id);
                                src.connect_prepare(move |_, _, _| {
                                    Some(gtk::gdk::ContentProvider::for_value(&payload.to_value()))
                                });
                                b.add_controller(src);
                            }
                            // PRIMARY launches. Middle launches and leaves the launcher open.
                            // Secondary reveals a small inline action: hiding must be a visible,
                            // deliberate command rather than an irreversible-looking surprise.
                            {
                                let st = state.clone();
                                let win = window.clone();
                                let term = terminal_cmd.clone();
                                let dismiss = dismiss.clone();
                                let holder2 = holder.clone();
                                // Captured by IDENTITY, never by index: the grid may be rebuilt
                                // between this being wired and the click arriving -- a query
                                // filters, a drag reorders, frecency moves a line -- and an index
                                // would by then name a different application.
                                let id = app.id.clone();

                                let hide_action_for_click = hide_action.clone();
                                let visible = visible_hide_action.clone();
                                let armed = armed_hide.clone();
                                let machine_name = m.name.clone();
                                let click = gtk::GestureClick::new();
                                // Every button, so middle and right arrive here too rather than
                                // only the primary one.
                                click.set_button(0);
                                click.connect_released(move |g, _, _, _| {
                                    let button = g.current_button();
                                    // Claimed, so the drag source on this same widget does not
                                    // also read the press as the beginning of a drag.
                                    g.set_state(gtk::EventSequenceState::Claimed);

                                    if button == 3 {
                                        if let Some(previous) = visible
                                            .borrow_mut()
                                            .replace(hide_action_for_click.clone())
                                        {
                                            previous.set_visible(false);
                                        }
                                        hide_action_for_click.set_visible(true);
                                        *armed.borrow_mut() =
                                            Some((machine_name.clone(), id.clone()));
                                        return;
                                    }
                                    if button != 1 && button != 2 {
                                        return;
                                    }

                                    // ARMED MEANS HIDE, and only for the application that was
                                    // armed: a primary click anywhere else disarms and does what it
                                    // always did, so changing your mind costs one ordinary click
                                    // rather than a gesture nobody would guess.
                                    let armed_here = armed
                                        .borrow()
                                        .as_ref()
                                        .is_some_and(|(am, ai)| *am == machine_name && *ai == id);
                                    if armed_here && button == 1 {
                                        *armed.borrow_mut() = None;
                                        *visible.borrow_mut() = None;
                                        hide_action_for_click.set_visible(false);
                                        let mut state = st.borrow_mut();
                                        let changed = state.hide_app(&machine_name, &id);
                                        state.clamp();
                                        drop(state);
                                        if changed && let Some(rf) = holder2.borrow().as_ref() {
                                            rf();
                                        }
                                        return;
                                    }
                                    // Clicking a DIFFERENT application disarms the one that was
                                    // armed, so the marker is taken from whichever widget is
                                    // showing it rather than from this one.
                                    if armed.borrow_mut().take().is_some()
                                        && let Some(previous) = visible.borrow_mut().take()
                                    {
                                        previous.set_visible(false);
                                    }

                                    let mut st_mut = st.borrow_mut();
                                    let Some(machine) = st_mut.view.get(c).cloned() else {
                                        return;
                                    };
                                    let found = machine
                                        .cells
                                        .iter()
                                        .flatten()
                                        .flat_map(|l| l.apps.iter())
                                        .find(|a| a.id == id)
                                        .cloned();
                                    let Some(app) = found else { return };

                                    // The keyboard owns appset launching through Shift+Enter;
                                    // pointer buttons act on exactly the item under the pointer.
                                    let batch = vec![app.clone()];
                                    let mut launched = false;
                                    for a in &batch {
                                        if spawn(&machine, a, &term) {
                                            st_mut.record_launch(&machine.name, &a.id);
                                            launched = true;
                                        }
                                    }
                                    if launched {
                                        st_mut.save_usage();
                                    }
                                    if button == 2 {
                                        // Stay open, and repaint so the frecency reorder this
                                        // launch may have earned is visible immediately.
                                        if launched {
                                            st_mut.rebuild();
                                        }
                                        drop(st_mut);
                                        if launched && let Some(rf) = holder2.borrow().as_ref() {
                                            rf();
                                        }
                                        return;
                                    }
                                    // The borrow ends before the window is touched: dismissing
                                    // releases idle pages and can re-enter, and a live borrow
                                    // here would panic at runtime rather than fail to compile.
                                    drop(st_mut);
                                    if launched {
                                        dismiss(&win);
                                    }
                                });
                                b.add_controller(click);
                            }
                            b.append(&icon_overlay);
                            let l = Label::new(Some(&app.name));
                            l.add_css_class("appname");
                            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                            l.set_max_width_chars(layout.borrow().max_label_chars);
                            l.set_tooltip_text(Some(&app.name));
                            b.append(&l);
                            lb.append(&b);
                            line_apps.push(b);
                        }
                        if rail {
                            // A rail is deliberately one long vector, but that does not make
                            // it the width authority for every row in this machine column. Give
                            // the vector a local viewport whose natural width does not propagate;
                            // ordinary appset rows now decide the compact column width, and only
                            // the rail pans when a title lies beyond it.
                            let viewport = gtk::ScrolledWindow::new();
                            viewport.add_css_class("vector-rail");
                            viewport.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
                            viewport.set_overlay_scrolling(true);
                            viewport.set_propagate_natural_width(false);
                            viewport.set_propagate_natural_height(true);
                            viewport.set_hexpand(true);
                            viewport.set_child(Some(&lb));
                            cell.append(&viewport);
                            rail_viewport = Some(viewport);
                        } else {
                            cell.append(&lb);
                        }
                        cell_lines.push(LineW {
                            bx: lb.clone(),
                            apps: line_apps,
                        });
                    }
                    grid.attach(&cell, c as i32 + 2, r as i32 + 1, 1, 1);
                    row_cells.push(CellW {
                        bx: cell.clone(),
                        lines: cell_lines,
                        rail: rail_viewport,
                    });
                }
                painted.borrow_mut().cells.push(row_cells);
            }

            // The selection classes are NOT set above. Painting them is the other function's job,
            // and doing it here as well would be a second implementation of the same rule, free to
            // disagree with the first the moment either changes.
            let drawn: usize = s
                .view
                .iter()
                .flat_map(|m| m.cells.iter().flatten())
                .map(|l| l.apps.len())
                .sum();
            drop(s);
            paint();
            trace(format_args!(
                "render us={} apps={drawn}",
                t_render.elapsed().as_micros()
            ));
        }
    });
    *render_holder.borrow_mut() = Some(render.clone());

    // WHAT REOPENING MEANS FOR A RESIDENT PROCESS.
    //
    // A process that never exits never re-reads anything, so without this a launcher left running
    // for a week would still be showing both the CONFIGURATION and the applications that existed
    // when it started. Reopening therefore re-reads config.json and re-asks every machine what it
    // has. Keeping the build-time Config here instead would make a folder rename or a new machine
    // invisible until the daemon was restarted -- exactly the lifecycle a resident process exists
    // to avoid paying.
    //
    // Cheap, because the expensive parts are already done: the inventory commands answer from
    // their own cache in tens of milliseconds, and any icon they name has been decoded since the
    // first launch. What this does NOT redo is exec, dynamic linking, GTK initialisation or the
    // stylesheet -- the whole of the 117ms a cold start pays.
    //
    // The query is cleared too. A search left over from last time is not a state anyone expects to
    // reopen into, and it would hide most of the grid while looking like an empty launcher.
    {
        let state = state.clone();
        let render = render.clone();
        let arm_focus = arm_focus.clone();
        let layout = layout.clone();
        let prefer_output = prefer_output.clone();
        // LAST KNOWN GOOD. Home Manager replaces the file atomically, but direct editors need not;
        // a parse failure during a save must not replace the coherent grid with fixtures or empty
        // columns. The next reveal tries again. This also lets a daemon started before config.json
        // existed begin using it once the file appears.
        let refresh_config = Rc::new(RefCell::new(loaded_config.clone()));
        let refresh_generation = Rc::new(std::cell::Cell::new(0u64));
        // Shared with the worker thread rather than owned by it: the worker is created fresh on
        // every reveal, and recognising an unchanged answer is precisely a memory ACROSS reveals.
        let inventories: std::sync::Arc<std::sync::Mutex<Option<Inventories>>> =
            std::sync::Arc::new(std::sync::Mutex::new(None));
        let reveal: Rc<dyn Fn()> = Rc::new(move || {
            let t_reveal = std::time::Instant::now();
            arm_focus();
            // Reveal the coherent cached grid immediately. Inventory refresh is external I/O and
            // belongs off the GTK thread; one wedged peer must not stop the existing window from
            // mapping or make focus-loss handling unresponsive.
            {
                let mut s = state.borrow_mut();
                s.query.clear();
                s.line = 0;
                s.item = 0;
                s.item_goal = 0;
                s.rebuild();
                s.clamp();
            }
            render();

            // Reading one small local JSON file is bounded work and belongs here on the GTK thread;
            // inventory remains in the worker below. Only model inputs reload live: folders,
            // subrows, machines, launch prefixes and vector layout. CSS, key bindings, the terminal
            // wrapper and layer-shell/window construction were bound above and deliberately remain
            // restart-only rather than being half-reconfigured under a mapped window.
            let latest_config = match config::load() {
                Ok(Some(cfg)) => {
                    *refresh_config.borrow_mut() = Some(cfg.clone());
                    Some(cfg)
                }
                Ok(None) => refresh_config.borrow().clone(),
                Err(e) => {
                    eprintln!("nixlaunch: {e}; retaining last valid configuration");
                    refresh_config.borrow().clone()
                }
            };
            let Some(cfg) = latest_config else {
                return;
            };
            // BEFORE THE `present` that follows this closure returns to. A layer surface's output
            // is decided at map time, so an output preference applied afterwards would not take
            // effect until the reveal after the one that read it.
            prefer_output(&cfg.outputs);
            let generation = refresh_generation.get().wrapping_add(1);
            refresh_generation.set(generation);
            let latest = refresh_generation.clone();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let seen = inventories.clone();
            std::thread::spawn(move || {
                let rows = cfg.folder_rows();
                let layout = cfg.layout.clone();
                let printed = inventory_bytes_all(&cfg.machines);
                // NOTHING MOVED, SO THERE IS NOTHING TO DO. The rows and the layout are compared
                // too because either can change without a machine saying anything different, and
                // both decide which row an application lands in.
                let unchanged = {
                    let seen = seen.lock().unwrap_or_else(|e| e.into_inner());
                    seen.as_ref().is_some_and(|last| {
                        last.printed == printed && last.rows == rows && last.layout == layout
                    })
                };
                if unchanged {
                    let _ = tx.send(None);
                    return;
                }
                let fresh = machines_from(&cfg.machines, &printed, &rows, &cfg.subrows);
                *seen.lock().unwrap_or_else(|e| e.into_inner()) = Some(Inventories {
                    printed,
                    rows: rows.clone(),
                    layout: layout.clone(),
                });
                let _ = tx.send(Some((rows, layout, fresh)));
            });
            let state = state.clone();
            let layout_state = layout.clone();
            let render = render.clone();
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(10), move || {
                match rx.try_recv() {
                    Ok(None) => {
                        trace(format_args!(
                            "inventory ms={} changed=false skipped=true",
                            t_reveal.elapsed().as_millis()
                        ));
                        gtk::glib::ControlFlow::Break
                    }
                    Ok(Some((rows, new_layout, fresh))) => {
                        if latest.get() == generation {
                            let mut s = state.borrow_mut();
                            let changed = s.replace_inventory(rows, new_layout.clone(), fresh);
                            drop(s);
                            trace(format_args!(
                                "inventory ms={} changed={changed} skipped=false",
                                t_reveal.elapsed().as_millis()
                            ));
                            if changed {
                                *layout_state.borrow_mut() = new_layout;
                                render();
                            }
                        }
                        gtk::glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => gtk::glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        gtk::glib::ControlFlow::Break
                    }
                }
            });
        });
        REVEAL.with(|r| *r.borrow_mut() = Some(reveal));
    }

    render();

    let keys = EventControllerKey::new();
    {
        let state = state.clone();
        let window = window.clone();
        let render = render.clone();
        let paint = paint.clone();
        let terminal_cmd = terminal_cmd_outer.clone();
        let keys_map = keys_map.clone();
        let layout = layout.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let shift = mods.contains(ModifierType::SHIFT_MASK);
            // WHAT the key means comes from the keymap, not from this match. GTK reports the
            // physical key; `keymap::Action` says what the user asked for, and anything unbound
            // falls through to the text arm -- which is why typing a name never needs a binding.
            //
            // Shift+Tab is asked for as `shift+tab` even though X11 hands it over as a different
            // keysym entirely (ISO_Left_Tab). Normalising here means a configuration file can say
            // the obvious thing.
            let name = match key {
                Key::ISO_Left_Tab => "tab".to_string(),
                k => k.name().map(|n| n.to_string()).unwrap_or_default(),
            };
            let chord = keymap::Keymap::chord(
                &name,
                mods.contains(ModifierType::CONTROL_MASK),
                mods.contains(ModifierType::ALT_MASK),
                shift,
                mods.contains(ModifierType::SUPER_MASK),
            );
            let act = keys_map.action(&chord);
            let structural;
            {
                let mut s = state.borrow_mut();
                // THE test for "did the grid's contents change", and it is exact rather than a
                // guess: every arm that calls `refilter` does so because the query moved, and no
                // other arm touches it. Comparing the query afterwards therefore identifies the
                // structural keys without each arm having to remember to declare itself -- a flag
                // set by hand would be one `s.query.push` away from being wrong, and the symptom
                // would be a grid that silently stops matching what was typed.
                let before = s.query.clone();
                let mut state_rebuilt = false;
                let rail = s.folders.get(s.row).is_some_and(|row| {
                    layout
                        .borrow()
                        .is_rail(row, s.cell().iter().map(|line| line.apps.len()).sum())
                });
                match (s.focus, act) {
                    // Esc unwinds one layer at a time rather than always closing: a typed query is
                    // state the user can lose accidentally, so it gets its own step.
                    (_, Some(keymap::Action::Cancel)) => {
                        if !s.query.is_empty() {
                            s.set_query(String::new());
                        } else if s.focus == Focus::Inside {
                            s.focus = Focus::Outside;
                        } else {
                            dismiss(&window);
                            return gtk::glib::Propagation::Stop;
                        }
                    }
                    (_, Some(keymap::Action::GoOutside)) => {
                        // Shift+Tab arrives as a DIFFERENT keysym, so the plain Tab arm never saw
                        // it and the binding was simply dead.
                        s.focus = Focus::Outside;
                    }
                    (_, Some(keymap::Action::ToggleInside)) => {
                        s.focus = if s.focus == Focus::Outside && !s.cell().is_empty() {
                            Focus::Inside
                        } else {
                            Focus::Outside
                        };
                        s.line = 0;
                        s.item = 0;
                        s.item_goal = 0;
                    }

                    (Focus::Outside, Some(keymap::Action::MoveLeft)) => {
                        s.col = s.col.saturating_sub(1)
                    }
                    (Focus::Outside, Some(keymap::Action::MoveRight)) => {
                        s.col = (s.col + 1).min(s.view.len().saturating_sub(1))
                    }
                    (Focus::Outside, Some(keymap::Action::MoveUp)) => s.row = s.next_row(s.row, -1),
                    (Focus::Outside, Some(keymap::Action::MoveDown)) => {
                        s.row = s.next_row(s.row, 1)
                    }
                    (Focus::Outside, Some(keymap::Action::Enter)) => {
                        if !s.cell().is_empty() {
                            s.focus = Focus::Inside;
                            s.line = 0;
                            s.item = 0;
                            s.item_goal = 0;
                        }
                    }
                    (
                        Focus::Outside,
                        Some(
                            action @ (keymap::Action::LaunchLine
                            | keymap::Action::LaunchCell
                            | keymap::Action::LaunchSelection),
                        ),
                    ) => {
                        // A long rail has no safe whole-cell action. The same chord that launches
                        // an inline vector enters the rail, where every launch action
                        // is deliberately reduced to the selected title.
                        if rail {
                            if !s.cell().is_empty() {
                                s.focus = Focus::Inside;
                                s.line = 0;
                                s.item = 0;
                                s.item_goal = 0;
                            }
                            s.clamp();
                            drop(s);
                            paint();
                            return gtk::glib::Propagation::Stop;
                        }
                        let Some(machine) = s.view.get(s.col).cloned() else {
                            return gtk::glib::Propagation::Stop;
                        };
                        let apps: Vec<App> = match action {
                            keymap::Action::LaunchLine => {
                                s.current_line().map(|l| l.apps.clone()).unwrap_or_default()
                            }
                            _ => s
                                .cell()
                                .iter()
                                .flat_map(|l| l.apps.iter().cloned())
                                .collect(),
                        };
                        let mut launched = false;
                        for app in &apps {
                            if spawn(&machine, app, &terminal_cmd) {
                                s.record_launch(&machine.name, &app.id);
                                launched = true;
                            }
                        }
                        if launched {
                            s.save_usage();
                            dismiss(&window);
                            return gtk::glib::Propagation::Stop;
                        }
                    }

                    // Left/right are the only keys that CHOOSE a column, so they are the only ones
                    // that move the goal. Up/down just change line and let `clamp` re-aim.
                    (Focus::Inside, Some(keymap::Action::MoveLeft)) => {
                        s.item = s.item.saturating_sub(1);
                        s.item_goal = s.item;
                    }
                    (Focus::Inside, Some(keymap::Action::MoveRight)) => {
                        let n = s.current_line().map(|l| l.apps.len()).unwrap_or(0);
                        s.item = (s.item + 1).min(n.saturating_sub(1));
                        s.item_goal = s.item;
                    }
                    (Focus::Inside, Some(keymap::Action::MoveUp)) => {
                        s.line = s.line.saturating_sub(1)
                    }
                    (Focus::Inside, Some(keymap::Action::MoveDown)) => {
                        s.line = (s.line + 1).min(s.cell().len().saturating_sub(1))
                    }
                    (
                        Focus::Inside,
                        Some(
                            action @ (keymap::Action::Enter
                            | keymap::Action::LaunchLine
                            | keymap::Action::LaunchCell
                            | keymap::Action::LaunchSelection),
                        ),
                    ) => {
                        let Some(machine) = s.view.get(s.col).cloned() else {
                            return gtk::glib::Propagation::Stop;
                        };
                        let apps: Vec<App> = if rail {
                            s.current_line()
                                .and_then(|l| l.apps.get(s.item))
                                .cloned()
                                .into_iter()
                                .collect()
                        } else {
                            match action {
                                keymap::Action::Enter => s
                                    .current_line()
                                    .and_then(|l| l.apps.get(s.item))
                                    .cloned()
                                    .into_iter()
                                    .collect(),
                                keymap::Action::LaunchCell => s
                                    .cell()
                                    .iter()
                                    .flat_map(|l| l.apps.iter().cloned())
                                    .collect(),
                                _ => s.current_line().map(|l| l.apps.clone()).unwrap_or_default(),
                            }
                        };
                        let mut launched = false;
                        for app in &apps {
                            if spawn(&machine, app, &terminal_cmd) {
                                s.record_launch(&machine.name, &app.id);
                                launched = true;
                            }
                        }
                        if launched {
                            s.save_usage();
                            // A launcher that stays up after launching is a window you then have
                            // to dismiss. Closing IS the confirmation.
                            dismiss(&window);
                            return gtk::glib::Propagation::Stop;
                        }
                    }

                    (_, Some(keymap::Action::Backspace)) => {
                        let mut q = s.query.clone();
                        q.pop();
                        s.set_query(q);
                    }
                    (_, Some(keymap::Action::ResetVisibility)) => {
                        state_rebuilt = s.reset_visibility();
                    }
                    _ => {
                        // A chord is a command, not text. Without this, Ctrl-W and Alt-F typed a
                        // literal "w" and "f" into the search box.
                        let chord = mods.contains(ModifierType::CONTROL_MASK)
                            || mods.contains(ModifierType::ALT_MASK)
                            || mods.contains(ModifierType::SUPER_MASK);
                        if !chord
                            && let Some(ch) = key.to_unicode()
                            && !ch.is_control()
                        {
                            let mut q = s.query.clone();
                            q.push(ch);
                            s.set_query(q);
                        }
                    }
                }
                s.clamp();
                structural = state_rebuilt || s.query != before;
            }
            if structural {
                render();
            } else {
                paint();
            }
            gtk::glib::Propagation::Stop
        });
    }
    window.add_controller(keys);

    // `--daemon` builds everything and shows nothing, so the later `activate` is a present() and
    // not a start.
    //
    // NO `hold()` HERE, and the story is worth keeping because it nearly became a permanent piece
    // of cargo cult. The daemon exited three seconds after starting, which looked exactly like a
    // lifetime problem, so a `hold()` went in with a confident comment about GtkApplication use
    // counts. The real cause was elsewhere -- GApplication rejecting an argv flag it did not know
    // -- and once that was fixed the daemon ran fine on two machines for hours.
    //
    // Then the compiler pointed out that `hold()` returns a guard which releases when dropped, so
    // the call had been doing nothing the entire time. Which is the proof: a hidden window keeps
    // the application alive on its own, exactly as the documentation says and as the first
    // reasoning had it. The "fix" was wrong AND inert, and only ever looked necessary because it
    // happened to be added at the same moment as the change that actually worked.
    if !start_hidden() {
        arm_focus();
        // The daemon reaches this through `reveal` instead; a one-shot start has no reveal to be
        // carried by, so it applies the preference for itself before its only map.
        prefer_output(
            &loaded_config
                .as_ref()
                .map(|c| c.outputs.clone())
                .unwrap_or_default(),
        );
        window.present();
    }
}

/// Config if there is one, fixture data if there is not.
///
/// A MISSING config is not an error -- a bare checkout must still run, which is what keeps this
/// repo testable by someone with no machines. A config that EXISTS but does not parse is a different
/// thing entirely and is reported rather than silently replaced by fixtures, because silently
/// showing invented machines when the real ones failed to load is the worst of both.
fn load_world() -> World {
    let default_rows = || {
        DEFAULT_FOLDERS
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
    };
    match config::load() {
        Err(e) => World {
            folders: default_rows(),
            machines: fixture(),
            theme: config::Theme::default(),
            layout: config::Layout::default(),
            terminal: vec![],
            surface: "layer".into(),
            keyboard: "exclusive".into(),
            exit_on_focus_loss: true,
            config: None,
            error: Some(e),
        },
        Ok(None) => World {
            folders: default_rows(),
            machines: fixture(),
            theme: config::Theme::default(),
            layout: config::Layout::default(),
            terminal: vec![],
            surface: "layer".into(),
            keyboard: "exclusive".into(),
            exit_on_focus_loss: true,
            config: None,
            error: None,
        },
        Ok(Some(cfg)) => {
            let rows = cfg.folder_rows();
            let machines = inventory_all(&cfg.machines, &rows, &cfg.subrows);
            World {
                folders: rows,
                machines,
                theme: cfg.theme.clone(),
                layout: cfg.layout.clone(),
                terminal: cfg.terminal.clone(),
                surface: cfg.surface.clone(),
                keyboard: cfg.keyboard.clone(),
                exit_on_focus_loss: cfg.exit_on_focus_loss,
                config: Some(cfg),
                error: None,
            }
        }
    }
}

/// Ask EVERY machine what it has, all at once.
///
/// One thread per machine, because the answer for a remote one is an SSH round trip and asking
/// them in turn makes the launcher wait for the sum of them. Measured on a three-machine config
/// with a cold inventory cache: 66ms + 269ms + 324ms sequentially, where the two remote ones are
/// each a fresh SSH connection. Concurrently that is bounded by the slowest, not the total, and
/// the launcher opens a third of a second sooner.
///
/// This is the ONLY place in the program where concurrency buys anything. Everything downstream of
/// here is GTK, which is single-threaded by construction -- so nothing else is a candidate, and the
/// rest of the startup cost has to come out of doing less work rather than doing it in more places.
///
/// Scoped threads specifically: they can borrow `rows` and the configs directly, so nothing has to
/// be cloned into each thread, and the scope cannot outlive the data by construction. The results
/// are joined IN ORDER, so the column order the user declared survives -- which matters, because
/// the first column is the one the launcher opens on.
///
/// A panicking thread yields that machine's column as unreachable rather than taking the process
/// with it. One machine's inventory command is not a reason for the other two to be unavailable,
/// and an inventory command is arbitrary user-supplied argv.
/// Ask every machine at once and keep the answers unparsed.
///
/// Concurrency buys exactly one thing here, and this is it: asking is the only part that waits on
/// something outside this process. Everything after it is arithmetic.
fn inventory_bytes_all(machines: &[config::MachineConfig]) -> Vec<Result<Vec<u8>, String>> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = machines
            .iter()
            .map(|mc| scope.spawn(move || inventory_bytes(mc)))
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("inventory panicked".into()))
            })
            .collect()
    })
}

/// The grid those answers describe. Pure, so it can be skipped when the answers have not moved.
fn machines_from(
    machines: &[config::MachineConfig],
    printed: &[Result<Vec<u8>, String>],
    rows: &[String],
    subrows: &std::collections::HashMap<String, Vec<config::SubRow>>,
) -> Vec<Machine> {
    machines
        .iter()
        .zip(printed)
        .map(|(mc, raw)| machine_from(mc, raw, rows, subrows))
        .collect()
}

fn inventory_all(
    machines: &[config::MachineConfig],
    rows: &[String],
    subrows: &std::collections::HashMap<String, Vec<config::SubRow>>,
) -> Vec<Machine> {
    machines_from(machines, &inventory_bytes_all(machines), rows, subrows)
}

/// Ask ONE machine what it has, by running the command config named for it.
///
/// Everything this program knows about detection is in these few lines: run argv, read JSON. No
/// SSH, no .desktop parsing, no package managers -- see config.rs on why that boundary is the
/// point rather than a simplification.
/// WHAT THE MACHINE PRINTED, unparsed -- or why it could not be asked.
///
/// Split from building the grid because the BYTES are the identity of the answer. A resident
/// launcher re-asks every machine on every open, and the overwhelmingly common outcome is that
/// nothing has changed since the last open: measured on a real three-machine inventory, the refresh
/// reported no change on every single reveal. Comparing the raw output first lets that case cost a
/// spawn, a read and a memcmp, instead of parsing two hundred applications, regrouping them into
/// rows and subrows, and then deep-comparing the result to discover it was identical.
fn inventory_bytes(mc: &config::MachineConfig) -> Result<Vec<u8>, String> {
    mc.inventory
        .split_first()
        .ok_or_else(|| "no inventory command configured".to_string())
        .and_then(|(bin, args)| {
            command_output(
                bin,
                args,
                std::time::Duration::from_millis(mc.inventory_timeout_ms),
            )
        })
        .and_then(|out| {
            if out.status.success() {
                Ok(out.stdout)
            } else {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                Err(if stderr.is_empty() {
                    format!("inventory exited with {}", out.status)
                } else {
                    stderr
                })
            }
        })
}

/// Turn one machine's printed answer into its column. Pure: no processes, no clock, no I/O.
fn machine_from(
    mc: &config::MachineConfig,
    printed: &Result<Vec<u8>, String>,
    rows: &[String],
    subrows: &std::collections::HashMap<String, Vec<config::SubRow>>,
) -> Machine {
    let mut cells: Vec<Vec<Line>> = vec![Vec::new(); rows.len()];
    // Declared without a value: both arms of the match below assign it, so an initial `None`
    // would be a value nothing ever reads -- which is exactly what the compiler was saying.
    let error;

    let parsed = match printed {
        Ok(bytes) => config::parse_inventory(bytes),
        Err(e) => Err(e.clone()),
    };

    match parsed {
        Ok(inv) => {
            // The machine reporting its own failure outranks anything inferred here.
            error = inv.error;
            for folder in inv.folders {
                // A label the config does not list falls into the inbox rather than being
                // dropped: an app nobody categorised must still be reachable.
                // WHICH ROW, and a declared subcategory beats the catch-all.
                //
                // The category table said which BOX this belongs in; the subcategory says which
                // rung inside it. Matching happens here, once, against the operator's own list,
                // rather than each application having to be dragged into place -- two hundred of
                // them is not a drag-and-drop job.
                let declared = subrows.get(&folder.label);
                let row_label = |a: &config::InventoryApp| -> String {
                    let id = a.id.clone().unwrap_or_default().to_lowercase();
                    let name = a.name.to_lowercase();
                    declared
                        .into_iter()
                        .flatten()
                        .find(|sr| {
                            sr.apps.iter().any(|want| {
                                let w = want.to_lowercase();
                                !w.is_empty() && (id.contains(&w) || name.contains(&w))
                            })
                        })
                        .map(|sr| format!("{}/{}", folder.label, sr.name))
                        .unwrap_or_else(|| folder.label.clone())
                };
                // Grouped by the row each app lands in, so a subcategory's members end up on its
                // row together rather than scattered by the order the inventory happened to list
                // them in.
                let mut by_row: Vec<Vec<&config::InventoryApp>> = vec![Vec::new(); rows.len()];
                for a in &folder.apps {
                    let label = row_label(a);
                    let r = rows
                        .iter()
                        .position(|x| *x == label)
                        .unwrap_or(rows.len().saturating_sub(1));
                    if let Some(bucket) = by_row.get_mut(r) {
                        bucket.push(a);
                    }
                }
                for (r, apps) in by_row
                    .into_iter()
                    .enumerate()
                    .filter(|(_, apps)| !apps.is_empty())
                {
                    cells[r].push(Line {
                        name: None,
                        apps: apps
                            .iter()
                            .map(|a| App {
                                id: a.id.clone().unwrap_or_else(|| a.name.clone()),
                                name: a.name.clone(),
                                icon: a.icon.clone(),
                                exec: a.exec.clone(),
                                terminal: a.terminal,
                            })
                            .collect(),
                    });
                }
            }
        }
        Err(e) => error = Some(e),
    }

    // The declared sub-rows, appended empty. They are drawn so they can be dragged into; an app
    // that has actually been filed into one arrives through the placement instead, which runs
    // after this and matches them by name.
    Machine {
        name: mc.name.clone(),
        aliases: mc.aliases.clone(),
        accent: mc.accent.clone(),
        launch: mc.launch.clone(),
        error,
        cells,
    }
}

/// Run one inventory command with a wall-clock bound. A process is put in its own process group so
/// a timeout can terminate the command and the helpers it started rather than leaving an SSH child
/// behind to keep the captured stdout/stderr pipes open forever.
fn command_output(
    bin: &str,
    args: &[String],
    timeout: std::time::Duration,
) -> Result<std::process::Output, String> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new(bin)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("{bin}: {e}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{bin}: stdout pipe was not created"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| format!("{bin}: stderr pipe was not created"))?;
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        // Nonblocking reads let this thread drain BOTH pipes while it also watches the deadline.
        // A reader thread per pipe looks simpler, but cannot be stopped if a grandchild inherits a
        // write end and keeps it open; one reveal would then leak two threads indefinitely.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            stop_child_group(&mut child);
            return Err(format!(
                "{bin}: could not make inventory pipes nonblocking: {}",
                std::io::Error::last_os_error()
            ));
        }
    }

    let started = std::time::Instant::now();
    let mut status = None;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut collect_until = None;
    loop {
        if status.is_none() {
            match child.try_wait() {
                Ok(Some(exit)) => {
                    status = Some(exit);
                    // Usually EOF is already visible. Give readers at least a short grace when the
                    // process exits on its deadline, while never waiting beyond the configured
                    // bound merely because a descendant inherited a pipe.
                    let now = std::time::Instant::now();
                    let grace = now
                        .checked_add(std::time::Duration::from_millis(100))
                        .unwrap_or(now);
                    collect_until = Some(
                        started
                            .checked_add(timeout)
                            .map(|deadline| deadline.max(grace))
                            .unwrap_or(grace),
                    );
                }
                Ok(None) => {}
                Err(e) => {
                    stop_child_group(&mut child);
                    return Err(format!("{bin}: {e}"));
                }
            }
        }

        let drained = (|| -> Result<(), String> {
            if !stdout_done {
                stdout_done = drain_capture(&mut stdout, &mut stdout_bytes, "stdout")?;
            }
            if !stderr_done {
                stderr_done = drain_capture(&mut stderr, &mut stderr_bytes, "stderr")?;
            }
            Ok(())
        })();
        if let Err(e) = drained {
            if status.is_none() {
                stop_child_group(&mut child);
            }
            return Err(format!("{bin}: {e}"));
        }

        if let Some(exit) = status {
            if stdout_done && stderr_done {
                return Ok(std::process::Output {
                    status: exit,
                    stdout: stdout_bytes,
                    stderr: stderr_bytes,
                });
            }
            if collect_until.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                return Err(format!(
                    "{bin}: inventory output remained open after the command exited"
                ));
            }
        } else if started.elapsed() >= timeout {
            // Negative pid means the process group created above. The leader has NOT been reaped
            // in this branch, so its pgid cannot have been recycled under us. Once try_wait returns
            // a status we deliberately never send to that number again.
            stop_child_group(&mut child);
            return Err(format!(
                "inventory timed out after {} ms",
                timeout.as_millis()
            ));
        }

        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Drain one nonblocking pipe without giving an arbitrary inventory command unbounded memory.
/// Sixteen MiB is orders of magnitude above a normal inventory, while still making a runaway
/// producer a column error rather than a launcher-wide OOM.
fn drain_capture(
    pipe: &mut impl std::io::Read,
    bytes: &mut Vec<u8>,
    label: &str,
) -> Result<bool, String> {
    const MAX_BYTES: usize = 16 * 1024 * 1024;
    let mut buf = [0u8; 16 * 1024];
    loop {
        let remaining = (MAX_BYTES + 1).saturating_sub(bytes.len());
        if remaining == 0 {
            return Err(format!("inventory {label} exceeded {MAX_BYTES} bytes"));
        }
        let take = remaining.min(buf.len());
        match pipe.read(&mut buf[..take]) {
            Ok(0) => return Ok(true),
            Ok(n) => {
                bytes.extend_from_slice(&buf[..n]);
                if bytes.len() > MAX_BYTES {
                    return Err(format!("inventory {label} exceeded {MAX_BYTES} bytes"));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(format!("could not read inventory {label}: {e}")),
        }
    }
}

/// Kill and reap a child which is still known to lead the process group created for it.
fn stop_child_group(child: &mut std::process::Child) {
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.wait();
}

/// Start one application, isolated from the launcher's process group and unit cgroup where the
/// host can provide that. Returns whether the immediate launcher process was created; it cannot
/// promise that a forwarding wrapper later succeeded, but known refusals must not earn usage.
///
/// A NEW PROCESS GROUP ON PURPOSE. `process_group(0)` is `setpgid`, not `setsid`: a Ctrl-C aimed at
/// the launcher's foreground process group does not reach the child. It neither creates a session
/// nor drops the controlling terminal; escaping a systemd unit's cgroup is the scope below's job.
///
/// Failures are reported, not swallowed: a missing binary is exactly the case where silence would
/// look like the keypress never registered.
fn spawn(machine: &Machine, app: &App, terminal: &[String]) -> bool {
    if app.terminal && terminal.is_empty() {
        // Say so rather than launching something that will vanish: a terminal program spawned
        // without one exits immediately, and the user sees a keypress that did nothing.
        eprintln!(
            "nixlaunch: {} needs a terminal and none is configured (set `terminal` in config)",
            app.name
        );
        return false;
    }
    if app.exec.trim().is_empty() {
        // Without this the argv is just the machine prefix, and a remote column would run its
        // forwarding wrapper with no command -- opening a stray session instead of an app.
        eprintln!("nixlaunch: {} has an empty exec line", app.name);
        return false;
    }
    let Some(argv) = launch_argv(machine, app, terminal) else {
        eprintln!("nixlaunch: {} is a read-only column", machine.name);
        return false;
    };
    let Some((bin, args)) = argv.split_first() else {
        eprintln!("nixlaunch: {} has no exec line", app.name);
        return false;
    };
    use std::os::unix::process::CommandExt;

    // OUT OF OUR CGROUP, not merely out of our process group.
    //
    // A resident launcher runs as a systemd unit, and everything it spawns lands in that unit's
    // cgroup. Restarting the unit then kills every application ever started from it -- including
    // forwarded sessions to other machines, which take a visible moment to rebuild.
    //
    // `process_group(0)` below does NOT prevent this: it creates a new process group, while systemd
    // kills by cgroup. The two hierarchies look interchangeable right up until a unit restarts.
    //
    // A transient scope is a unit of its own, so the application leaves this cgroup entirely.
    // Measured, not assumed: a child started this way survives its parent unit being stopped,
    // where the same child started directly does not.
    //
    // Falls back to a plain spawn where systemd-run is absent -- a launcher must not require an
    // init system to start a program.
    let scoped = user_scopes_supported();

    let mut cmd = if scoped {
        let mut c = std::process::Command::new("systemd-run");
        c.args(["--user", "--scope", "--collect", "--quiet", "--", bin]);
        c.args(args);
        c
    } else {
        let mut c = std::process::Command::new(bin);
        c.args(args);
        c
    };
    // Still a separate process group: the property is independent of whether a scope exists.
    cmd.process_group(0);
    match cmd.spawn() {
        Ok(child) => {
            // Dropping `Child` does not reap it. A resident daemon would otherwise accumulate one
            // zombie per launched application for the whole session.
            let pid = gtk::glib::Pid(child.id() as _);
            drop(child);
            gtk::glib::child_watch_add_local(pid, |_, _| {});
            eprintln!("nixlaunch: started {} on {}", app.name, machine.name);
            true
        }
        Err(e) => {
            eprintln!("nixlaunch: {} on {}: {e}", app.name, machine.name);
            false
        }
    }
}

/// Test the capability actually used, once. Finding a `systemd-run` binary says nothing about
/// whether this session has a user manager and bus; assuming it does makes every launch vanish
/// inside a wrapper that immediately fails while the launcher reports success.
fn user_scopes_supported() -> bool {
    static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SUPPORTED.get_or_init(|| {
        std::process::Command::new("systemd-run")
            .args(["--user", "--scope", "--collect", "--quiet", "--", "true"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    })
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The old wait-then-read implementation blocked as soon as either pipe reached 64 KiB. Real
    /// inventories can cross that threshold simply through long absolute Exec paths.
    #[test]
    fn inventory_output_larger_than_a_pipe_is_drained_while_running() {
        let script = concat!(
            "dd if=/dev/zero bs=131072 count=1 2>/dev/null; ",
            "dd if=/dev/zero bs=131072 count=1 1>&2 2>/dev/null"
        );
        let output = command_output(
            "sh",
            &["-c".to_string(), script.to_string()],
            std::time::Duration::from_secs(5),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 131_072);
        assert_eq!(output.stderr.len(), 131_072);
    }

    #[test]
    fn inventory_timeout_is_a_wall_clock_bound() {
        let started = std::time::Instant::now();
        let error = command_output(
            "sh",
            &["-c".to_string(), "sleep 10".to_string()],
            std::time::Duration::from_millis(50),
        )
        .unwrap_err();
        assert!(error.contains("timed out after 50 ms"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }
}
