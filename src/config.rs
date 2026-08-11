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
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub exec: String,
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MachineConfig {
    pub name: String,
    /// The identity colour for this machine's column. Same value the window frames and
    /// forwarded-window badges use, so a column is recognisable before its label is read.
    #[serde(default = "default_accent")]
    pub accent: String,
    /// argv that prints an `Inventory` as JSON on stdout. NOT a shell string: a list cannot be
    /// re-split on spaces by accident, and a machine name with a space in it stays one argument.
    pub inventory: Vec<String>,
    /// argv template for launching one app. `{}` is replaced by the app's own Exec. Absent means
    /// this machine cannot launch anything, which is a legitimate read-only column.
    #[serde(default)]
    pub launch: Vec<String>,
}

fn default_accent() -> String {
    "#22C55E".to_string()
}

#[derive(Deserialize, Debug, Clone)]
pub struct Config {
    pub machines: Vec<MachineConfig>,
    /// Row order. "Other" is appended automatically if absent and forced last if present -- it is
    /// the inbox, not a category, and a config that could bury it in the middle would defeat it.
    #[serde(default)]
    pub folders: Vec<String>,
}

impl Config {
    /// Row labels, with the inbox guaranteed present and last.
    pub fn folder_rows(&self) -> Vec<String> {
        let mut rows: Vec<String> =
            self.folders.iter().filter(|f| f.as_str() != "Other").cloned().collect();
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
    let Some(path) = config_path() else { return Ok(None) };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    serde_json::from_str(&text).map(Some).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_is_appended_when_absent() {
        let c: Config = serde_json::from_str(
            r#"{"machines":[],"folders":["Terminals","Editors"]}"#,
        )
        .unwrap();
        assert_eq!(c.folder_rows(), vec!["Terminals", "Editors", "Other"]);
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
        let c: Config = serde_json::from_str(
            r#"{"machines":[{"name":"box","inventory":["echo","{}"]}]}"#,
        )
        .unwrap();
        assert_eq!(c.machines[0].name, "box");
        assert_eq!(c.machines[0].accent, "#22C55E", "accent defaulted");
        assert!(c.machines[0].launch.is_empty(), "a read-only column is legitimate");
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
}
