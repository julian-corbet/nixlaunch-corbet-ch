// A probe, not a shell. It draws the grid once and does nothing else.
//
// The question is what a non-GTK toolkit COSTS -- private memory and time to first draw -- and
// neither of those needs the launcher to work. So there is no navigation here, no drag and drop,
// no search, no launching: just the same inventory, arranged the same way, put on screen once.
//
// TEXT ONLY, WHICH IS WHY THE COMPARISON RUNS AGAINST GTK WITH ICONS BLANKED. Icons are a real
// cost (they were 90% of this program's memory before they were cached) and drawing them here
// would mean reimplementing the icon-theme lookup, the downscale and the cache in a second
// toolkit. Comparing text-only against text-only measures the toolkit, which is the question;
// comparing text-only against icons-included would measure my willingness to port a cache.
use nixlaunch_core::{config, model};

use iced::widget::{column, row, scrollable, text};
use iced::{Element, Length};

#[derive(Debug, Clone)]
enum Message {}

struct Probe {
    machines: Vec<model::Machine>,
    folders: Vec<String>,
}

impl Probe {
    fn new() -> Self {
        // The same path the real shell takes: read the config, run each machine's inventory
        // command, bucket by the operator's own folder table. All of it from the core crate,
        // unchanged -- which is the thing being demonstrated as much as the memory figure.
        let Ok(Some(cfg)) = config::load() else {
            return Probe { machines: vec![], folders: vec![] };
        };
        let folders = cfg.folder_rows();
        let machines = cfg
            .machines
            .iter()
            .map(|mc| inventory(mc, &folders, cfg.theme.line_width))
            .collect();
        Probe { machines, folders }
    }

    fn view(&self) -> Element<'_, Message> {
        let mut cols = row![].spacing(12);
        for m in &self.machines {
            let mut col = column![text(m.name.clone()).size(14)].spacing(6);
            for (r, label) in self.folders.iter().enumerate() {
                let mut cell = column![text(label.clone()).size(11)].spacing(2);
                for line in m.cells.get(r).into_iter().flatten() {
                    let names: Vec<&str> = line.apps.iter().map(|a| a.name.as_str()).collect();
                    cell = cell.push(text(names.join("  ")).size(11));
                }
                col = col.push(cell);
            }
            cols = cols.push(col.width(Length::Fill));
        }
        scrollable(cols).into()
    }

    fn update(&mut self, _m: Message) {}
}

/// The same shape as the real shell's, minus everything that is not needed to draw once: no icon
/// handling, no terminal wrapping, no error column. Sequential rather than threaded, because a
/// prototype measuring steady-state memory does not care which order the machines answer in.
fn inventory(mc: &config::MachineConfig, rows: &[String], line_width: usize) -> model::Machine {
    let mut cells: Vec<Vec<model::Line>> = vec![Vec::new(); rows.len()];
    if let Some((bin, args)) = mc.inventory.split_first() {
        if let Ok(out) = std::process::Command::new(bin).args(args).output() {
            if let Ok(inv) = config::parse_inventory(&out.stdout) {
                for folder in inv.folders {
                    let r = rows
                        .iter()
                        .position(|x| *x == folder.label)
                        .unwrap_or(rows.len().saturating_sub(1));
                    for chunk in folder.apps.chunks(line_width.max(1)) {
                        cells[r].push(model::Line {
                            apps: chunk
                                .iter()
                                .map(|a| model::App {
                                    id: a.id.clone().unwrap_or_else(|| a.name.clone()),
                                    name: a.name.clone(),
                                    icon: String::new(),
                                    exec: a.exec.clone(),
                                    terminal: a.terminal,
                                })
                                .collect(),
                        });
                    }
                }
            }
        }
    }
    model::Machine {
        name: mc.name.clone(),
        accent: mc.accent.clone(),
        launch: mc.launch.clone(),
        error: None,
        cells,
    }
}

fn main() -> iced::Result {
    iced::run("nixlaunch shell probe", Probe::update, Probe::view).map(|_| ())
}

impl Default for Probe {
    fn default() -> Self {
        Probe::new()
    }
}
