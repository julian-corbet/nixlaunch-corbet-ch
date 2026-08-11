// model.rs — everything nixlaunch knows, with no pixels in it.
//
// SPLIT ON PURPOSE. Every bug found while building this was in here and none of them were in the
// drawing: the box that was not really 2D, the goal column that drifted left, the search that
// filtered nothing, the cursor stranded on an emptied cell. All of it is ordinary data movement,
// and none of it needs a display to exercise -- so it lives apart from GTK and is covered by real
// tests at the bottom of this file rather than by opening the launcher and squinting.
//
// The rule that keeps it that way: nothing in this file may import gtk.
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use crate::usage::{self, Usage};
use serde_json;
use std::collections::HashMap;
use std::path::PathBuf;

// ── the model ───────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct App {
    /// WHAT THIS APP IS, as opposed to what it calls itself. Placement and usage are both keyed on
    /// this and never on `name`, because a display name is not an identity: it is translated, it is
    /// upstream's choice rather than the packager's, and two entries sharing one is ordinary.
    /// Keyed on the name, two such apps collapsed into a single row and whichever lost became
    /// unlaunchable -- and the arrangement of one silently drove the other.
    pub id: String,
    pub name: String,
    pub icon: String,
    /// The `Exec` line as the inventory reported it, field codes and all. Kept verbatim rather
    /// than pre-split: splitting is the launcher's job and the raw string is what a future
    /// provider will keep giving us.
    pub exec: String,
    /// `Terminal=true` in the desktop entry: this program draws no window of its own and MUST be
    /// given one. Ignoring it is not a degraded launch, it is no launch at all -- the process
    /// starts with no controlling terminal and dies immediately, silently.
    pub terminal: bool,
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
    /// argv prefix that turns "run this" into "run this THERE". Empty is a read-only column.
    pub launch: Vec<String>,
    /// Why this machine could not be asked, if it could not. Carried rather than raised: an
    /// unreachable peer is a normal state on a roaming laptop, and a column drawn with a reason on
    /// it is honest, where an empty column is indistinguishable from a machine that has nothing.
    pub error: Option<String>,
    /// Parallel to the row set: cells[r] is this machine's lines for folder r. An empty cell is a
    /// real and common state worth DRAWING rather than collapsing -- it tells you "that machine has no
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
/// The fallback row set, used only when there is no config to read one from -- a bare checkout
/// with no fleet still runs. The REAL row set is `State::folders`, supplied by config, because the
/// grouping table is a per-estate value and hardcoding it here would make this repo carry one
/// operator's taxonomy as though it were a fact about launchers.
pub const DEFAULT_FOLDERS: &[&str] =
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

/// A MISSING file is an empty arrangement; a file that exists and does not parse is not, and the
/// difference matters because the next drag rewrites whatever we decide it was. Say so rather than
/// silently starting from nothing and overwriting the user's real arrangement with the assumption.
pub fn load_placement() -> (Placement, Option<String>) {
    let path = placement_path();
    match std::fs::read_to_string(&path) {
        Err(_) => (Placement::new(), None),
        Ok(text) => match serde_json::from_str(&text) {
            Ok(p) => (p, None),
            Err(e) => (Placement::new(), Some(format!("{}: {e}", path.display()))),
        },
    }
}

/// Best-effort about FAILING, never about corrupting. A launcher that refused to open because it
/// could not write its placement file would trade the whole feature for one of its conveniences --
/// but a half-written file is worse than no file, because it parses as an empty arrangement and
/// the next drag makes that permanent. Write beside it and rename, which is atomic on any POSIX
/// filesystem, so a reader sees the old file or the new one and never a truncated one.
pub fn save_placement(p: &Placement) {
    // NEVER FROM A TEST. `place_app` saves as part of doing its job, so every placement test was
    // writing the fixture arrangement -- machine "m", apps "Foot" and "Brand New" -- straight over
    // the real file in the developer's own state directory. Inside the Nix sandbox HOME is a
    // throwaway and it went unnoticed; anyone running `cargo test` on their own machine lost their
    // arrangement to it, silently, and this is how one such file was found.
    //
    // A cfg guard rather than a redirected path: tests share one process, so pointing
    // XDG_STATE_HOME somewhere else would race between parallel test threads and leave the
    // protection depending on which test ran first.
    if cfg!(test) {
        return;
    }
    write_atomic(&placement_path(), p);
}

pub fn write_atomic<T: serde::Serialize>(path: &std::path::Path, value: &T) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let Ok(text) = serde_json::to_string_pretty(value) else { return };
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

#[derive(PartialEq, Clone, Copy, Debug)]
pub enum Focus {
    Outside,
    Inside,
}

pub struct State {
    /// The row set as CONFIG declared it, and the order `base`'s cells are indexed by. Never
    /// permuted -- `folders` below is re-derived from this on every rebuild.
    ///
    /// Keeping these apart is not tidiness. When usage ordering permuted the live row list in
    /// place, the next rebuild's unplaced pass indexed base's ORIGINAL row numbers into a cells
    /// vector whose positions had moved, so untouched machines silently swapped rows -- and the
    /// next drag snapshotted that corruption into placement.json, permanently.
    pub canonical_folders: Vec<String>,
    /// The row set as DISPLAYED, after usage ordering. Rebuilt from `canonical_folders` each time.
    pub folders: Vec<String>,
    /// How often each thing is reached for. See usage.rs.
    pub usage: Usage,
    /// Standard errors an item must lead by before it may move. See `significantly_greater`.
    pub z: f64,
    pub half_life_days: f64,
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

/// Used when there are no machines at all, so `cell()` can answer without indexing into nothing.
/// A config with an empty machine list is a shape the module can legitimately render (every
/// machine toggled off), and it must open empty rather than panic on startup.
static NO_LINES: Vec<Line> = Vec::new();

impl State {
    pub fn cell(&self) -> &Vec<Line> {
        match self.view.get(self.col).and_then(|m| m.cells.get(self.row)) {
            Some(c) => c,
            None => &NO_LINES,
        }
    }

    pub fn current_line(&self) -> Option<&Line> {
        self.cell().get(self.line)
    }

