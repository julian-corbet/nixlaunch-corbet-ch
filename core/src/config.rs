// config.rs — what nixlaunch is TOLD, as opposed to what it discovers or what the user rearranges.
//
// THREE KINDS OF DATA, AND KEEPING THEM APART IS THE WHOLE DESIGN:
//
//   config     (this file)  declared in Nix, rendered to a file, read-only here. Which machines
//                           exist, what they are called, what colour they are, and HOW TO ASK each
//                           one what it has. Rebuilt by a deploy; never written by this program.
//   inventory  (model.rs)   what a machine actually has right now. Discovered, cached, disposable
//                           -- a re-inventory may replace it wholesale at any time.
//   placement  (model.rs)   what the user rearranged. Written by this program, and must survive
//                           both of the above being replaced.
//
// Conflating any two of them is how a launcher ends up either forgetting your arrangement on every
// rebuild, or freezing an app list that has since changed.
//
// ── WHY THIS SCHEMA IS NOT NEW ───────────────────────────────────────────────────────────────
//
// It mirrors the option schema `nixremote.launcher` already exposes and that a real infra repo
// already drives, deliberately, because those decisions were argued once and are still right:
//
//   * `machines` is a LIST, not a map. The order is the tab order and it is meaningful; a map
//     would silently alphabetise it.
//   * `folders` is a LIST in PRIORITY ORDER, for the same reason and a sharper one: grouping is
//     first-match-wins, so "TerminalEmulator" must be able to precede "System" or every terminal
//     lands in System. Alphabetising that changes which group an app falls into.
//   * `hide` matches a `.desktop` FILENAME, never a display name. The filename is the stable
//     identifier; `Name=` is localised and changes with a package update.
//
// ── HOW PROGRAMS ARE DETECTED: THEY ARE NOT, HERE ────────────────────────────────────────────
//
// `inventory` is a COMMAND, and that is the entire detection story from this program's point of
// view. It runs it, and reads the JSON contract back. It does not know about SSH, .desktop files,
// flatpaks, package managers or remote hosts, and it must not learn: "what programs exist on a
// machine" is a question that already has an owner, and a launcher is the wrong place to grow a
// second, competing answer to it.
//
// That keeps this repo buildable and testable by anyone with no fleet at all -- point `inventory`
// at a script that echoes fixed JSON and the launcher works completely.
use serde::Deserialize;
use std::path::PathBuf;

/// Read what an inventory command printed.
///
/// HERE RATHER THAN IN A SHELL, because the shape of this JSON is the contract and the contract is
/// the core's. A shell's job is to run the command and hand over the bytes; if each one parsed for
/// itself, each would own a copy of the schema and they would drift the first time a field was
/// added. It also means a shell needs no JSON library of its own -- the GTK one carried serde_json
/// solely for this call.
pub fn parse_inventory(bytes: &[u8]) -> Result<Inventory, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("unreadable inventory: {e}"))
}

/// One named row inside a box, and which applications belong in it.
///
/// The names alone were half a feature: declaring "biz", "leis" and "priv" builds three empty
/// shelves and leaves every application in the catch-all, which is more rows to look past rather
/// than fewer. What makes a subcategory worth having is that things are IN it.
///
/// Membership is declared beside the name rather than dragged in one application at a time,
/// because two hundred applications is not a drag-and-drop job -- and because an arrangement that
/// exists only in a state file cannot be reviewed, copied to another machine, or explained.
/// Dragging still works and still wins: it writes to placement, which is applied after this.
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct SubRow {
    pub name: String,
    /// Matched case-insensitively as a SUBSTRING of the application's id or its display name, so
    /// `signal` catches `signal-desktop.desktop` without anyone needing to know which spelling a
    /// package happens to use. Precision is available by writing more of the name.
    #[serde(default)]
    pub apps: Vec<String>,
}

