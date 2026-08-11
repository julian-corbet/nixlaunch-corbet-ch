// model.rs — everything nixlaunch knows, with no pixels in it.
//
// SPLIT ON PURPOSE. Every bug found while building this was in here and none of them were in the
// drawing: the box that was not really 2D, the goal column that drifted left, the search that
// filtered nothing, the cursor stranded on an emptied cell. All of it is ordinary data movement,
// and none of it needs a display to exercise -- so it lives apart from GTK and is covered by real
// tests at the bottom of this file rather than by opening the launcher and squinting.
//
// The rule that keeps it that way: nothing in this file may import gtk.
use serde_json;
use std::collections::HashMap;
use std::path::PathBuf;

// ── the model ───────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct App {
    pub name: String,
    pub icon: String,
}

/// A line IS an appset: the apps on it are meant to be started together, and the fact that they
/// sit on one line is the whole declaration. No separate "group" concept, no naming ceremony.
#[derive(Clone)]
pub struct Line {
    pub apps: Vec<App>,
}

#[derive(Clone)]
pub struct Machine {
    pub name: String,
    pub accent: String,
    /// Parallel to `FOLDERS`: cells[r] is this machine's lines for folder r. An empty cell is a
    /// real and common state worth DRAWING rather than collapsing -- it tells you "archlxc has no
    /// chat client" at a glance, which a list-based launcher can only tell you by absence.
    pub cells: Vec<Vec<Line>>,
}

/// Folder order is the operator's own priority table (infra's rlaunch.nix), not alphabetical.
///
/// "Other" is LAST and PERMANENT -- it is drawn even when empty, because it is not a category, it
/// is the inbox. Anything the category table does not recognise lands there, which is exactly what
/// happens the first time a newly-installed app appears: it shows up in a known place instead of
/// silently joining a list of two hundred. Filing it from there is a drag, and the filing sticks
/// (see `Placement`). The inventory side already agrees -- rlaunch's own bucket() returns "Other"
/// for anything unmatched and orders it last.
pub const FOLDERS: &[&str] =
    &["Terminals", "Editors", "Browsers", "Chat", "Files", "Media", "Other"];

/// The user's own ARRANGEMENT of a machine's apps: machine -> folder label -> lines of app names.
///
/// A folder-per-app map (what this was first) can say "Bottles belongs in Files" and nothing more.
/// It cannot express WHERE on a line, or which line, or the order within one -- and those are
/// exactly the decisions dragging is for. Recording the arrangement itself makes all of them one
/// mechanism: a drop rewrites a position in this structure, and the grid is re-derived from it.
///
/// Apps NOT named here keep whatever folder the category table computed and appear as their own
/// line, so a newly-installed app still lands in "Other" without any entry existing for it. An app
/// named here that no longer exists is skipped silently -- uninstalling something must not corrupt
/// the arrangement of everything around it.
pub type Placement = HashMap<String, HashMap<String, Vec<Vec<String>>>>;

/// STATE, not config: a record of what the user did, written by the program itself, so it belongs
/// under XDG_STATE_HOME and emphatically not in the Nix store or a config file a rebuild would
/// overwrite. "This should be permanent" is the whole requirement.
pub fn placement_path() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("nixlaunch").join("placement.json")
}