    /// Test-only: bump a score without writing to disk.
    #[cfg(test)]
    pub fn record_launch_for_test(&mut self, machine: &str, app: &str) {
        usage::record(&mut self.usage, machine, app, usage::now_secs(), self.half_life_days);
    }

    /// Record a launch, and persist it.
    pub fn record_launch(&mut self, machine: &str, app: &str) {
        usage::record(&mut self.usage, machine, app, usage::now_secs(), self.half_life_days);
        usage::save(&self.usage);
    }

    /// Rewrite state written when placement and usage were keyed on the DISPLAY NAME.
    ///
    /// Changing a key without migrating is the same thing as deleting the file: every entry stops
    /// matching, the grid falls back to the computed grouping, and the user's arrangement is gone
    /// with no error and nothing to restore it from. So the change and the migration ship together.
    ///
    /// The test for "is this token an old one" is deliberately not a guess about its shape -- an id
    /// is opaque by contract, and "it ends in .desktop" is one provider's habit rather than a rule.
    /// A token is old when it is NOT a known id AND IS a known name. That is self-limiting: after
    /// the rewrite nothing matches it any more, so running it again is a no-op, and a token that is
    /// neither (an app since uninstalled) is left exactly where it is, because those entries are
    /// carried on purpose -- see `materialise`.
    ///
    /// AMBIGUITY RESOLVES TO NOTHING. If two apps share a display name, an old entry naming it
    /// cannot say which was meant, and picking one would silently hand the other's arrangement to
    /// the wrong app. The entry is left alone instead: the apps then appear in their computed
    /// folders, which is visible and fixable with a drag, where a wrong guess is neither.
    pub fn migrate_names_to_ids(&mut self) {
        let mut touched = false;
        for m in &self.base {
            let mut ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
            let mut by_name: HashMap<&str, Vec<&str>> = HashMap::new();
            for lines in &m.cells {
                for l in lines {
                    for a in &l.apps {
                        ids.insert(a.id.as_str());
                        by_name.entry(a.name.as_str()).or_default().push(a.id.as_str());
                    }
                }
            }
            let rename = |token: &String| -> Option<String> {
                if ids.contains(token.as_str()) {
                    return None;
                }
                match by_name.get(token.as_str()) {
                    Some(candidates) if candidates.len() == 1 => Some(candidates[0].to_string()),
                    _ => None,
                }
            };

            if let Some(folders) = self.placement.get_mut(&m.name) {
                for lines in folders.values_mut() {
                    for l in lines.iter_mut() {
                        for token in l.iter_mut() {
                            if let Some(id) = rename(token) {
                                *token = id;
                                touched = true;
                            }
                        }
                    }
                }
            }

            // Usage keys are "<machine>\u{1}<app>", so the same rewrite applies to the second half.
            let prefix = format!("{}\u{1}", m.name);
            let moves: Vec<(String, String)> = self
                .usage
                .keys()
                .filter_map(|k| {
                    let app = k.strip_prefix(&prefix)?;
                    rename(&app.to_string()).map(|id| (k.clone(), usage::key(&m.name, &id)))
                })
                .collect();
            for (from, to) in moves {
                if let Some(entry) = self.usage.remove(&from) {
                    self.usage.insert(to, entry);
                    touched = true;
                }
            }
        }

        // Written once, so the next start has nothing to do. Skipped entirely when nothing moved,
        // which is every start after the first -- a migration that rewrote both state files on
        // every launch would be a needless write on the path this program is judged by.
        if touched {
            save_placement(&self.placement);
            usage::save(&self.usage);
        }
    }

    /// Re-derive the grid from the pristine inventory plus the user's filings, then re-filter.
    /// Called on startup and after every drop, so there is exactly one path from
    /// (inventory, placement) to what is on screen.
    pub fn rebuild(&mut self) {
        // Always from canonical: apply_placement indexes base's cells, which are in canonical
        // order, and a permuted row list here is what desynchronised the two.
        self.folders = self.canonical_folders.clone();
        self.machines = apply_placement(&self.base, &self.placement, &self.canonical_folders);
        // Placement decides MEMBERSHIP -- which folder, which line. Usage decides ORDER within
        // that, and only where the evidence justifies a move. Running it here rather than baking
        // it into the placement file keeps the two separable: the file stays a record of what the
        // user arranged, never of what the statistics did to it.
        apply_usage(
            &mut self.machines,
            &mut self.folders,
            &self.usage,
            usage::now_secs(),
            self.z,
            self.half_life_days,
        );
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
        // FROM THE PLACEMENT GRID, NOT THE RENDERED ONE, and the difference is the whole reason
        // this is not simply `&self.machines[mi]`.
        //
        // `self.machines` has been through `apply_usage`, so its lines and rows are in FRECENCY
        // order. Snapshotting that would write the statistics' opinion into placement.json as
        // though the user had arranged it -- and once written it is indistinguishable from a real
        // arrangement, so it never decays and never moves again. Two drags in, the file is a
        // fossil of whatever the scores happened to say at the moment of the first one.
        //
        // `rebuild`'s own comment promises exactly the opposite: placement decides membership,
        // usage decides order, and the file "stays a record of what the user arranged, never of
        // what the statistics did to it". This recomputes the placement-only grid so that promise
        // is true rather than merely stated.
        let placed = apply_placement(&self.base, &self.placement, &self.canonical_folders);
        let m = &placed[mi];
        let mut folders: HashMap<String, Vec<Vec<String>>> = HashMap::new();
        for (r, lines) in m.cells.iter().enumerate() {
            if lines.is_empty() {
                continue;
            }
            folders.insert(
                // Canonical, matching the grid this was read from. `self.folders` is the RENDERED
                // row order, which usage may have permuted -- pairing it with canonical cells would
                // file every row's contents under a neighbouring row's name.
                self.canonical_folders[r].clone(),
                lines.iter().map(|l| l.apps.iter().map(|a| a.id.clone()).collect()).collect(),
            );
        }
        // MERGE, never replace. An app named in the placement but absent from today's inventory
        // has no cell to be snapshotted from, so a wholesale insert would drop it -- silently
        // discarding the arrangement of everything that happened to be missing (uninstalled, or a
        // backend that timed out and returned a partial list) the moment the user dragged anything
        // else. Entries for apps we cannot currently see are carried through untouched, which is
        // what makes "uninstall it and it returns to where you put it" true.
        let seen: std::collections::HashSet<String> = folders
            .values()
            .flatten()
            .flatten()
            .cloned()
            .collect();
        let previous = self.placement.get(&m.name).cloned().unwrap_or_default();
        for (folder, lines) in previous {
            let kept: Vec<Vec<String>> = lines
                .into_iter()
                .map(|l| l.into_iter().filter(|n| !seen.contains(n)).collect::<Vec<_>>())
                .filter(|l: &Vec<String>| !l.is_empty())
                .collect();
            if !kept.is_empty() {
                folders.entry(folder).or_default().extend(kept);
            }
        }
        self.placement.insert(m.name.clone(), folders);
    }