/// The JSON one `inventory` command must print. Deliberately identical to what
/// `rlaunch --json <host>` already emits, so the common case needs no adapter.
#[derive(Deserialize, Debug, Clone)]
pub struct Inventory {
    #[serde(default)]
    pub host: String,
    /// Carried, not raised. An unreachable machine is a normal state on a roaming laptop, and the
    /// UI wants to draw that column greyed out with a reason on it rather than be handed an empty
    /// list it cannot tell apart from "this machine genuinely has nothing".
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub folders: Vec<InventoryFolder>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct InventoryFolder {
    pub label: String,
    #[serde(default)]
    pub apps: Vec<InventoryApp>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct InventoryApp {
    pub name: String,
    /// The provider's own stable identity for this application -- `rlaunch --json` emits the
    /// desktop-entry filename. OPTIONAL, defaulting to the name, so a provider that predates this
    /// field still parses: such an inventory behaves exactly as the program did before ids
    /// existed, which is correct until two of its apps share a display name.
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub exec: String,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct MachineConfig {
    pub name: String,
    /// Shorter things this machine answers to when typed in the search box.
    ///
    /// A local convention, which is why it is configuration and not a guess: what an estate
    /// shortens its hostnames to is not derivable from the hostnames. Declared aliases beat prefix
    /// matching, so a shortcut somebody relies on cannot be broken later by adding a machine whose
    /// name happens to begin with the same letters.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// The identity colour for this machine's column. Same value the window frames and
    /// forwarded-window badges use, so a column is recognisable before its label is read.
    #[serde(default = "default_accent")]
    pub accent: String,
    /// argv that prints an `Inventory` as JSON on stdout. NOT a shell string: a list cannot be
    /// re-split on spaces by accident, and a machine name with a space in it stays one argument.
    pub inventory: Vec<String>,
    /// Maximum time one inventory command may run. Inventory is arbitrary external code and an
    /// unreachable machine is normal; without a bound, one wedged command can keep a cold start
    /// waiting forever and can leak one refresh worker on every reopen.
    #[serde(default = "default_inventory_timeout_ms")]
    pub inventory_timeout_ms: u64,
    /// argv template for launching one app. `{}` is replaced by the app's own Exec. Absent means
    /// this machine cannot launch anything, which is a legitimate read-only column.
    #[serde(default)]
    pub launch: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_surface() -> String {
    "layer".to_string()
}

fn default_keyboard() -> String {
    "exclusive".to_string()
}

fn default_accent() -> String {
    "#22C55E".to_string()
}

fn default_inventory_timeout_ms() -> u64 {
    5_000
}

/// The palette, as VALUES rather than as code.
///
/// The defaults below are a working dark set so the launcher is usable the moment it is installed
/// -- they are NOT a house palette, and nothing here should be read as one. Override them with
/// whatever the rest of your desktop already uses, so this looks like part of the same product
/// rather than a second one that happens to be running.
///
/// Welding these into the stylesheet was the original mistake: a colour no consumer can reach is
/// this repo carrying one estate's taste as though it were a property of launchers.
#[derive(Deserialize, Debug, Clone)]
#[serde(default, deny_unknown_fields)]
pub struct Theme {
    pub ground: String,
    pub surface: String,
    pub fg: String,
    pub muted: String,
    pub dim: String,
    pub accent: String,
    pub error: String,
    pub border: String,
    pub icon_size: i32,

    /// An image for the corner the label columns leave empty: an absolute path, or an icon name

    /// resolved against the theme. Empty means nothing is drawn -- a launcher that shipped

    /// somebody's mark by default would be wearing it.

    #[serde(default)]
    pub logo: String,

    /// How large that image is drawn. Separate from `icon_size` because the corner has a whole

    /// header row to fill while an application icon has to sit inside a line of text.

    #[serde(default = "default_logo_size")]
    pub logo_size: i32,
    /// Apps per line when packing a fresh inventory. Taste, not mechanism: it is how many
    /// left/right steps a row costs before up/down is the faster move, and the answer depends on
    /// how many machine columns you have and how wide the display is. Two machines on an ultrawide
    /// want more; six on a laptop want fewer.
    pub line_width: usize,
    /// How much of the display the grid may occupy before it scrolls. Display-RELATIVE is
    /// mechanism and stays; the fraction itself is a preference about how much of the session
    /// stays visible behind the launcher.
    pub max_height_fraction: f64,
    /// How much of the display's WIDTH the grid may take before it scrolls sideways.
    ///
    /// The companion to `max_height_fraction`, and it exists for the same reason rather than for
    /// symmetry: more machine columns than fit meant a window wider than the screen, with the far
    /// columns unreachable by scrollbar, keyboard or drag. Higher than the height fraction because
    /// a launcher spanning most of the width still leaves the session legible behind it, where one
    /// spanning most of the height does not.
    pub max_width_fraction: f64,
    /// Minimum width of the search bar, and so effectively of the window.
    pub width: i32,
}

fn default_logo_size() -> i32 {
    28
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            ground: "#0A0A0A".into(),
            surface: "#0E0E0E".into(),
            fg: "#F0F0F0".into(),
            muted: "#999999".into(),
            dim: "#444444".into(),
            accent: "#22C55E".into(),
            error: "#B91322".into(),
            border: "#1C1C1C".into(),
            icon_size: 20,
            logo: String::new(),
            logo_size: default_logo_size(),
            line_width: 4,
            max_height_fraction: 0.66,
            max_width_fraction: 0.9,
            width: 560,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub machines: Vec<MachineConfig>,
    /// "layer" (default) or "window".
    ///
    /// A layer surface, like every other Wayland launcher. `window` exists for debugging.
    ///
    /// The keyboard question is settled by `exit_on_focus_loss`, not by the surface type. See it.
    #[serde(default = "default_surface")]
    pub surface: String,

    /// Close the moment the keyboard focus goes elsewhere. THIS is what makes an exclusive grab
    /// harmless, and it is exactly what fuzzel does by default.
    ///
    /// Exclusive focus alone is a lock screen: it holds the seat for as long as the surface
    /// exists, so anything you type into another window is swallowed. Exclusive focus PLUS this is
    /// a launcher: it has the keyboard the instant it opens so you can type immediately, and the
    /// moment you click elsewhere it is gone and your keystrokes go where you are looking. The
    /// grab cannot outlive your attention, which is the only property that made it a problem.
    #[serde(default = "default_true")]
    pub exit_on_focus_loss: bool,

    /// Layer-surface keyboard mode, used only when `surface = "layer"`.
    ///
    /// EXCLUSIVE IS THE DEFAULT BECAUSE ON-DEMAND DOES NOT WORK, and that is a compositor bug
    /// rather than a preference. On every released sway (1.10-1.12) and its forks, a mapping layer
    /// surface is granted focus in `handle_map`, and then the `arrange_layers` call at the end of
    /// that same handler revokes it again for anything whose keyboard_interactive is not
    /// EXCLUSIVE. The surface maps and simply never receives a key. Every shipping launcher
    /// defaults to exclusive for this reason -- fuzzel additionally hard-falls-back to it when the
    /// protocol is too old to offer on-demand at all.
    ///
    /// The mode must also be set BEFORE the window is presented: sway reads it at map time from
    /// the surface's initial commit, and a mode applied after that is silently ignored.
    #[serde(default = "default_keyboard")]
    pub keyboard: String,

    /// argv that wraps a program declaring `Terminal=true` -- e.g. `["foot", "-e"]`.
    ///
    /// Not guessed, and not defaulted to some popular emulator: the right answer is whatever
    /// terminal this desktop already uses, and a launcher that opened a DIFFERENT one than every
    /// other part of the session would be wrong in a way nobody would think to look for. Empty
    /// means such programs are refused with an error rather than launched invisibly and lost.
    #[serde(default)]
    pub terminal: Vec<String>,
    #[serde(default)]
    pub theme: Theme,
    /// Row order. "Other" is appended automatically if absent and forced last if present -- it is
    /// the inbox, not a category, and a config that could bury it in the middle would defeat it.
    #[serde(default)]
    pub folders: Vec<String>,

    /// Named rows INSIDE a box, per folder: `{"Chat": ["business", "leisure", "private"]}`.

    ///

    /// A box with twenty-four things in it is a list wearing a grid's clothes -- the layout stops

    /// paying for itself once a cell is taller than a glance. Sub-rows give the second axis back

    /// inside the cell, and they are declared rather than inferred because a taxonomy is a

    /// judgement: no rule derives "business" from a set of chat clients.

    ///

    /// Declared rows are drawn even when empty, which is what makes them usable -- an invisible

    /// row is one you cannot drag anything into. Apps that were never filed keep appearing in

    /// unnamed lines above them, so declaring a sub-row never hides anything.

    #[serde(default)]
    pub subrows: std::collections::HashMap<String, Vec<SubRow>>,

    /// Key bindings, as chord -> action. Overrides the defaults rather than replacing them, and a

    /// null action unbinds -- see `keymap` for why both matter.

    #[serde(default)]
    pub keys: std::collections::HashMap<String, Option<crate::keymap::Action>>,
}

impl Config {
    /// Refuse values that would otherwise fail later as a blank surface, an index panic, or an
    /// ambiguous machine qualifier. The Home Manager module catches the same shapes at evaluation;
    /// the binary must still protect direct JSON consumers.
    pub fn validate(self) -> Result<Self, String> {
        if !matches!(self.surface.as_str(), "layer" | "window") {
            return Err(format!(
                "surface must be `layer` or `window`, got {:?}",
                self.surface
            ));
        }
        if !matches!(self.keyboard.as_str(), "exclusive" | "ondemand" | "none") {
            return Err(format!(
                "keyboard must be `exclusive`, `ondemand`, or `none`, got {:?}",
                self.keyboard
            ));
        }
        if self.theme.icon_size <= 0 || self.theme.logo_size <= 0 || self.theme.width <= 0 {
            return Err("theme icon_size, logo_size, and width must be positive".to_string());
        }
        if self.theme.line_width == 0 {
            return Err("theme.line_width must be positive".to_string());
        }
        for (name, value) in [
            ("max_height_fraction", self.theme.max_height_fraction),
            ("max_width_fraction", self.theme.max_width_fraction),
        ] {
            if !(0.0 < value && value <= 1.0) {
                return Err(format!("theme.{name} must be greater than 0 and at most 1"));
            }
        }
        if self.machines.iter().any(|m| m.name.trim().is_empty()) {
            return Err("machine names must not be empty".to_string());
        }
        if self.machines.iter().any(|m| m.inventory.is_empty()) {
            return Err("every machine needs an inventory command".to_string());
        }
        if self.machines.iter().any(|m| m.inventory_timeout_ms == 0) {
            return Err("machine inventory_timeout_ms must be positive".to_string());
        }

        let mut tokens = std::collections::HashMap::<String, String>::new();
        for machine in &self.machines {
            for token in std::iter::once(&machine.name).chain(machine.aliases.iter()) {
                let normal = token.to_lowercase();
                if let Some(first) = tokens.insert(normal, machine.name.clone()) {
                    return Err(format!(
                        "machine names and aliases must be unique case-insensitively; {:?} collides between {first:?} and {:?}",
                        token, machine.name
                    ));
                }
            }
        }

        for folder in self.subrows.keys() {
            if folder == "Other" || !self.folders.iter().any(|f| f == folder) {
                return Err(format!(
                    "subrows.{folder} has no declared folder; subrows must belong to a configured folder other than Other"
                ));
            }
        }
        Ok(self)
    }

    /// Row labels, with the inbox guaranteed present and last.
    pub fn folder_rows(&self) -> Vec<String> {
        // DEDUPED, first occurrence wins. A repeated label is not merely untidy: rows are matched
        // by label, so a duplicate makes every app in that folder appear in two rows at once, and
        // a drag onto either writes to whichever the code happens to reach first.
        // A SUBCATEGORY IS A ROW, spelled `folder/sub`.
        //
        // Making it part of the row label rather than a structure inside the cell is what lets the
        // label sit OUTSIDE the machine columns: the grid already draws one label per row, so a
        // subcategory drawn as a row gets that for free and lines up across every machine. Nested
        // inside a cell it could only ever line up with itself.
        //
        // The bare folder name is emitted LAST, after its subcategories, and is the catch-all --
        // which is also what makes this readable by an arrangement written before subcategories
        // existed: those entries are keyed on the bare folder name and still land in it.
        let mut seen = std::collections::HashSet::new();
        let mut rows: Vec<String> = Vec::new();
        let push =
            |rows: &mut Vec<String>, r: String, seen: &mut std::collections::HashSet<String>| {
                if seen.insert(r.clone()) {
                    rows.push(r);
                }
            };
        for f in self.folders.iter().filter(|f| f.as_str() != "Other") {
            for sub in self.subrows.get(f).into_iter().flatten() {
                push(&mut rows, format!("{f}/{}", sub.name), &mut seen);
            }
            push(&mut rows, f.clone(), &mut seen);
        }
        rows.push("Other".to_string());
        rows
    }
}

/// `$NIXLAUNCH_CONFIG` first so a test or a bisect can point at one explicitly without touching
/// the user's real setup, then the XDG location a Nix module would render into.
pub fn config_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NIXLAUNCH_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("nixlaunch").join("config.json"))
}

/// None when there is no config at all -- the caller falls back to fixture data so the binary is
/// still runnable on a bare checkout. A config that EXISTS but does not parse is a different
/// situation entirely and must not be silently ignored: returning the error lets the caller say so
/// rather than pretending the machine list is empty.
pub fn load() -> Result<Option<Config>, String> {
    let Some(path) = config_path() else {
        return Ok(None);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    serde_json::from_str::<Config>(&text)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .validate()
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_is_appended_when_absent() {
        let c: Config =
            serde_json::from_str(r#"{"machines":[],"folders":["Terminals","Editors"]}"#).unwrap();
        assert_eq!(c.folder_rows(), vec!["Terminals", "Editors", "Other"]);
    }

    /// A repeated label would make every app in that folder appear in two rows at once.
    #[test]
    fn duplicate_folders_are_deduped() {
        let c: Config =
            serde_json::from_str(r#"{"machines":[],"folders":["Chat","Editors","Chat"]}"#).unwrap();
        assert_eq!(c.folder_rows(), vec!["Chat", "Editors", "Other"]);
    }

    /// A config that names "Other" in the middle must not be able to bury the inbox.
    #[test]
    fn other_is_forced_last_when_present() {
        let c: Config =
            serde_json::from_str(r#"{"machines":[],"folders":["Other","Terminals"]}"#).unwrap();
        assert_eq!(c.folder_rows(), vec!["Terminals", "Other"]);
    }

    #[test]
    fn a_machine_needs_only_a_name_and_an_inventory_command() {
        let c: Config =
            serde_json::from_str(r#"{"machines":[{"name":"box","inventory":["echo","{}"]}]}"#)
                .unwrap();
        assert_eq!(c.machines[0].name, "box");
        assert_eq!(c.machines[0].accent, "#22C55E", "accent defaulted");
        assert_eq!(
            c.machines[0].inventory_timeout_ms, 5_000,
            "timeout defaulted"
        );
        assert!(
            c.machines[0].launch.is_empty(),
            "a read-only column is legitimate"
        );
    }

    /// The inventory contract is exactly what `rlaunch --json` already prints.
    #[test]
    fn inventory_parses_the_rlaunch_shape() {
        let inv: Inventory = serde_json::from_str(
            r#"{"host":"console","error":null,"folders":[
                 {"label":"Terminals","apps":[
                   {"name":"Foot","icon":"foot","exec":"foot","terminal":false}]}]}"#,
        )
        .unwrap();
        assert_eq!(inv.host, "console");
        assert!(inv.error.is_none());
        assert_eq!(inv.folders[0].apps[0].name, "Foot");
    }

    /// An unreachable machine reports a reason and no folders, and that must parse cleanly --
    /// it is the state the UI most needs to draw differently.
    #[test]
    fn an_unreachable_machine_carries_its_reason() {
        let inv: Inventory =
            serde_json::from_str(r#"{"host":"faraway","error":"ssh: timed out","folders":[]}"#)
                .unwrap();
        assert_eq!(inv.error.as_deref(), Some("ssh: timed out"));
        assert!(inv.folders.is_empty());
    }

    #[test]
    fn unknown_config_fields_are_errors_not_silent_typos() {
        let error = serde_json::from_str::<Config>(r#"{"machines":[],"widht":500}"#).unwrap_err();
        assert!(error.to_string().contains("unknown field `widht`"));
    }

    #[test]
    fn machine_names_and_aliases_cannot_collide_by_case() {
        let config: Config = serde_json::from_str(
            r#"{"machines":[
                {"name":"Server","inventory":["true"]},
                {"name":"laptop","aliases":["server"],"inventory":["true"]}
            ]}"#,
        )
        .unwrap();
        let error = config.validate().unwrap_err();
        assert!(error.contains("unique case-insensitively"), "{error}");
    }

    #[test]
    fn invalid_dimensions_are_rejected_before_the_ui_uses_them() {
        let config: Config = serde_json::from_str(
            r#"{"machines":[],"theme":{"line_width":0,"max_height_fraction":1.5}}"#,
        )
        .unwrap();
        let error = config.validate().unwrap_err();
        assert!(error.contains("line_width"), "{error}");
    }
}
