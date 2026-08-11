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
// ── THIS FILE IS THE GUI ITERATION, WITH FIXTURE DATA ───────────────────────────────────────
// The data below is fake, deliberately, and shaped like `rlaunch --json <host>` already emits
// (host, error, folders[] -> label + apps[] -> name/icon/exec/terminal), plus the one thing that
// inventory cannot know: which apps belong on a line together. Getting the interaction right is
// the hard part and it does not need real data; wiring the real inventory in afterwards is then a
// parse plus an appset table, not a redesign.
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


mod config;
mod icons;
mod model;
mod usage;
use model::*;

/// never has to land in the thin space between two widgets to mean something.
fn insert_index_at(container: &GBox, x: f64) -> usize {
    // `compute_bounds` rather than `allocation()`, which GTK deprecated in 4.12. It answers in the
    // coordinate space of the widget you pass, which is exactly the space the drop's own `x` is
    // already in -- so the two are directly comparable with no offset arithmetic to get wrong.
    let mut idx = 0usize;
    let mut child = container.first_child();
    while let Some(w) = child {
        let Some(b) = w.compute_bounds(container) else { break };
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
/// `config::Theme` -- a colour nobody can reach is this repo carrying one estate's taste.
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
.appname {{ font-size: 12px; }}
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
fn release_idle_pages() {
    unsafe {
        malloc_trim(0);
    }
    let Ok(maps) = std::fs::read_to_string("/proc/self/maps") else { return };
    for line in maps.lines() {
        let mut parts = line.split_whitespace();
        let (Some(range), Some(perms)) = (parts.next(), parts.next()) else { continue };
        // A fourth field beyond the offset/dev/inode means the mapping is file-backed, and those
        // are the ones already cheap to reclaim.
        if parts.clone().count() > 3 {
            continue;
        }
        if !perms.starts_with("rw") {
            continue;
        }
        let Some((lo, hi)) = range.split_once('-') else { continue };
        let (Ok(lo), Ok(hi)) = (usize::from_str_radix(lo, 16), usize::from_str_radix(hi, 16)) else {
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

/// The way back in, published so the single-instance path can reach the window that already exists.
thread_local! {
    static REVEAL: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
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
    let application = Application::builder().application_id("io.github.nixlaunch").build();
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
            existing.present();
            REVEAL.with(|r| {
                if let Some(f) = r.borrow().as_ref() {
                    f();
                }
            });
            return;
        }
        build(app);
    });
    // `run()`, NOT `run_with_args(&[])`. An EMPTY argv is not "no arguments" to GApplication --
    // argv[0] is the program name and g_application_run treats the vector as malformed without
    // it, returning without ever emitting `activate`. The symptom is the worst kind: the process
    // starts, GTK initialises far enough to probe Vulkan, and then simply exits no window, no
    // error. Cost an iteration to find; noted here so it costs nobody another one.
    application.run();
}

fn build(application: &Application) {
    let (folders, base, theme, terminal_cmd_outer, surface_mode, keyboard_mode, exit_on_focus_loss, config_error) = load_world();
    // A placement that exists and does not parse is reported, never assumed empty: the next drag
    // rewrites whatever we decide it was, so guessing "nothing" would overwrite a real arrangement.
    let (placement, placement_error) = load_placement();
    let startup_error = config_error.clone().or(placement_error);


    // NO default size. A launcher should be exactly as big as what it is showing: a fixed size
    // leaves dead space under a short grid and clips a tall one, and BOTH are wrong for a surface
    // whose whole content is known before it maps. Unanchored, a layer surface takes the natural
    // size GTK computes from the children, so the window hugs the matrix and grows with it -- one
    // more folder row makes it taller, a fourth machine makes it wider, with nothing to re-tune.
    //
    // The search entry carries the only explicit measurement, a minimum width, so an empty or
    // heavily-filtered grid cannot collapse the window to a sliver mid-keystroke.
    let window = ApplicationWindow::builder().application(application).build();

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

    // Created before anything can use it, and named apart from the module it comes from.
    let icon_cache = Rc::new(RefCell::new(icons::Icons::load(theme.icon_size)));

    let state = Rc::new(RefCell::new(State {
        canonical_folders: folders.clone(),
        folders,
        usage: usage::load(),
        // Two standard errors, ~95% confidence. Lower and the grid twitches; higher and a real
        // preference takes weeks to show up.
        z: 2.0,
        half_life_days: usage::HALF_LIFE_DAYS,
        base,
        placement: placement,
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
    let spacer = Label::new(None);
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
    // scrolls, so a small estate still gets a window that hugs its content.
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
    // The SMALLEST attached monitor, not the first-enumerated one. Which output the compositor
    // places an unanchored layer surface on is not knowable before it maps, so sizing to monitor 0
    // overflows the moment it lands on a shorter panel -- and a layer surface has no titlebar to
    // drag back into view. Fitting the smallest is correct wherever it ends up.
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

    if exit_on_focus_loss {
        // A GRACE PERIOD, not just a "has been active" flag. Launching from a bar button or a dock
        // means something else held focus at the moment of the click, and focus can bounce once
        // before it settles -- so a handler that closes on the FIRST inactive edge closes the
        // launcher immediately and it reads as "the button does nothing". Ignore focus loss for a
        // moment after mapping; after that, losing the keyboard means the user looked elsewhere
        // and the window should go.
        let armed = std::rc::Rc::new(std::cell::Cell::new(false));
        {
            let armed = armed.clone();
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                armed.set(true);
            });
        }
        let dismiss_on_blur = dismiss.clone();
        window.connect_is_active_notify(move |w| {
            if !w.is_active() && armed.get() {
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
    let painted: Rc<RefCell<Painted>> = Rc::new(RefCell::new(Painted::default()));

    // One theme handle and one texture cache for the life of the process, so a rebuild costs no
    // disk reads at all after the first.
    let icon_theme = gtk::gdk::Display::default()
        .map(|d| gtk::IconTheme::for_display(&d))
        .unwrap_or_default();

    // `render` must be callable from inside a drop handler that `render` itself installed, so it
    // needs a handle to itself. The holder is that indirection -- filled in immediately after
    // construction, and only ever read while no other borrow of it is live.
    let render_holder: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));

    let paint: Rc<dyn Fn()> = Rc::new({
        let state = state.clone();
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

            hint.set_markup(match s.focus {
                Focus::Outside =>
                    "<b>\u{2190}\u{2192}</b> machine   <b>\u{2191}\u{2193}</b> folder   <b>Tab</b>/<b>Enter</b> go inside   <b>Shift+Enter</b> launch the whole cell   <b>drag</b> onto a folder to file  \u{2022}  onto a line to join/reorder it   <b>Esc</b> close",
                Focus::Inside =>
                    "<b>\u{2190}\u{2192}</b> app   <b>\u{2191}\u{2193}</b> line (appset)   <b>Enter</b> launch app   <b>Shift+Enter</b> launch the line   <b>Tab</b>/<b>Esc</b> back out",
            });

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
                let bx = cell.bx.clone();
                let scroller = scroller.clone();
                let grid = grid_for_scroll.clone();
                gtk::glib::idle_add_local_once(move || {
                    let Some(b) = bx.compute_bounds(&grid) else { return };
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
                    reveal(&scroller.vadjustment(), b.y() as f64, (b.y() + b.height()) as f64);
                    reveal(&scroller.hadjustment(), b.x() as f64, (b.x() + b.width()) as f64);
                });
            }
        }
    });

    let render: Rc<dyn Fn()> = Rc::new({
        let state = state.clone();
        let grid = grid.clone();
        let spacer = spacer.clone();
        let holder = render_holder.clone();
        let theme_error = theme.error.clone();
        let painted = painted.clone();
        let paint = paint.clone();
        let icon_px = theme.icon_size;
        let icon_theme = icon_theme.clone();
        let icon_cache = icon_cache.clone();
        move || {
            let s = state.borrow();

            // Every handle recorded below belongs to a widget that is about to be destroyed, so the
            // record is cleared FIRST. Leaving the old ones in place would have the next repaint
            // remove a class from a widget that is no longer in the tree -- harmless, and a silent
            // way for the real selection to keep a highlight it should have lost.
            painted.borrow_mut().reset();

            while let Some(c) = grid.first_child() {
                grid.remove(&c);
            }

            // Equal widths for the MACHINE columns only. `set_column_homogeneous` would drag
            // column 0 (the folder labels) in with them; hexpand alone only shares SPARE width, so
            // a column holding "IntelliJ IDEA" ends up wider than one holding "Foot" and the thing
            // stops reading as a grid. A horizontal SizeGroup sizes every member to the widest of
            // them and leaves column 0 alone, which is exactly the subset rule Grid cannot express.
            let cols = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
            // Rebuilt every render rather than kept: the row-head widgets are recreated each time,
            // and a long-lived group would accumulate memberships for widgets that no longer exist.
            let labelcol = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);
            labelcol.add_widget(&spacer);

            for (c, m) in s.view.iter().enumerate() {
                let head = Label::new(None);
                head.set_xalign(0.0);
                head.add_css_class("colhead");
                // NOT hexpand. A widget in a horizontal SizeGroup already takes the width of the
                // widest member, and asking it to ALSO absorb spare width makes the two rules
                // circular: the group's width depends on the allocation, the allocation depends on
                // the group's width, and inside a ScrolledWindow propagating its child's natural
                // size the loop has nothing to damp it. The symptom was not a wrong layout -- it
                // was a window that repainted ~21 times a second forever and never went idle,
                // roughly one launch in six. The SizeGroup alone gives equal machine columns; the
                // window sizes to content regardless, so nothing needs to absorb slack.
                cols.add_widget(&head);
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
                grid.attach(&head, c as i32 + 1, 0, 1, 1);
            }

            for (r, folder) in s.folders.iter().enumerate() {
                let rh = Label::new(Some(folder));
                rh.set_xalign(1.0);
                rh.set_valign(Align::Center);
                rh.add_css_class("rowhead");
                labelcol.add_widget(&rh);
                grid.attach(&rh, 0, r as i32 + 1, 1, 1);
                painted.borrow_mut().rowheads.push(rh.clone());
                let mut row_cells: Vec<CellW> = Vec::with_capacity(s.view.len());

                for (c, m) in s.view.iter().enumerate() {
                    let lines = &m.cells[r];
                    let cell = GBox::new(Orientation::Vertical, 2);
                    cell.add_css_class("cell");
                    cols.add_widget(&cell);
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
                            let Ok(p) = value.get::<String>() else { return false };
                            let Some((from_col, name)) = p.split_once('\u{1}') else { return false };
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
                            let names: Vec<String> =
                                ln.apps.iter().map(|a| a.id.clone()).collect();
                            tgt.connect_drop(move |_, value, x, _| {
                                let Ok(payload) = value.get::<String>() else { return false };
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
                        let mut line_apps: Vec<GBox> = Vec::with_capacity(ln.apps.len());
                        for app in ln.apps.iter() {
                            let b = GBox::new(Orientation::Horizontal, 4);
                            b.add_css_class("app");
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
                            let img = match icon_cache.borrow_mut().texture(&app.icon, &icon_theme) {
                                Some(tex) => Image::from_paintable(Some(&tex)),
                                None => Image::from_icon_name(&app.icon),
                            };
                            img.set_pixel_size(icon_px);
                            b.append(&img);
                            let l = Label::new(Some(&app.name));
                            l.add_css_class("appname");
                            b.append(&l);
                            lb.append(&b);
                            line_apps.push(b);
                        }
                        cell.append(&lb);
                        cell_lines.push(LineW { bx: lb.clone(), apps: line_apps });
                    }
                    grid.attach(&cell, c as i32 + 1, r as i32 + 1, 1, 1);
                    row_cells.push(CellW { bx: cell.clone(), lines: cell_lines });
                }
                painted.borrow_mut().cells.push(row_cells);
            }

            // The selection classes are NOT set above. Painting them is the other function's job,
            // and doing it here as well would be a second implementation of the same rule, free to
            // disagree with the first the moment either changes.
            drop(s);
            paint();
        }
    });
    *render_holder.borrow_mut() = Some(render.clone());

    // WHAT REOPENING MEANS FOR A RESIDENT PROCESS.
    //
    // A process that never exits never re-reads anything, so without this a launcher left running
    // for a week would still be showing the applications that existed when it started -- install
    // something and it simply would not be there, with no way to tell why. Reopening therefore
    // re-asks every machine what it has.
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
        let reveal: Rc<dyn Fn()> = Rc::new(move || {
            if let Ok(Some(cfg)) = config::load() {
                let rows = cfg.folder_rows();
                let fresh = inventory_all(&cfg.machines, &rows, cfg.theme.line_width);
                let mut s = state.borrow_mut();
                s.base = fresh;
                s.query.clear();
                s.line = 0;
                s.item = 0;
                s.item_goal = 0;
                s.rebuild();
                s.clamp();
            }
            render();
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
        keys.connect_key_pressed(move |_, key, _, mods| {
            let shift = mods.contains(ModifierType::SHIFT_MASK);
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
                match (s.focus, key) {
                    // Esc unwinds one layer at a time rather than always closing: a typed query is
                    // state the user can lose accidentally, so it gets its own step.
                    (_, Key::Escape) => {
                        if !s.query.is_empty() {
                            s.set_query(String::new());
                        } else if s.focus == Focus::Inside {
                            s.focus = Focus::Outside;
                        } else {
                            dismiss(&window);
                            return gtk::glib::Propagation::Stop;
                        }
                    }
                    (_, Key::ISO_Left_Tab) => {
                        // Shift+Tab arrives as a DIFFERENT keysym, so the plain Tab arm never saw
                        // it and the binding was simply dead.
                        s.focus = Focus::Outside;
                    }
                    (_, Key::Tab) => {
                        s.focus = if s.focus == Focus::Outside && !s.cell().is_empty() {
                            Focus::Inside
                        } else {
                            Focus::Outside
                        };
                        s.line = 0;
                        s.item = 0;
                        s.item_goal = 0;
                    }

                    (Focus::Outside, Key::Left) => s.col = s.col.saturating_sub(1),
                    (Focus::Outside, Key::Right) => {
                        s.col = (s.col + 1).min(s.view.len().saturating_sub(1))
                    }
                    (Focus::Outside, Key::Up) => s.row = s.row.saturating_sub(1),
                    (Focus::Outside, Key::Down) => {
                        let last = s.folders.len().saturating_sub(1);
                        s.row = (s.row + 1).min(last);
                    }
                    (Focus::Outside, Key::Return) => {
                        if shift {
                            let machine = s.view[s.col].clone();
                            let all: Vec<App> =
                                s.cell().iter().flat_map(|l| l.apps.iter().cloned()).collect();
                            if !all.is_empty() {
                                for app in &all {
                                    spawn(&machine, app, &terminal_cmd);
                                    s.record_launch(&machine.name, &app.id);
                                }
                                dismiss(&window);
                                return gtk::glib::Propagation::Stop;
                            }
                        } else if !s.cell().is_empty() {
                            s.focus = Focus::Inside;
                            s.line = 0;
                            s.item = 0;
                            s.item_goal = 0;
                        }
                    }

                    // Left/right are the only keys that CHOOSE a column, so they are the only ones
                    // that move the goal. Up/down just change line and let `clamp` re-aim.
                    (Focus::Inside, Key::Left) => {
                        s.item = s.item.saturating_sub(1);
                        s.item_goal = s.item;
                    }
                    (Focus::Inside, Key::Right) => {
                        let n = s.current_line().map(|l| l.apps.len()).unwrap_or(0);
                        s.item = (s.item + 1).min(n.saturating_sub(1));
                        s.item_goal = s.item;
                    }
                    (Focus::Inside, Key::Up) => s.line = s.line.saturating_sub(1),
                    (Focus::Inside, Key::Down) => {
                        s.line = (s.line + 1).min(s.cell().len().saturating_sub(1))
                    }
                    (Focus::Inside, Key::Return) => {
                        let machine = s.view[s.col].clone();
                        let apps: Vec<App> = if shift {
                            s.current_line().map(|l| l.apps.clone()).unwrap_or_default()
                        } else {
                            s.current_line()
                                .and_then(|l| l.apps.get(s.item))
                                .cloned()
                                .into_iter()
                                .collect()
                        };
                        if !apps.is_empty() {
                            for app in &apps {
                                spawn(&machine, app, &terminal_cmd);
                                s.record_launch(&machine.name, &app.id);
                            }
                            // A launcher that stays up after launching is a window you then have
                            // to dismiss. Closing IS the confirmation.
                            dismiss(&window);
                            return gtk::glib::Propagation::Stop;
                        }
                    }

                    (_, Key::BackSpace) => {
                        let mut q = s.query.clone();
                        q.pop();
                        s.set_query(q);
                    }
                    _ => {
                        // A chord is a command, not text. Without this, Ctrl-W and Alt-F typed a
                        // literal "w" and "f" into the search box.
                        let chord = mods.contains(ModifierType::CONTROL_MASK)
                            || mods.contains(ModifierType::ALT_MASK)
                            || mods.contains(ModifierType::SUPER_MASK);
                        if !chord {
                            if let Some(ch) = key.to_unicode() {
                                if !ch.is_control() {
                                    let mut q = s.query.clone();
                                    q.push(ch);
                                    s.set_query(q);
                                }
                            }
                        }
                    }
                }
                s.clamp();
                structural = s.query != before;
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

    window.present();
}

/// Config if there is one, fixture data if there is not.
///
/// A MISSING config is not an error -- a bare checkout must still run, which is what keeps this
/// repo testable by someone with no fleet. A config that EXISTS but does not parse is a different
/// thing entirely and is reported rather than silently replaced by fixtures, because silently
/// showing invented machines when the real ones failed to load is the worst of both.
fn load_world() -> (Vec<String>, Vec<Machine>, config::Theme, Vec<String>, String, String, bool, Option<String>) {
    let default_rows = || DEFAULT_FOLDERS.iter().map(|f| f.to_string()).collect::<Vec<_>>();
    match config::load() {
        Err(e) => (default_rows(), fixture(), config::Theme::default(), vec![], "layer".into(), "exclusive".into(), true, Some(e)),
        Ok(None) => (default_rows(), fixture(), config::Theme::default(), vec![], "layer".into(), "exclusive".into(), true, None),
        Ok(Some(cfg)) => {
            let rows = cfg.folder_rows();
            let machines = inventory_all(&cfg.machines, &rows, cfg.theme.line_width);
            (rows, machines, cfg.theme.clone(), cfg.terminal.clone(), cfg.surface.clone(), cfg.keyboard.clone(), cfg.exit_on_focus_loss, None)
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
fn inventory_all(machines: &[config::MachineConfig], rows: &[String], line_width: usize) -> Vec<Machine> {
    std::thread::scope(|scope| {
        let handles: Vec<_> = machines
            .iter()
            .map(|mc| scope.spawn(move || inventory(mc, rows, line_width)))
            .collect();
        handles
            .into_iter()
            .zip(machines)
            .map(|(h, mc)| {
                h.join().unwrap_or_else(|_| Machine {
                    name: mc.name.clone(),
                    accent: mc.accent.clone(),
                    launch: mc.launch.clone(),
                    error: Some("inventory panicked".into()),
                    cells: vec![Vec::new(); rows.len()],
                })
            })
            .collect()
    })
}

/// Ask ONE machine what it has, by running the command config named for it.
///
/// Everything this program knows about detection is in these few lines: run argv, read JSON. No
/// SSH, no .desktop parsing, no package managers -- see config.rs on why that boundary is the
/// point rather than a simplification.
fn inventory(mc: &config::MachineConfig, rows: &[String], line_width: usize) -> Machine {
    let mut cells: Vec<Vec<Line>> = vec![Vec::new(); rows.len()];
    let mut error = None;

    let parsed = mc
        .inventory
        .split_first()
        .ok_or_else(|| "no inventory command configured".to_string())
        .and_then(|(bin, args)| {
            std::process::Command::new(bin)
                .args(args)
                .output()
                .map_err(|e| format!("{bin}: {e}"))
        })
        .and_then(|out| {
            if out.status.success() {
                serde_json::from_slice::<config::Inventory>(&out.stdout)
                    .map_err(|e| format!("unreadable inventory: {e}"))
            } else {
                Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
            }
        });

    match parsed {
        Ok(inv) => {
            // The machine reporting its own failure outranks anything inferred here.
            error = inv.error;
            for folder in inv.folders {
                // A label this estate does not list falls into the inbox rather than being
                // dropped: an app nobody categorised must still be reachable.
                let r = rows
                    .iter()
                    .position(|x| *x == folder.label)
                    .unwrap_or(rows.len().saturating_sub(1));
                // PACK, do not stack. A fresh inventory has no appsets in it -- an appset is
                // something the user makes by dragging -- so giving every app a line of its own
                // produces a cell as tall as the folder is long, and destroys the 2D navigation
                // inside the box: up/down would step one app at a time and left/right would do
                // nothing. Chunking into rows of `theme.line_width` makes a cell a small grid, which is
                // what the inside of a box is supposed to be. The fixture hid this completely,
                // because hand-written fixtures are always short.
                for chunk in folder.apps.chunks(line_width.max(1)) {
                    cells[r].push(Line {
                        apps: chunk
                            .iter()
                            .map(|a| App {
                                // No id from this provider means it predates the field, and the
                                // name is the best identity available -- which is exactly the old
                                // behaviour, correct until two of its apps share a display name.
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

    Machine { name: mc.name.clone(), accent: mc.accent.clone(), launch: mc.launch.clone(), error, cells }
}

/// Start one application, detached.
///
/// DETACHED ON PURPOSE. The launcher exits immediately after launching, and a child in our own
/// process group would be killed with us -- so the thing you just started would vanish. `setsid`
/// via `process_group(0)` is what makes the app outlive the launcher, which is the entire point of
/// a launcher.
///
/// Failures are reported, not swallowed: a missing binary is exactly the case where silence would
/// look like the keypress never registered.
fn spawn(machine: &Machine, app: &App, terminal: &[String]) {
    if app.terminal && terminal.is_empty() {
        // Say so rather than launching something that will vanish: a terminal program spawned
        // without one exits immediately, and the user sees a keypress that did nothing.
        eprintln!(
            "nixlaunch: {} needs a terminal and none is configured (set `terminal` in config)",
            app.name
        );
    }
    if app.exec.trim().is_empty() {
        // Without this the argv is just the machine prefix, and a remote column would run its
        // forwarding wrapper with no command -- opening a stray session instead of an app.
        eprintln!("nixlaunch: {} has an empty exec line", app.name);
        return;
    }
    let argv = launch_argv(machine, app, terminal);
    let Some((bin, args)) = argv.split_first() else {
        eprintln!("nixlaunch: {} has no exec line", app.name);
        return;
    };
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(bin);
    cmd.args(args);
    cmd.process_group(0);
    match cmd.spawn() {
        Ok(_) => eprintln!("nixlaunch: started {} on {}", app.name, machine.name),
        Err(e) => eprintln!("nixlaunch: {} on {}: {e}", app.name, machine.name),
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