    /// Move `app` on machine `mi` into `folder`, joining the line that `join` names, immediately
    /// before the app that `before` names. This one call covers filing into a folder, joining an
    /// appset at a chosen position, and reordering within a line -- they differ only in where the
    /// drop landed, which is the point of recording arrangements rather than folders.
    ///
    /// ── EVERYTHING HERE IS NAMED, NOTHING IS NUMBERED, AND THAT IS THE FIX ────────────────────
    ///
    /// This used to take a line INDEX and a position INDEX, read straight off the rendered grid,
    /// and it was wrong in two independent ways at once:
    ///
    ///   * The rendered grid is FILTERED. With a query active, "the second line" on screen might
    ///     be the fifth line in the placement -- every line whose apps all failed to match is
    ///     simply absent. Dropping onto a visible appset therefore joined a different one, and the
    ///     user had no way to see which.
    ///   * The rendered grid is USAGE-ORDERED. `apply_usage` permutes lines by frecency, so even
    ///     with no query the on-screen order is not the stored order once anything has been
    ///     launched a few times.
    ///
    /// An index is only meaningful in the space it was computed in, and the drop happened in a
    /// space that is two transformations away from the one being written to. Names survive both
    /// transformations, because filtering and reordering move apps around without renaming them.
    ///
    /// `join` is the rendered line's membership rather than one chosen app: the first name on it
    /// may BE the app being dragged, which is gone by the time the target is looked up, so the
    /// anchor has to be picked as one that will still be there.
    pub fn place_app(
        &mut self,
        mi: usize,
        app: &str,
        folder: usize,
        join: Option<&[String]>,
        before: Option<&str>,
    ) {
        // Dropping something into the gap it already occupies is not a move. Worth an early return
        // rather than letting it fall through: after the removal below, "insert before `app`"
        // names a position that no longer exists, and the fallback for an unfindable neighbour is
        // "append to the end" -- so the no-op would silently become a jump to the far right.
        if before == Some(app) {
            return;
        }

        self.materialise(mi);
        let machine = self.machines[mi].name.clone();
        let target_label = self.folders[folder].clone();
        // An anchor that survives the removal: any name on the target line that is not the one
        // being moved. A line holding only the dragged app has none, and correctly yields None --
        // moving an app out of its own single-app line and back onto it is a no-op either way.
        let anchor = join.and_then(|names| names.iter().find(|n| n.as_str() != app).cloned());
        let folders = self.placement.entry(machine).or_default();

        // Remove it from wherever it currently is, and drop any line that empties as a result.
        for lines in folders.values_mut() {
            for l in lines.iter_mut() {
                l.retain(|n| n != app);
            }
            lines.retain(|l| !l.is_empty());
        }

        // Find the target AFTER the removal, never before: taking the app out can empty a line and
        // delete it, which shifts every later line down one. A target located beforehand would
        // then name the wrong line -- the same class of bug as the rendered indices this replaced.
        let target_line = anchor.as_ref().and_then(|a| {
            folders
                .get(&target_label)
                .and_then(|ls| ls.iter().position(|l| l.iter().any(|n| n == a)))
        });

        let lines = folders.entry(target_label).or_default();
        match target_line {
            Some(li) => {
                // An unfindable neighbour appends rather than panicking. It happens legitimately:
                // `before` names what was rendered next, and that app may be filtered out of the
                // stored line, or on a different one entirely.
                let at = before
                    .and_then(|b| lines[li].iter().position(|n| n == b))
                    .unwrap_or(lines[li].len());
                lines[li].insert(at, app.to_string());
            }
            // No anchor, or the line it named is gone: give the app a line of its own, which is
            // also what dropping on a cell's background means.
            None => lines.push(vec![app.to_string()]),
        }

        save_placement(&self.placement);
        self.rebuild();
        // The drop can shrink the line the cursor was on. Nothing else in this path re-checks it.
        self.clamp();
    }

    /// Change the query, and put the cursor back at the top of what is now on screen.
    ///
    /// SEPARATE FROM `refilter`, and the distinction is the bug this fixes. `refilter` runs for two
    /// unrelated reasons -- the query changed, or the grid was rebuilt after a drag -- and only one
    /// of them should move the cursor. Resetting inside `refilter` would drag the selection away
    /// from the app the user just dropped; not resetting at all leaves the goal column pointing at
    /// a position that now holds a different app.
    ///
    /// `item_goal` is the column that up/down preserves, and it is only meaningful while a line's
    /// membership is stable. A query rewrites that membership, so the goal is stale by definition:
    /// keeping it selects the Nth surviving app rather than the first match, and the first match is
    /// the one the user is looking at while they type. `snap_to_content` already reasoned this out
    /// for the case where the cell empties entirely; this is the same rule for the case where it
    /// does not.
    pub fn set_query(&mut self, q: String) {
        self.query = q;
        self.line = 0;
        self.item = 0;
        self.item_goal = 0;
        self.refilter();
    }