pub fn load_placement() -> Placement {
    std::fs::read_to_string(placement_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Best-effort. A launcher that refused to open because it could not write its placement file
/// would be trading the whole feature for one of its conveniences.
pub fn save_placement(p: &Placement) {
    let path = placement_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(p) {
        let _ = std::fs::write(&path, text);
    }
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Focus {
    Outside,
    Inside,
}

pub struct State {
    /// The inventory EXACTLY as the category table produced it. Never mutated: filing an app is
    /// recorded in `placement` and re-applied, so a re-inventory can replace this wholesale and
    /// every filing survives. Mutating the grid in place instead would make the user's decisions
    /// indistinguishable from the guesses they were correcting.
    pub base: Vec<Machine>,
    /// The user's own arrangement. Persisted; see `placement_path`.
    pub placement: Placement,
    /// `base` with `placement` applied. Filtered into `view`.
    pub machines: Vec<Machine>,
    /// `machines` with the query applied. Navigation and rendering BOTH read this, so the cursor
    /// can never point at something the user cannot see.
    pub view: Vec<Machine>,
    pub col: usize,
    pub row: usize,
    pub line: usize,
    pub item: usize,
    /// The column the user last chose DELIBERATELY, with left/right. Moving up or down aims for
    /// it rather than for wherever the previous line happened to end -- so a cell is a real 2D
    /// grid: at line 1 item 2, Down lands on line 2 item 2. Crossing a SHORT line clamps for that
    /// line only and does not forget the goal, which is what makes down-then-up return you to
    /// where you started instead of drifting left. Same model a text editor uses for a column.
    pub item_goal: usize,
    pub focus: Focus,
    pub query: String,
}

impl State {
    pub fn cell(&self) -> &Vec<Line> {
        &self.view[self.col].cells[self.row]
    }

    pub fn current_line(&self) -> Option<&Line> {
        self.cell().get(self.line)
    }

    /// Re-derive the grid from the pristine inventory plus the user's filings, then re-filter.
    /// Called on startup and after every drop, so there is exactly one path from
    /// (inventory, placement) to what is on screen.
    pub fn rebuild(&mut self) {
        self.machines = apply_placement(&self.base, &self.placement);
        self.refilter();
    }

    /// Freeze a machine's CURRENT grid into the placement verbatim.
    ///
    /// Called before every move, and the reason is subtle: until the user has touched a machine,
    /// most of its apps have no placement entry at all and are drawn from the computed grouping.
    /// Moving one app without first recording the rest would leave its neighbours to be re-derived
    /// and re-appended, so they would visibly jump. Snapshotting first means a drop changes exactly
    /// the one thing the user dragged.
    pub fn materialise(&mut self, mi: usize) {
        let m = &self.machines[mi];
        let mut folders: HashMap<String, Vec<Vec<String>>> = HashMap::new();
        for (r, lines) in m.cells.iter().enumerate() {
            if lines.is_empty() {
                continue;
            }
            folders.insert(
                FOLDERS[r].to_string(),
                lines.iter().map(|l| l.apps.iter().map(|a| a.name.clone()).collect()).collect(),
            );
        }
        self.placement.insert(m.name.clone(), folders);
    }

    /// Move `app` on machine `mi` to `folder`, either INTO an existing line at `pos`, or onto a
    /// new line of its own when `target_line` is None. This one call covers filing into a folder,
    /// joining an appset at a chosen position, and reordering within a line -- they differ only in
    /// where the drop landed, which is the point of recording arrangements rather than folders.
    pub fn place_app(
        &mut self,
        mi: usize,
        app: &str,
        folder: usize,
        target_line: Option<usize>,
        pos: usize,
    ) {
        self.materialise(mi);
        let machine = self.machines[mi].name.clone();
        let folders = self.placement.entry(machine).or_default();

        // Remove it from wherever it currently is, and drop any line that empties as a result.
        for lines in folders.values_mut() {
            for l in lines.iter_mut() {
                l.retain(|n| n != app);
            }
            lines.retain(|l| !l.is_empty());
        }

        let lines = folders.entry(FOLDERS[folder].to_string()).or_default();
        match target_line {
            // The target line can have vanished in the removal above (it held only this app), so
            // an out-of-range index degrades to "its own line" rather than panicking.
            Some(li) if li < lines.len() => {
                let at = pos.min(lines[li].len());
                lines[li].insert(at, app.to_string());
            }
            _ => lines.push(vec![app.to_string()]),
        }

        save_placement(&self.placement);
        self.rebuild();
    }

    /// Case-insensitive substring over app names, applied to EVERY cell at once -- the whole point
    /// of a matrix is that you can see "where does this thing exist" across machines, and a filter
    /// that only searched the current column would throw that away. Lines that lose every app
    /// disappear; a cell that loses every line renders as empty, which is itself the answer to
    /// "does archlxc have this?".
    pub fn refilter(&mut self) {
        let q = self.query.to_lowercase();
        self.view = self
            .machines
            .iter()
            .map(|m| Machine {
                name: m.name.clone(),
                accent: m.accent.clone(),
                cells: m
                    .cells
                    .iter()
                    .map(|lines| {
                        lines
                            .iter()
                            .filter_map(|l| {
                                let apps: Vec<App> = l
                                    .apps
                                    .iter()
                                    .filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q))
                                    .cloned()
                                    .collect();
                                if apps.is_empty() { None } else { Some(Line { apps }) }
                            })
                            .collect()
                    })
                    .collect(),
            })
            .collect();
        self.snap_to_content();
    }

    /// After a filter, the cursor is very likely sitting on a cell that no longer has anything in
    /// it. Rather than leaving it stranded, walk to the first cell that DOES -- reading order,
    /// row-major -- so typing always leaves something selected and Enter always means something.
    pub fn snap_to_content(&mut self) {
        if !self.cell().is_empty() {
            return;
        }
        for r in 0..FOLDERS.len() {
            for c in 0..self.view.len() {
                if !self.view[c].cells[r].is_empty() {
                    self.row = r;
                    self.col = c;
                    self.line = 0;
                    self.item = 0;
                    return;
                }
            }
        }
    }

    /// Every cursor move funnels through here, so no key handler can leave the cursor pointing
    /// into a cell or line that shrank under it -- the class of bug that only appears once real
    /// inventories differ in length between machines, or the moment a filter empties something.
    pub fn clamp(&mut self) {
        self.col = self.col.min(self.view.len().saturating_sub(1));
        self.row = self.row.min(FOLDERS.len().saturating_sub(1));
        let lines = self.cell().len();
        if lines == 0 {
            self.line = 0;
            self.item = 0;
            self.focus = Focus::Outside;
            return;
        }
        self.line = self.line.min(lines - 1);
        let items = self.cell()[self.line].apps.len();
        // Aim for the goal column, clamped to THIS line -- never overwrite the goal itself.
        self.item = if items == 0 { 0 } else { self.item_goal.min(items - 1) };
    }
}

/// Re-derive a machine's grid from the pristine inventory plus the user's arrangement.
///
/// Two passes, and the order is the contract: the user's own lines first, in their own order, then
/// everything the arrangement does not mention, in the folder the category table computed for it.
/// That is what makes a newly-installed app appear in "Other" without any entry existing for it,
/// while nothing the user has already arranged moves to make room.
///
/// Names that no longer resolve are skipped rather than removed from the arrangement -- uninstall
/// something and reinstall it later and it returns to where you put it.
pub fn apply_placement(base: &[Machine], p: &Placement) -> Vec<Machine> {
    base.iter()
        .map(|m| {
            let mut by_name: HashMap<&str, &App> = HashMap::new();
            for lines in &m.cells {
                for l in lines {
                    for app in &l.apps {
                        by_name.insert(app.name.as_str(), app);
                    }
                }
            }

            let arranged = p.get(&m.name);
            let mut placed: std::collections::HashSet<&str> = std::collections::HashSet::new();
            if let Some(folders) = arranged {
                for lines in folders.values() {
                    for l in lines {
                        for n in l {
                            placed.insert(n.as_str());
                        }
                    }
                }
            }

            let mut cells: Vec<Vec<Line>> = vec![Vec::new(); FOLDERS.len()];

            if let Some(folders) = arranged {
                for (fi, fname) in FOLDERS.iter().enumerate() {
                    let Some(lines) = folders.get(*fname) else { continue };
                    for l in lines {
                        let apps: Vec<App> =
                            l.iter().filter_map(|n| by_name.get(n.as_str()).map(|a| (*a).clone())).collect();
                        if !apps.is_empty() {
                            cells[fi].push(Line { apps });
                        }
                    }
                }
            }

            for (r, lines) in m.cells.iter().enumerate() {
                for l in lines {
                    let apps: Vec<App> =
                        l.apps.iter().filter(|a| !placed.contains(a.name.as_str())).cloned().collect();
                    if !apps.is_empty() {
                        cells[r].push(Line { apps });
                    }
                }
            }

            Machine { name: m.name.clone(), accent: m.accent.clone(), cells }
        })
        .collect()
}

/// Which GAP between a line's items a drop at `x` belongs to.
///
/// Compared against each child's MIDPOINT, not its edges, so the target flips when the pointer
/// passes the middle of an item -- the behaviour every reorderable list has, and the reason a drop

pub fn a(name: &str, icon: &str) -> App {
    App { name: name.into(), icon: icon.into() }
}

pub fn line(apps: Vec<App>) -> Line {
    Line { apps }
}

/// FIXTURE. Deliberately uneven -- different line counts per cell, different item counts per
/// line, and some cells genuinely empty -- because an even grid hides exactly the layout bugs a
/// real fleet produces.
pub fn fixture() -> Vec<Machine> {
    vec![
        Machine {
            name: "elitebook".into(),
            accent: "#166534".into(),
            cells: vec![
                vec![line(vec![a("Foot", "foot"), a("Foot Client", "foot")]), line(vec![a("Foot Server", "foot")])],
                vec![
                    line(vec![a("Helix", "helix"), a("Code - OSS", "com.visualstudio.code.oss")]),
                    line(vec![a("Builder", "org.gnome.Builder"), a("IntelliJ IDEA", "idea")]),
                ],
                vec![line(vec![a("Firefox", "firefox"), a("Chromium", "chromium")])],
                // The appset that started this whole conversation: all the messengers, one line.
                vec![line(vec![a("Telegram", "org.telegram.desktop"), a("ZapZap", "com.rtosta.zapzap"), a("Teams", "teams-for-linux")])],
                vec![line(vec![a("Thunar", "thunar"), a("Czkawka", "com.github.qarmin.czkawka")])],
                vec![line(vec![a("mpv", "mpv"), a("VLC", "vlc")])],
                // Other: the inbox. Two apps the category table did not recognise.
                vec![line(vec![a("Czkawka", "com.github.qarmin.czkawka")]), line(vec![a("Bottles", "com.usebottles.bottles")])],
            ],
        },
        Machine {
            name: "archlxc".into(),
            accent: "#B45309".into(),
            cells: vec![
                vec![line(vec![a("Foot", "foot"), a("Alacritty", "Alacritty")])],
                vec![line(vec![a("Helix", "helix"), a("Zed", "dev.zed.Zed")])],
                vec![line(vec![a("Firefox", "firefox")])],
                vec![],
                vec![line(vec![a("Thunar", "thunar")])],
                vec![
                    line(vec![a("mpv", "mpv"), a("VLC", "vlc")]),
                    line(vec![a("Kdenlive", "kdenlive"), a("GIMP", "gimp")]),
                ],
                vec![line(vec![a("Steam", "steam")])],
            ],
        },
        Machine {
            name: "devhome".into(),
            accent: "#9F1239".into(),
            cells: vec![
                vec![line(vec![a("Foot", "foot")])],
                vec![line(vec![a("Helix", "helix"), a("Nano", "nano")])],
                vec![],
                vec![line(vec![a("aerc", "utilities-terminal")])],
                vec![line(vec![a("Engrampa", "engrampa")])],
                vec![line(vec![a("mpv", "mpv")])],
                // An EMPTY inbox -- the steady state, and still drawn.
                vec![],
            ],
        },
    ]
}


// ── tests ───────────────────────────────────────────────────────────────────────────────────
//
// These cover the four things that were actually wrong at some point while building this, plus
// the invariants that stop them coming back. Every one of them runs without a display.
#[cfg(test)]
mod tests {
    use super::*;

    fn app(n: &str) -> App {
        App { name: n.into(), icon: "x".into() }
    }

    /// One machine, one folder, with the given lines. `folder` indexes FOLDERS.
    fn machine(name: &str, folder: usize, lines: Vec<Vec<&str>>) -> Machine {
        let mut cells: Vec<Vec<Line>> = vec![Vec::new(); FOLDERS.len()];
        cells[folder] =
            lines.into_iter().map(|l| Line { apps: l.into_iter().map(app).collect() }).collect();
        Machine { name: name.into(), accent: "#fff".into(), cells }
    }

    fn state(machines: Vec<Machine>) -> State {
        let mut s = State {
            base: machines,
            placement: Placement::new(),
            machines: Vec::new(),
            view: Vec::new(),
            col: 0,
            row: 0,
            line: 0,
            item: 0,
            item_goal: 0,
            focus: Focus::Outside,
            query: String::new(),
        };
        s.machines = apply_placement(&s.base, &s.placement);
        s.refilter();
        s
    }

    /// A cell is a real 2D grid, not a list of lists: moving DOWN keeps the column.
    #[test]
    fn down_preserves_the_column() {
        let mut s = state(vec![machine("m", 0, vec![vec!["a", "b", "c"], vec!["d", "e", "f"]])]);
        s.focus = Focus::Inside;
        s.item = 1;
        s.item_goal = 1;
        s.line = 1;
        s.clamp();
        assert_eq!(s.cell()[s.line].apps[s.item].name, "e", "line 1 item 2 -> down -> line 2 item 2");
    }

    /// Crossing a SHORT line clamps for that line only. Coming back returns to the goal column
    /// instead of drifting left -- the bug a naive `item = min(item, len-1)` produces.
    #[test]
    fn a_short_line_does_not_eat_the_goal_column() {
        let mut s = state(vec![machine("m", 0, vec![vec!["a", "b", "c"], vec!["solo"], vec!["x", "y", "z"]])]);
        s.focus = Focus::Inside;
        s.item = 2;
        s.item_goal = 2;

        s.line = 1;
        s.clamp();
        assert_eq!(s.item, 0, "clamped onto the one-item line");

        s.line = 2;
        s.clamp();
        assert_eq!(s.cell()[s.line].apps[s.item].name, "z", "goal column survived the short line");
    }

    /// The filter reaches every machine, not just the focused column.
    #[test]
    fn search_spans_all_machines() {
        let mut s = state(vec![
            machine("one", 0, vec![vec!["Firefox", "Helix"]]),
            machine("two", 0, vec![vec!["Helix"]]),
        ]);
        s.query = "fire".into();
        s.refilter();
        assert_eq!(s.view[0].cells[0].len(), 1);
        assert_eq!(s.view[0].cells[0][0].apps.len(), 1);
        assert!(s.view[1].cells[0].is_empty(), "the other machine filtered down to nothing");
    }

    /// Typing must never strand the cursor on a cell the filter emptied.
    #[test]
    fn the_cursor_snaps_to_surviving_content() {
        let mut s = state(vec![
            machine("one", 0, vec![vec!["Firefox"]]),
            machine("two", 1, vec![vec!["Kdenlive"]]),
        ]);
        s.query = "kden".into();
        s.refilter();
        assert!(!s.cell().is_empty(), "cursor moved to a cell that still has something");
        assert_eq!(s.cell()[0].apps[0].name, "Kdenlive");
    }

    /// Dropping on a cell background gives the app a line of its own, in the target folder.
    #[test]
    fn filing_into_a_folder_moves_the_app() {
        let mut s = state(vec![machine("m", 6, vec![vec!["Bottles"]])]); // 6 = Other, the inbox
        s.place_app(0, "Bottles", 4, None, 0); // 4 = Files
        assert!(s.machines[0].cells[6].is_empty(), "left the inbox");
        assert_eq!(s.machines[0].cells[4][0].apps[0].name, "Bottles");
    }

    /// Dropping ON a line inserts INTO it, at the requested position -- this is appset building.
    #[test]
    fn dropping_on_a_line_inserts_at_position() {
        let mut m = machine("m", 0, vec![vec!["a", "b", "c"]]);
        m.cells[6] = vec![Line { apps: vec![app("new")] }];
        let mut s = state(vec![m]);
        s.place_app(0, "new", 0, Some(0), 1);
        let names: Vec<&str> =
            s.machines[0].cells[0][0].apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["a", "new", "b", "c"], "landed between a and b");
    }

    /// Same call, same line: a reorder.
    #[test]
    fn reordering_within_a_line() {
        let mut s = state(vec![machine("m", 0, vec![vec!["a", "b", "c"]])]);
        s.place_app(0, "c", 0, Some(0), 0);
        let names: Vec<&str> =
            s.machines[0].cells[0][0].apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["c", "a", "b"]);
    }

    /// An app nobody arranged keeps the folder the category table computed for it, so a newly
    /// installed app appears in the inbox without any placement entry existing.
    #[test]
    fn unplaced_apps_keep_their_computed_folder() {
        let mut m = machine("m", 0, vec![vec!["Foot"]]);
        m.cells[6] = vec![Line { apps: vec![app("Brand New")] }];
        let mut s = state(vec![m]);
        s.place_app(0, "Foot", 1, None, 0); // arrange something else entirely
        assert_eq!(s.machines[0].cells[6][0].apps[0].name, "Brand New", "still in the inbox");
    }

    /// A placement naming an app that no longer exists must not corrupt its neighbours.
    #[test]
    fn a_vanished_app_is_skipped_not_fatal() {
        let base = vec![machine("m", 0, vec![vec!["a", "b"]])];
        let mut p = Placement::new();
        let mut folders = HashMap::new();
        folders.insert(FOLDERS[0].to_string(), vec![vec!["a".to_string(), "ghost".to_string(), "b".to_string()]]);
        p.insert("m".to_string(), folders);
        let out = apply_placement(&base, &p);
        let names: Vec<&str> = out[0].cells[0][0].apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"], "ghost skipped, order of the survivors kept");
    }

    /// clamp() is the only thing standing between a shrinking grid and an index panic.
    ///
    /// NOTE THE SHAPE OF THIS INVARIANT, which a first version of this test got wrong: resting on
    /// an EMPTY cell is legal and reachable on purpose. Empty cells are drawn rather than
    /// collapsed, because "archlxc has no chat client" is an answer worth seeing, and you have to
    /// be able to arrow onto one to read it. So the guarantee is not "there is always an item" --
    /// it is that indices are in range whenever there IS something to point at, and that an empty
    /// cell cannot leave you inside it.
    #[test]
    fn clamp_never_points_out_of_range() {
        let mut s = state(vec![machine("m", 0, vec![vec!["a"]])]);
        s.col = 99;
        s.row = 99;
        s.line = 99;
        s.item = 99;
        s.item_goal = 99;
        s.clamp();
        assert!(s.col < s.view.len(), "column in range");
        assert!(s.row < FOLDERS.len(), "row in range");
        if s.cell().is_empty() {
            assert_eq!(s.focus, Focus::Outside, "an empty cell must not hold you inside it");
        } else {
            assert!(s.cell().get(s.line).is_some_and(|l| l.apps.get(s.item).is_some()));
        }
    }

    /// The same guarantee where it actually bites: a NON-empty cell with wild indices.
    #[test]
    fn clamp_lands_on_a_real_item_when_one_exists() {
        let mut s = state(vec![machine("m", 0, vec![vec!["a", "b"]])]);
        s.row = 0;
        s.line = 99;
        s.item = 99;
        s.item_goal = 99;
        s.clamp();
        assert_eq!(s.cell()[s.line].apps[s.item].name, "b", "clamped to the last real item");
    }
}
