// nixlaunch — a launcher whose layout is a MATRIX: machines across, folders down, appsets within.
//
// The shape is the point. Every other launcher on Wayland is a search box over ONE list, so the
// only way it can express "which machine" or "which kind of thing" is by making you narrow a
// single flat set. A screen is two-dimensional; a matrix uses both axes, so "the editors on
// archlxc" is a POSITION you move to rather than a query you compose.
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
mod model;
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
const CSS: &str = "
window { background-color: #0A0A0A; color: #F0F0F0; }
.root { padding: 18px; }
.search { font-size: 15px; padding: 8px 12px; margin-bottom: 14px;
          border: 1px solid #262626; border-radius: 6px; background-color: #111111; }
.search.empty { color: #666666; }
.colhead { font-weight: bold; font-size: 13px; padding: 4px 8px; margin-bottom: 6px;
           border-bottom: 2px solid #262626; }
.rowhead { font-size: 13px; color: #999999; padding-right: 12px; }
.rowhead.active { color: #F0F0F0; font-weight: bold; }
.cell { border: 1px solid #1C1C1C; border-radius: 6px; padding: 5px; margin: 3px;
        background-color: #0E0E0E; }
.cell.cursor { border-color: #22C55E; background-color: #101810; }
.cell.inside { border-width: 2px; padding: 4px; }
.cell.empty { border-style: dashed; }
.line { border-radius: 4px; padding: 2px; }
.line.sel { background-color: #22C55E1A; }
.app { padding: 3px 6px; border-radius: 4px; }
.app.sel { background-color: #22C55E33; }
.appname { font-size: 12px; }
.dim { color: #444444; font-size: 12px; font-style: italic; }
.hint { color: #666666; font-size: 11px; margin-top: 12px; }
.hint b { color: #22C55E; }
";

fn main() {
    let application = Application::builder().application_id("ch.corbet.nixlaunch").build();
    application.connect_activate(|app| {
        eprintln!("nixlaunch: activate");
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
    let provider = CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, 800);
    }

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
    if std::env::var_os("NIXLAUNCH_NO_LAYER").is_none() {
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        // OnDemand, NOT Exclusive. Exclusive takes the keyboard away from everything else for as
        // long as the surface exists, which is correct for a lock screen and wrong for a launcher
        // -- it means the rest of the session cannot be typed into while this is open, and it
        // feels like the window has seized the machine rather than been given focus. OnDemand
        // asks the compositor for focus the normal way, so it behaves like any other window and
        // clicking away actually works.
        window.set_keyboard_mode(KeyboardMode::OnDemand);
    }

    let state = Rc::new(RefCell::new(State {
        base: fixture(),
        placement: load_placement(),
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
    // Applies saved filings, then populates `view` with an empty query, i.e. everything.
    state.borrow_mut().rebuild();

    let root = GBox::new(Orientation::Vertical, 0);
    root.add_css_class("root");

    let search = Label::new(None);
    search.set_xalign(0.0);
    search.add_css_class("search");
    // The one fixed measurement in the layout -- see the window comment above.
    search.set_width_request(760);
    root.append(&search);

    let grid = gtk::Grid::new();
    // NOT column-homogeneous. That makes EVERY column the same width including column 0, which
    // holds nothing but the folder labels -- so the label column gets sized like a machine column
    // and leaves a wide empty gutter down the left with the labels shoved into its far edge.
    // Instead the machine columns carry `hexpand` (set per cell below) and share the spare width
    // equally between themselves, while column 0 takes only the width its longest label needs.
    grid.set_column_homogeneous(false);
    root.append(&grid);

    let hint = Label::new(None);
    hint.set_xalign(0.0);
    hint.set_use_markup(true);
    hint.add_css_class("hint");
    root.append(&hint);

    window.set_child(Some(&root));

    // `render` must be callable from inside a drop handler that `render` itself installed, so it
    // needs a handle to itself. The holder is that indirection -- filled in immediately after
    // construction, and only ever read while no other borrow of it is live.
    let render_holder: Rc<RefCell<Option<Rc<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let render: Rc<dyn Fn()> = Rc::new({
        let state = state.clone();
        let grid = grid.clone();
        let search = search.clone();
        let hint = hint.clone();
        let holder = render_holder.clone();
        move || {
            let s = state.borrow();

            if s.query.is_empty() {
                search.set_text("type to search\u{2026}");
                search.add_css_class("empty");
            } else {
                search.set_text(&s.query);
                search.remove_css_class("empty");
            }

            while let Some(c) = grid.first_child() {
                grid.remove(&c);
            }

            // Equal widths for the MACHINE columns only. `set_column_homogeneous` would drag
            // column 0 (the folder labels) in with them; hexpand alone only shares SPARE width, so
            // a column holding "IntelliJ IDEA" ends up wider than one holding "Foot" and the thing
            // stops reading as a grid. A horizontal SizeGroup sizes every member to the widest of
            // them and leaves column 0 alone, which is exactly the subset rule Grid cannot express.
            let cols = gtk::SizeGroup::new(gtk::SizeGroupMode::Horizontal);

            for (c, m) in s.view.iter().enumerate() {
                let head = Label::new(None);
                head.set_xalign(0.0);
                head.add_css_class("colhead");
                head.set_hexpand(true);
                cols.add_widget(&head);
                head.set_markup(&format!(
                    "<span foreground=\"{}\">{}</span>",
                    m.accent,
                    escape(&m.name)
                ));
                grid.attach(&head, c as i32 + 1, 0, 1, 1);
            }

            for (r, folder) in FOLDERS.iter().enumerate() {
                let rh = Label::new(Some(folder));
                rh.set_xalign(1.0);
                rh.set_valign(Align::Center);
                rh.add_css_class("rowhead");
                if r == s.row {
                    rh.add_css_class("active");
                }
                grid.attach(&rh, 0, r as i32 + 1, 1, 1);

                for (c, m) in s.view.iter().enumerate() {
                    let lines = &m.cells[r];
                    let cell = GBox::new(Orientation::Vertical, 2);
                    cell.add_css_class("cell");
                    cell.set_hexpand(true);
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
                            st.borrow_mut().place_app(c, name, r, None, 0);
                            if let Some(rf) = holder2.borrow().as_ref() {
                                rf();
                            }
                            true
                        });
                        cell.add_controller(tgt);
                    }
                    let is_cursor = c == s.col && r == s.row;
                    let inside = is_cursor && s.focus == Focus::Inside;
                    if is_cursor {
                        cell.add_css_class("cursor");
                    }
                    if inside {
                        cell.add_css_class("inside");
                    }
                    if lines.is_empty() {
                        cell.add_css_class("empty");
                        let dash = Label::new(Some("\u{2014}"));
                        dash.set_xalign(0.0);
                        dash.add_css_class("dim");
                        cell.append(&dash);
                    }
                    for (li, ln) in lines.iter().enumerate() {
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
                            tgt.connect_drop(move |_, value, x, _| {
                                let Ok(payload) = value.get::<String>() else { return false };
                                let Some((from_col, name)) = payload.split_once('\u{1}') else {
                                    return false;
                                };
                                if from_col.parse::<usize>().ok() != Some(c) {
                                    return false;
                                }
                                let at = insert_index_at(&lb2, x);
                                st.borrow_mut().place_app(c, name, r, Some(li), at);
                                if let Some(rf) = holder2.borrow().as_ref() {
                                    rf();
                                }
                                true
                            });
                            lb.add_controller(tgt);
                        }
                        if inside && li == s.line {
                            lb.add_css_class("sel");
                        }
                        for (ii, app) in ln.apps.iter().enumerate() {
                            let b = GBox::new(Orientation::Horizontal, 4);
                            b.add_css_class("app");
                            // The payload carries the COLUMN it came from as well as the name, so
                            // the drop side can refuse a cross-machine drag without having to ask
                            // anyone: filing is per machine, and "Firefox on archlxc" is not the
                            // same object as "Firefox on elitebook".
                            {
                                let src = gtk::DragSource::new();
                                src.set_actions(gtk::gdk::DragAction::MOVE);
                                let payload = format!("{}\u{1}{}", c, app.name);
                                src.connect_prepare(move |_, _, _| {
                                    Some(gtk::gdk::ContentProvider::for_value(&payload.to_value()))
                                });
                                b.add_controller(src);
                            }
                            if inside && li == s.line && ii == s.item {
                                b.add_css_class("sel");
                            }
                            let img = Image::from_icon_name(&app.icon);
                            img.set_pixel_size(20);
                            b.append(&img);
                            let l = Label::new(Some(&app.name));
                            l.add_css_class("appname");
                            b.append(&l);
                            lb.append(&b);
                        }
                        cell.append(&lb);
                    }
                    grid.attach(&cell, c as i32 + 1, r as i32 + 1, 1, 1);
                }
            }

            hint.set_markup(match s.focus {
                Focus::Outside =>
                    "<b>\u{2190}\u{2192}</b> machine   <b>\u{2191}\u{2193}</b> folder   <b>Tab</b>/<b>Enter</b> go inside   <b>Shift+Enter</b> launch the whole cell   <b>drag</b> onto a folder to file  \u{2022}  onto a line to join/reorder it   <b>Esc</b> close",
                Focus::Inside =>
                    "<b>\u{2190}\u{2192}</b> app   <b>\u{2191}\u{2193}</b> line (appset)   <b>Enter</b> launch app   <b>Shift+Enter</b> launch the line   <b>Tab</b>/<b>Esc</b> back out",
            });
        }
    });
    *render_holder.borrow_mut() = Some(render.clone());

    render();

    let keys = EventControllerKey::new();
    {
        let state = state.clone();
        let window = window.clone();
        let render = render.clone();
        keys.connect_key_pressed(move |_, key, _, mods| {
            let shift = mods.contains(ModifierType::SHIFT_MASK);
            {
                let mut s = state.borrow_mut();
                match (s.focus, key) {
                    // Esc unwinds one layer at a time rather than always closing: a typed query is
                    // state the user can lose accidentally, so it gets its own step.
                    (_, Key::Escape) => {
                        if !s.query.is_empty() {
                            s.query.clear();
                            s.refilter();
                        } else if s.focus == Focus::Inside {
                            s.focus = Focus::Outside;
                        } else {
                            window.close();
                            return gtk::glib::Propagation::Stop;
                        }
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
                    (Focus::Outside, Key::Down) => s.row = (s.row + 1).min(FOLDERS.len() - 1),
                    (Focus::Outside, Key::Return) => {
                        if shift {
                            let all: Vec<String> = s
                                .cell()
                                .iter()
                                .flat_map(|l| l.apps.iter().map(|x| x.name.clone()))
                                .collect();
                            println!("CELL on {}: {:?}", s.view[s.col].name, all);
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
                        let host = s.view[s.col].name.clone();
                        if shift {
                            let set: Vec<String> = s
                                .current_line()
                                .map(|l| l.apps.iter().map(|x| x.name.clone()).collect())
                                .unwrap_or_default();
                            println!("APPSET on {}: {:?}", host, set);
                        } else if let Some(app) = s.current_line().and_then(|l| l.apps.get(s.item)) {
                            println!("LAUNCH {} on {}", app.name, host);
                        }
                    }

                    (_, Key::BackSpace) => {
                        s.query.pop();
                        s.refilter();
                    }
                    _ => {
                        if let Some(ch) = key.to_unicode() {
                            if !ch.is_control() {
                                s.query.push(ch);
                                s.refilter();
                            }
                        }
                    }
                }
                s.clamp();
            }
            render();
            gtk::glib::Propagation::Stop
        });
    }
    window.add_controller(keys);

    window.present();
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