    /// FUZZY, not substring, and not hand-rolled: `nucleo`, the matcher Helix uses. A substring
    /// filter is the obvious thing to write and the wrong thing to ship -- typing "e" matched
    /// almost every application here, because nearly every name contains one. Fuzzy matching with
    /// a real score is what makes "cod" find "Code - OSS" and "gimp" not match "Manage Printing",
    /// and it is a solved problem with a maintained crate behind it.
    ///
    /// Applied to EVERY cell at once -- the whole point of a matrix is seeing "where does this
    /// thing exist" across machines, and a filter that only searched the current column would
    /// throw that away. Lines that lose every app disappear; a cell that loses every line renders
    /// as empty, which is itself the answer to "does that machine have this?".
    pub fn refilter(&mut self) {
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = (!self.query.is_empty())
            .then(|| Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart));
        let mut buf = Vec::new();
        let mut keep = |name: &str| -> bool {
            match &pattern {
                None => true,
                Some(p) => p.score(Utf32Str::new(name, &mut buf), &mut matcher).is_some(),
            }
        };
        self.view = self
            .machines
            .iter()
            .map(|m| Machine {
                name: m.name.clone(),
                accent: m.accent.clone(),
                launch: m.launch.clone(),
                error: m.error.clone(),
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
                                    .filter(|a| keep(&a.name))
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
        if self.view.is_empty() || !self.cell().is_empty() {
            return;
        }
        for r in 0..self.folders.len() {
            for c in 0..self.view.len() {
                if !self.view[c].cells[r].is_empty() {
                    self.row = r;
                    self.col = c;
                    self.line = 0;
                    self.item = 0;
                    // The goal column belongs to the cell it was chosen in. Carrying it across a
                    // snap selects the Nth item of an unrelated cell rather than the first match,
                    // which for a search result is the one thing the user is looking at.
                    self.item_goal = 0;
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
        self.row = self.row.min(self.folders.len().saturating_sub(1));
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
pub fn apply_placement(base: &[Machine], p: &Placement, folders: &[String]) -> Vec<Machine> {
    base.iter()
        .map(|m| {
            let mut by_id: HashMap<&str, &App> = HashMap::new();
            for lines in &m.cells {
                for l in lines {
                    for app in &l.apps {
                        by_id.insert(app.id.as_str(), app);
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

            let mut cells: Vec<Vec<Line>> = vec![Vec::new(); folders.len()];

            if let Some(arranged_folders) = arranged {
                for (fi, fname) in folders.iter().enumerate() {
                    let Some(lines) = arranged_folders.get(fname.as_str()) else { continue };
                    for l in lines {
                        let apps: Vec<App> =
                            l.iter().filter_map(|n| by_id.get(n.as_str()).map(|a| (*a).clone())).collect();
                        if !apps.is_empty() {
                            cells[fi].push(Line { apps });
                        }
                    }
                }
            }

            for (r, lines) in m.cells.iter().enumerate() {
                for l in lines {
                    let apps: Vec<App> =
                        l.apps.iter().filter(|a| !placed.contains(a.id.as_str())).cloned().collect();
                    if !apps.is_empty() {
                        cells[r].push(Line { apps });
                    }
                }
            }

            Machine { name: m.name.clone(), accent: m.accent.clone(), launch: m.launch.clone(), error: m.error.clone(), cells }
        })
        .collect()
}

/// Build the argv that actually starts `app` on `machine`.
///
/// FIELD CODES ARE STRIPPED. A `.desktop` Exec carries placeholders the spec defines -- %f %F %u
/// %U for files and URLs, %i %c %k for icon/name/path -- which are meant to be substituted by
/// whoever launches with arguments. We launch with none, and the spec is explicit that unhandled
/// codes must be REMOVED rather than passed through: leave "%U" in place and Firefox opens a tab
/// for a file literally named "%U". Handled inline rather than by pulling in a whole .desktop
/// parser, because this program never parses .desktop files at all -- the inventory does.
///
/// Splitting is `shlex`, not `split_whitespace`: an Exec may legitimately quote an argument
/// containing spaces, and naive splitting turns one path into two broken ones.
pub fn launch_argv(machine: &Machine, app: &App, terminal: &[String]) -> Vec<String> {
    // SPLIT FIRST, then drop field codes. Stripping on whitespace beforehand reached INSIDE
    // quoted arguments and collapsed runs of spaces, so `prog "a  b"` became `prog "a b"` -- a
    // path silently altered on its way to exec.
    let tokens: Vec<String> = shlex::split(&app.exec).unwrap_or_else(|| vec![app.exec.clone()]);
    let tokens: Vec<String> = tokens
        .into_iter()
        .filter(|t| !(t.len() == 2 && t.starts_with('%')))
        .collect();

    let mut argv = machine.launch.clone();
    // ORDER MATTERS, and it is the non-obvious part: the machine prefix comes FIRST, then the
    // terminal, then the program. A terminal emulator for a remote program has to run on the
    // remote machine -- `<forward> foot -e helix`, never `foot -e <forward> helix`, which would
    // open a local window and then try to forward from inside it.
    if app.terminal {
        argv.extend(terminal.iter().cloned());
    }
    // `{}` is a PLACEHOLDER when present, a prefix when absent. config.rs documented the
    // placeholder and nothing ever substituted it, so a launch template containing one passed the
    // two braces to exec as a literal argument.
    if argv.iter().any(|a| a == "{}") {
        let mut out = Vec::new();
        for a in argv {
            if a == "{}" {
                out.extend(tokens.iter().cloned());
            } else {
                out.push(a);
            }
        }
        return out;
    }
    argv.extend(tokens);
    argv
}

/// Which GAP between a line's items a drop at `x` belongs to.
///
/// Compared against each child's MIDPOINT, not its edges, so the target flips when the pointer
/// passes the middle of an item -- the behaviour every reorderable list has, and the reason a drop

/// Order the grid by how often things are actually used -- WITHOUT sorting it.
///
/// Three levels, all through the same significance gate, so nothing moves on noise:
///   * items on a line, by their own score;
///   * lines within a cell, by the sum of their apps -- an appset you start often should rise as
///     a unit, since starting the line is one action;
///   * the rows themselves, by the aggregate across every machine, because a row is one row for
///     the whole grid and ordering it per-column is not a thing the layout can express.
///
/// The inbox is PINNED LAST and never participates. It is not a category competing for position;
/// it is where uncategorised things wait, and a busy inbox floating to the top would bury the
/// folders the user actually organised.
pub fn apply_usage(
    machines: &mut [Machine],
    folders: &mut [String],
    u: &Usage,
    now: u64,
    z: f64,
    hl: f64,
) {
    for m in machines.iter_mut() {
        let name = m.name.clone();
        for cell in m.cells.iter_mut() {
            for line in cell.iter_mut() {
                usage::reorder_stable(&mut line.apps, |a| usage::score_of(u, &name, &a.id, now, hl), z);
            }
            usage::reorder_stable(
                cell,
                |l| l.apps.iter().map(|a| usage::score_of(u, &name, &a.id, now, hl)).sum::<f64>(),
                z,
            );
        }
    }

    // Rows. Everything but the inbox, which stays where it is.
    let n = folders.len();
    if n < 3 {
        return;
    }
    let inbox = n - 1;
    let row_score = |r: usize| -> f64 {
        machines
            .iter()
            .map(|m| {
                m.cells
                    .get(r)
                    .map(|cell| {
                        cell.iter()
                            .flat_map(|l| l.apps.iter())
                            .map(|a| usage::score_of(u, &m.name, &a.id, now, hl))
                            .sum::<f64>()
                    })
                    .unwrap_or(0.0)
            })
            .sum()
    };

    let mut order: Vec<usize> = (0..inbox).collect();
    usage::reorder_stable(&mut order, |r| row_score(*r), z);
    order.push(inbox);

    // Apply the permutation to the row labels and to every machine's cells together -- they are
    // parallel by contract, and permuting one without the other silently mislabels the whole grid.
    let new_folders: Vec<String> = order.iter().map(|&r| folders[r].clone()).collect();
    for (i, f) in new_folders.into_iter().enumerate() {
        folders[i] = f;
    }
    for m in machines.iter_mut() {
        let cells: Vec<Vec<Line>> =
            order.iter().map(|&r| m.cells.get(r).cloned().unwrap_or_default()).collect();
        m.cells = cells;
    }
}

pub fn a(name: &str, icon: &str) -> App {
    App { id: name.into(), name: name.into(), icon: icon.into(), exec: name.to_lowercase(), terminal: false }
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
            name: "laptop".into(),
            accent: "#166534".into(),
            launch: vec![],
            error: None,
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
            name: "workstation".into(),
            accent: "#B45309".into(),
            launch: vec![],
            error: None,
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
            name: "console".into(),
            accent: "#9F1239".into(),
            launch: vec![],
            error: None,
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

    /// A rendered line's membership, as `place_app` takes it. Tests name the line by what is on
    /// it for the same reason the drop handler does -- an index would not survive a filter.
    fn on(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn app(n: &str) -> App {
        App { id: n.into(), name: n.into(), icon: "x".into(), exec: n.to_lowercase(), terminal: false }
    }

    /// One machine, one folder, with the given lines. `folder` indexes DEFAULT_FOLDERS.
    fn machine(name: &str, folder: usize, lines: Vec<Vec<&str>>) -> Machine {
        let mut cells: Vec<Vec<Line>> = vec![Vec::new(); DEFAULT_FOLDERS.len()];
        cells[folder] =
            lines.into_iter().map(|l| Line { apps: l.into_iter().map(app).collect() }).collect();
        Machine { name: name.into(), accent: "#fff".into(), launch: vec![], error: None, cells }
    }

    fn state(machines: Vec<Machine>) -> State {
        let mut s = State {
            canonical_folders: DEFAULT_FOLDERS.iter().map(|f| f.to_string()).collect(),
            folders: DEFAULT_FOLDERS.iter().map(|f| f.to_string()).collect(),
            // No usage in the model tests: they are about placement and navigation, and a live
            // reordering pass would make their expectations depend on statistics they are not
            // testing. usage.rs owns those, with its own suite.
            usage: Usage::new(),
            z: 2.0,
            half_life_days: crate::usage::HALF_LIFE_DAYS,
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
        s.machines = apply_placement(&s.base, &s.placement, &s.folders);
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

    /// Fuzzy, not substring: gaps are allowed, so an acronym-ish prefix finds the real name.
    #[test]
    fn search_matches_fuzzily() {
        let mut s = state(vec![machine("m", 0, vec![vec!["Code - OSS", "Manage Printing"]])]);
        s.query = "cod".into();
        s.refilter();
        let hits: Vec<&str> =
            s.view[0].cells[0].iter().flat_map(|l| l.apps.iter().map(|a| a.name.as_str())).collect();
        assert_eq!(hits, vec!["Code - OSS"], "matched across the gap, and did not match the other");
    }

    /// The regression that motivated dropping substring matching: a single common letter must not
    /// match nearly everything, which is exactly what `contains` did.
    #[test]
    fn a_single_letter_is_not_a_wildcard() {
        let mut s = state(vec![machine("m", 0, vec![vec!["Zed", "Krita"]])]);
        s.query = "zd".into();
        s.refilter();
        let hits: Vec<&str> =
            s.view[0].cells[0].iter().flat_map(|l| l.apps.iter().map(|a| a.name.as_str())).collect();
        assert_eq!(hits, vec!["Zed"]);
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
        s.place_app(0, "Bottles", 4, None, None); // 4 = Files
        assert!(s.machines[0].cells[6].is_empty(), "left the inbox");
        assert_eq!(s.machines[0].cells[4][0].apps[0].name, "Bottles");
    }

    /// Dropping ON a line inserts INTO it, at the requested position -- this is appset building.
    #[test]
    fn dropping_on_a_line_inserts_at_position() {
        let mut m = machine("m", 0, vec![vec!["a", "b", "c"]]);
        m.cells[6] = vec![Line { apps: vec![app("new")] }];
        let mut s = state(vec![m]);
        s.place_app(0, "new", 0, Some(&on(&["a", "b", "c"])), Some("b"));
        let names: Vec<&str> =
            s.machines[0].cells[0][0].apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["a", "new", "b", "c"], "landed between a and b");
    }

    /// Same call, same line: a reorder.
    #[test]
    fn reordering_within_a_line() {
        let mut s = state(vec![machine("m", 0, vec![vec!["a", "b", "c"]])]);
        s.place_app(0, "c", 0, Some(&on(&["a", "b", "c"])), Some("a"));
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
        s.place_app(0, "Foot", 1, None, None); // arrange something else entirely
        assert_eq!(s.machines[0].cells[6][0].apps[0].name, "Brand New", "still in the inbox");
    }

    /// A placement naming an app that no longer exists must not corrupt its neighbours.
    #[test]
    fn a_vanished_app_is_skipped_not_fatal() {
        let base = vec![machine("m", 0, vec![vec!["a", "b"]])];
        let mut p = Placement::new();
        let mut folders = HashMap::new();
        folders.insert(DEFAULT_FOLDERS[0].to_string(), vec![vec!["a".to_string(), "ghost".to_string(), "b".to_string()]]);
        p.insert("m".to_string(), folders);
        let rows: Vec<String> = DEFAULT_FOLDERS.iter().map(|f| f.to_string()).collect();
        let out = apply_placement(&base, &p, &rows);
        let names: Vec<&str> = out[0].cells[0][0].apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"], "ghost skipped, order of the survivors kept");
    }

    fn appx(name: &str, exec: &str) -> App {
        App { id: name.into(), name: name.into(), icon: "x".into(), exec: exec.into(), terminal: false }
    }

    /// Unhandled field codes must be REMOVED, not passed through. Left in place, "%U" becomes a
    /// literal argument and the browser opens a tab for a file called "%U".
    #[test]
    fn field_codes_are_stripped() {
        let m = machine("m", 0, vec![vec!["x"]]);
        assert_eq!(launch_argv(&m, &appx("Firefox", "firefox %u"), &[]), vec!["firefox"]);
        assert_eq!(launch_argv(&m, &appx("Files", "thunar %F"), &[]), vec!["thunar"]);
        assert_eq!(
            launch_argv(&m, &appx("Foot", "foot --server %i"), &[]),
            vec!["foot", "--server"],
            "only the codes go, the real flags stay"
        );
    }

    /// A percent sign that is not a field code is an ordinary argument.
    #[test]
    fn a_bare_percent_is_not_a_field_code() {
        let m = machine("m", 0, vec![vec!["x"]]);
        assert_eq!(launch_argv(&m, &appx("odd", "prog 100%"), &[]), vec!["prog", "100%"]);
    }

    /// Splitting is shlex, so a quoted argument containing spaces stays ONE argument. Naive
    /// whitespace splitting turns one path into two broken ones.
    #[test]
    fn quoted_arguments_survive_splitting() {
        let m = machine("m", 0, vec![vec!["x"]]);
        assert_eq!(
            launch_argv(&m, &appx("q", "prog --path \"/a b/c\" --flag"), &[]),
            vec!["prog", "--path", "/a b/c", "--flag"]
        );
    }

    /// A Terminal=true program is wrapped, and the ORDER is the part worth pinning: machine
    /// prefix, then terminal, then program. The other order opens a local window and tries to
    /// forward from inside it.
    #[test]
    fn a_terminal_program_is_wrapped_in_the_right_order() {
        let mut m = machine("remote", 0, vec![vec!["x"]]);
        m.launch = vec!["fwd@remote".to_string()];
        let mut app = appx("Helix", "helix %F");
        app.terminal = true;
        assert_eq!(
            launch_argv(&m, &app, &["foot".to_string(), "-e".to_string()]),
            vec!["fwd@remote", "foot", "-e", "helix"]
        );
    }

    /// A graphical program must NOT be wrapped even when a terminal is configured.
    #[test]
    fn a_graphical_program_is_not_wrapped() {
        let m = machine("m", 0, vec![vec!["x"]]);
        assert_eq!(
            launch_argv(&m, &appx("Firefox", "firefox %u"), &["foot".to_string(), "-e".to_string()]),
            vec!["firefox"]
        );
    }

    /// The machine's prefix is what turns "run this" into "run this THERE".
    #[test]
    fn the_launch_prefix_leads() {
        let mut m = machine("remote", 0, vec![vec!["x"]]);
        m.launch = vec!["waypipe@remote".to_string()];
        assert_eq!(
            launch_argv(&m, &appx("Helix", "helix %F"), &[]),
            vec!["waypipe@remote", "helix"]
        );
    }

    /// THE REGRESSION THAT MOTIVATED canonical_folders. Usage ordering permuted the live row list
    /// in place; the next rebuild's unplaced pass then indexed base's ORIGINAL row numbers into a
    /// cells vector whose positions had moved, so a machine the user never touched silently
    /// swapped rows -- and the next drag snapshotted that into placement.json permanently.
    ///
    /// The invariant: rebuilding twice must produce the same grid as rebuilding once.
    #[test]
    fn rebuilding_is_stable_under_row_reordering() {
        let mut s = state(vec![machine("m", 0, vec![vec!["Foot"]])]);
        // Make a later row decisively outrank row 0, so a permutation really happens.
        s.base[0].cells[1] = vec![Line { apps: vec![app("Helix")] }];
        for _ in 0..80 {
            s.record_launch_for_test("m", "Helix");
        }
        s.rebuild();
        let first: Vec<(String, Vec<String>)> = snapshot(&s);
        s.rebuild();
        s.rebuild();
        assert_eq!(snapshot(&s), first, "the grid must not drift on repeated rebuilds");
    }

    fn snapshot(s: &State) -> Vec<(String, Vec<String>)> {
        s.folders
            .iter()
            .enumerate()
            .map(|(r, f)| {
                (
                    f.clone(),
                    s.machines[0].cells[r]
                        .iter()
                        .flat_map(|l| l.apps.iter().map(|a| a.name.clone()))
                        .collect(),
                )
            })
            .collect()
    }

    /// An empty machine list is a shape the config module can legitimately render -- every machine
    /// toggled off -- and must open empty rather than panic on the first cursor read.
    #[test]
    fn no_machines_does_not_panic() {
        let mut s = state(vec![]);
        s.rebuild();
        assert!(s.cell().is_empty());
        s.clamp();
    }

    /// An app named in the placement but absent from today's inventory must survive a drag
    /// elsewhere -- otherwise uninstalling something, or one partial inventory, silently discards
    /// the arrangement of everything that was missing at that moment.
    #[test]
    fn materialise_keeps_entries_for_apps_it_cannot_see() {
        let mut s = state(vec![machine("m", 0, vec![vec!["here"]])]);
        let mut folders = HashMap::new();
        folders.insert("Media".to_string(), vec![vec!["gone".to_string()]]);
        s.placement.insert("m".to_string(), folders);
        s.rebuild();

        s.place_app(0, "here", 1, None, None);

        let after = &s.placement["m"];
        assert!(after.contains_key("Media"), "the absent app's folder survived: {after:?}");
        assert_eq!(after["Media"], vec![vec!["gone".to_string()]]);
    }

    /// Dragging rightward within a line must land where the pointer was, not one place further.
    ///
    /// This used to need explicit correction: with a numeric position, removing the dragged item
    /// first shifted every later gap down one, so a rightward drag overshot its neighbour. Naming
    /// the neighbour instead makes the arithmetic disappear rather than get fixed -- "before c"
    /// means the same thing whether or not anything left the line in between.
    #[test]
    fn rightward_reorder_lands_where_it_was_dropped() {
        let mut s = state(vec![machine("m", 0, vec![vec!["a", "b", "c", "d"]])]);
        s.place_app(0, "a", 0, Some(&on(&["a", "b", "c", "d"])), Some("c")); // the gap after b
        let names: Vec<&str> =
            s.machines[0].cells[0][0].apps.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["b", "a", "c", "d"], "landed after b, not after c");
    }

    /// A drop while a QUERY is active must join the appset the user is looking at.
    ///
    /// The rendered grid is filtered, so a line's position on screen is not its position in the
    /// placement: here the only visible line in the folder is the placement's SECOND one, because
    /// the first matched nothing. Addressing it by rendered index joined the wrong appset -- and
    /// silently, since the line it actually joined was the one hidden by the filter.
    #[test]
    fn a_drop_under_a_filter_joins_the_line_that_is_visible() {
        let mut m = machine("m", 0, vec![vec!["alpha", "beta"], vec!["gamma"]]);
        m.cells[6] = vec![Line { apps: vec![app("delta")] }];
        let mut s = state(vec![m]);

        // Only the gamma line survives this: on screen it is line 0, in the placement it is line 1.
        s.query = "gamma".into();
        s.refilter();
        assert_eq!(s.machines[0].cells[0].len(), 2, "the model still holds both lines");

        s.place_app(0, "delta", 0, Some(&on(&["gamma"])), None);

        let lines = &s.placement["m"]["Terminals"];
        let with_gamma = lines
            .iter()
            .find(|l| l.contains(&"gamma".to_string()))
            .expect("the gamma line is still there");
        assert!(with_gamma.contains(&"delta".to_string()), "joined the visible appset: {lines:?}");
        let with_alpha = lines.iter().find(|l| l.contains(&"alpha".to_string())).unwrap();
        assert!(
            !with_alpha.contains(&"delta".to_string()),
            "did NOT join the filtered-away one: {lines:?}"
        );
    }

    /// Frecency must never be written into the arrangement.
    ///
    /// `rebuild` orders the rendered grid by usage; `materialise` freezes the grid into
    /// placement.json before a move. Snapshotting the RENDERED grid therefore recorded the
    /// statistics' order as though the user had chosen it -- and once written it is
    /// indistinguishable from a real arrangement, so it stops decaying and never moves again.
    #[test]
    fn a_drag_does_not_fossilise_the_frecency_order() {
        let mut m = machine("m", 0, vec![vec!["rare"], vec!["popular"]]);
        m.cells[6] = vec![Line { apps: vec![app("unrelated")] }];
        let mut s = state(vec![m]);

        // Enough launches that the significance gate is cleared and the order really does move.
        for _ in 0..60 {
            s.record_launch_for_test("m", "popular");
        }
        s.rebuild();
        assert_eq!(
            s.machines[0].cells[0][0].apps[0].name, "popular",
            "precondition: usage put the popular line first ON SCREEN"
        );

        // A drag ANYWHERE on this machine materialises the whole of it.
        s.place_app(0, "unrelated", 1, None, None);

        assert_eq!(
            s.placement["m"]["Terminals"],
            vec![vec!["rare".to_string()], vec!["popular".to_string()]],
            "the FILE kept the arrangement, not the ranking"
        );
    }

    /// Typing puts the cursor on the first match, not on whatever the goal column happens to hit.
    ///
    /// The goal column is what makes up/down preserve your position, and it is only meaningful
    /// while a line's membership holds still. A query rewrites that membership, so a surviving
    /// goal selects the Nth survivor -- which is an arbitrary app, while the one the user is
    /// looking at as they type is the first.
    #[test]
    fn a_query_puts_the_cursor_on_the_first_match() {
        let mut s = state(vec![machine("m", 0, vec![vec!["alpha", "beta", "gamma", "delta"]])]);
        s.focus = Focus::Inside;
        s.item = 3;
        s.item_goal = 3;

        // Every name contains an "a", so nothing is filtered out and only the cursor can move.
        s.set_query("a".into());
        s.clamp();

        assert_eq!(s.item, 0, "landed on the first match");
        assert_eq!(s.item_goal, 0, "and the stale goal column went with it");
        assert_eq!(s.view[0].cells[0][0].apps[s.item].name, "alpha");
    }

    /// ...but a REBUILD must not move the cursor, which is why the reset lives in `set_query` and
    /// not in `refilter`. A drag re-derives the whole grid, and a cursor that jumped to the top
    /// afterwards would leave the app the user just dropped.
    #[test]
    fn a_rebuild_leaves_the_cursor_where_it_was() {
        let mut s = state(vec![machine("m", 0, vec![vec!["a", "b", "c", "d"]])]);
        s.focus = Focus::Inside;
        s.item = 2;
        s.item_goal = 2;

        s.rebuild();
        s.clamp();

        assert_eq!(s.item, 2, "a rebuild is not a query");
        assert_eq!(s.item_goal, 2);
    }

    /// An app with an id distinct from its name, for the identity tests below.
    fn app_id(id: &str, name: &str) -> App {
        App { id: id.into(), name: name.into(), icon: "x".into(), exec: id.into(), terminal: false }
    }

    /// TWO APPS, ONE DISPLAY NAME. This is the whole reason ids exist.
    ///
    /// `org.kde.foo.desktop` and `foo.desktop` both calling themselves "Foo" is ordinary. Keyed on
    /// the name they were one object: filing one filed both, and whichever the lookup did not
    /// return became unreachable.
    #[test]
    fn two_apps_sharing_a_name_stay_distinct() {
        let mut m = machine("m", 0, vec![]);
        m.cells[0] = vec![Line { apps: vec![app_id("a.desktop", "Foo"), app_id("b.desktop", "Foo")] }];
        let mut s = state(vec![m]);

        // File ONE of them somewhere else, by its id.
        s.place_app(0, "b.desktop", 4, None, None);

        let terminals: Vec<&str> =
            s.machines[0].cells[0].iter().flat_map(|l| l.apps.iter()).map(|a| a.id.as_str()).collect();
        let files: Vec<&str> =
            s.machines[0].cells[4].iter().flat_map(|l| l.apps.iter()).map(|a| a.id.as_str()).collect();
        assert_eq!(terminals, vec!["a.desktop"], "the other one stayed put");
        assert_eq!(files, vec!["b.desktop"], "and only the named one moved");
    }

    /// An arrangement written before ids existed must survive, or the change is a silent delete.
    #[test]
    fn an_arrangement_written_under_names_is_migrated() {
        let mut m = machine("m", 0, vec![]);
        m.cells[0] = vec![Line { apps: vec![app_id("helix.desktop", "Helix")] }];
        let mut s = state(vec![m]);

        // What the old code would have written: the DISPLAY NAME, in a different folder.
        let mut folders = HashMap::new();
        folders.insert("Editors".to_string(), vec![vec!["Helix".to_string()]]);
        s.placement.insert("m".to_string(), folders);
        s.usage.insert(usage::key("m", "Helix"), usage::Entry { score: 9.0, last: 0 });

        s.migrate_names_to_ids();
        s.rebuild();

        assert_eq!(s.placement["m"]["Editors"], vec![vec!["helix.desktop".to_string()]]);
        assert!(s.usage.contains_key(&usage::key("m", "helix.desktop")), "the score came too");
        assert!(!s.usage.contains_key(&usage::key("m", "Helix")), "and did not stay behind");
        assert_eq!(
            s.machines[0].cells[1][0].apps[0].id, "helix.desktop",
            "and the arrangement still applies: it is in Editors, not its computed folder"
        );
    }

    /// Running it twice must change nothing the second time, or a later start could rewrite an id
    /// that happens to collide with some other app's display name.
    #[test]
    fn the_migration_is_idempotent() {
        let mut m = machine("m", 0, vec![]);
        m.cells[0] = vec![Line { apps: vec![app_id("helix.desktop", "Helix")] }];
        let mut s = state(vec![m]);
        let mut folders = HashMap::new();
        folders.insert("Editors".to_string(), vec![vec!["Helix".to_string()]]);
        s.placement.insert("m".to_string(), folders);

        s.migrate_names_to_ids();
        let once = s.placement.clone();
        s.migrate_names_to_ids();
        assert_eq!(s.placement, once);
    }

    /// An AMBIGUOUS old entry is left alone rather than guessed.
    ///
    /// If two apps share the display name an old entry records, nothing in the file says which was
    /// meant. Picking one hands the other's arrangement to the wrong app, silently and with no way
    /// to notice; leaving it drops both back to their computed folders, which is visible and one
    /// drag from fixed.
    #[test]
    fn an_ambiguous_name_is_not_guessed() {
        let mut m = machine("m", 0, vec![]);
        m.cells[0] = vec![Line { apps: vec![app_id("a.desktop", "Foo"), app_id("b.desktop", "Foo")] }];
        let mut s = state(vec![m]);
        let mut folders = HashMap::new();
        folders.insert("Editors".to_string(), vec![vec!["Foo".to_string()]]);
        s.placement.insert("m".to_string(), folders);

        s.migrate_names_to_ids();

        assert_eq!(
            s.placement["m"]["Editors"],
            vec![vec!["Foo".to_string()]],
            "untouched -- an unresolvable entry is left, not resolved to a coin flip"
        );
    }

    /// Dropping an item into the gap it already occupies is a no-op, not a shuffle.
    #[test]
    fn dropping_an_item_where_it_already_is_changes_nothing() {
        let mut s = state(vec![machine("m", 0, vec![vec!["a", "b", "c"]])]);
        s.place_app(0, "a", 0, Some(&on(&["a", "b", "c"])), Some("b")); // the gap a already occupies
        let names: Vec<&str> =
            s.machines[0].cells[0][0].apps.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c"]);
    }

    /// Field codes are dropped AFTER splitting, so a quoted argument keeps its internal spacing.
    #[test]
    fn stripping_does_not_reach_inside_a_quoted_argument() {
        let m = machine("m", 0, vec![vec!["x"]]);
        assert_eq!(
            launch_argv(&m, &appx("q", "prog \"/a  b\" %U"), &[]),
            vec!["prog", "/a  b"],
            "two spaces preserved, %U gone"
        );
    }

    /// `{}` is substituted where it appears rather than reaching exec as a literal argument.
    #[test]
    fn the_launch_template_placeholder_is_substituted() {
        let mut m = machine("m", 0, vec![vec!["x"]]);
        m.launch = vec!["ssh".into(), "box".into(), "{}".into()];
        assert_eq!(
            launch_argv(&m, &appx("Helix", "helix %F"), &[]),
            vec!["ssh", "box", "helix"]
        );
    }

    /// clamp() is the only thing standing between a shrinking grid and an index panic.
    ///
    /// NOTE THE SHAPE OF THIS INVARIANT, which a first version of this test got wrong: resting on
    /// an EMPTY cell is legal and reachable on purpose. Empty cells are drawn rather than
    /// collapsed, because "that machine has no chat client" is an answer worth seeing, and you have to
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
        assert!(s.row < s.folders.len(), "row in range");
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
