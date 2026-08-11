// icons.rs — decode each icon once in the life of the machine, not once per launch.
//
// WHY THIS EXISTS. Measured headlessly on a 191-application inventory, with the inventory served
// from a file so no SSH is in the path:
//
//   191 apps, icons blanked    0.095s   <- exec, dynamic link, GTK init, window, grid
//   191 apps, with icons       0.170s   <- the same, plus icons
//
// Icons are three quarters of everything above the floor, and the grid itself is about ten
// milliseconds of it. That cost is paid on EVERY launch to produce a result that is identical
// every time: the same 95 distinct names resolve to the same files and rasterise to the same
// twenty-pixel images until somebody installs software.
//
// So it is paid once and written down. What lands here is the finished pixels -- not the PNG, not
// the SVG, not a path to either -- so a warm launch does no theme lookup, opens no image file,
// runs no rasteriser, and never loads librsvg at all. 95 icons at 20px is 152kB on disk.
//
// ── WHY A WHOLE-CACHE STAMP RATHER THAN PER-ENTRY VALIDATION ────────────────────────────────
//
// Checking each icon against its source file would mean asking the icon theme where every icon
// lives, and that lookup is a large part of the cost this exists to avoid -- a cache that has to
// do the expensive thing to find out whether it is valid saves nothing. Instead the whole file
// carries one stamp taken from the icon THEME's own index files. Installing or removing software
// rewrites those indexes, so the stamp moves and the cache is discarded wholesale. Between
// installs it is trusted completely.
//
// The failure mode this chooses is the right way round: a stale cache shows a stale icon until the
// next package operation, while an over-eager check costs latency on every launch forever.
use gtk4::prelude::*;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

/// Bumped whenever the on-disk layout changes, so an old file is discarded rather than
/// misread. A cache that cannot be parsed is simply absent -- never a reason to fail to start.
const MAGIC: &[u8; 6] = b"NLIC02";

pub struct Icons {
    px: i32,
    stamp: u64,
    /// WHERE this cache lives, carried rather than recomputed. Tests need their own file, and the
    /// obvious way to give them one -- pointing XDG_CACHE_HOME somewhere else -- is a race: tests
    /// share a process, so one setting the variable changes it under every other running thread.
    /// That is the same trap the state writers are guarded against, and writing it here anyway is
    /// how this was found. An injected path cannot race.
    path: PathBuf,
    /// name -> RGBA pixels at `px` square. The persisted form.
    pixels: HashMap<String, Vec<u8>>,
    /// name -> the texture handed to GTK, built on first use and kept for the process's life.
    /// Separate from `pixels` because a texture cannot be written to a file and pixels cannot be
    /// drawn; keeping both costs one copy of 1.6kB per icon and saves rebuilding either.
    textures: HashMap<String, Option<gtk4::gdk::Texture>>,
    /// Set when a name was decoded that the file did not have, i.e. the file is worth rewriting.
    dirty: bool,
}

fn cache_path(px: i32) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("nixlaunch").join(format!("icons-{px}.bin"))
}

/// One number for "has the set of installed icons changed".
///
/// The newest mtime across the icon themes' own index files. Package operations rewrite those --
/// it is what `gtk-update-icon-cache` exists to do -- so this moves exactly when the answer to an
/// icon lookup could have changed, and stays still the rest of the time.
fn theme_stamp() -> u64 {
    let mut roots: Vec<PathBuf> = vec![
        PathBuf::from("/usr/share/icons"),
        PathBuf::from("/usr/share/pixmaps"),
        PathBuf::from("/run/current-system/sw/share/icons"),
    ];
    if let Some(h) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(&h).join(".local/share/icons"));
        roots.push(PathBuf::from(&h).join(".nix-profile/share/icons"));
    }
    let mut newest = 0u64;
    for root in roots {
        let Ok(themes) = std::fs::read_dir(&root) else { continue };
        for theme in themes.flatten() {
            for f in ["icon-theme.cache", "index.theme"] {
                if let Ok(md) = std::fs::metadata(theme.path().join(f)) {
                    if let Ok(t) = md.modified() {
                        if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                            newest = newest.max(d.as_secs());
                        }
                    }
                }
            }
        }
    }
    newest
}

impl Icons {
    pub fn load(px: i32) -> Self {
        Self::load_from(cache_path(px), px, theme_stamp())
    }

    fn load_from(path: PathBuf, px: i32, stamp: u64) -> Self {
        let mut me = Icons {
            px,
            stamp,
            path: path.clone(),
            pixels: HashMap::new(),
            textures: HashMap::new(),
            dirty: false,
        };
        let Ok(buf) = std::fs::read(&path) else { return me };
        if buf.len() < MAGIC.len() + 12 || &buf[..MAGIC.len()] != MAGIC {
            return me;
        }
        let mut at = MAGIC.len();
        let take = |b: &[u8], at: &mut usize, n: usize| -> Option<Vec<u8>> {
            if *at + n > b.len() {
                return None;
            }
            let v = b[*at..*at + n].to_vec();
            *at += n;
            Some(v)
        };
        let Some(st) = take(&buf, &mut at, 8) else { return me };
        if u64::from_le_bytes(st.try_into().unwrap_or([0; 8])) != stamp {
            // Something was installed or removed since this was written. Start again rather than
            // trust it -- this is the one moment a wrong icon is cheap to avoid.
            return me;
        }
        let Some(cnt) = take(&buf, &mut at, 4) else { return me };
        let count = u32::from_le_bytes(cnt.try_into().unwrap_or([0; 4])) as usize;
        let bytes = (px * px * 4) as usize;
        for _ in 0..count {
            let Some(nl) = take(&buf, &mut at, 2) else { break };
            let n = u16::from_le_bytes(nl.try_into().unwrap_or([0; 2])) as usize;
            let Some(name) = take(&buf, &mut at, n) else { break };
            let Some(px_data) = take(&buf, &mut at, bytes) else { break };
            if let Ok(name) = String::from_utf8(name) {
                me.pixels.insert(name, px_data);
            }
        }
        me
    }

    /// The texture for `name`, from the cache when possible and from the icon theme when not.
    pub fn texture(
        &mut self,
        name: &str,
        theme: &gtk4::IconTheme,
    ) -> Option<gtk4::gdk::Texture> {
        if name.is_empty() {
            return None;
        }
        if let Some(hit) = self.textures.get(name) {
            return hit.clone();
        }
        let px = self.px;
        let made = match self.pixels.get(name) {
            // THE WARM PATH, and the whole point: no lookup, no file, no rasteriser.
            Some(data) => Some(gtk4::gdk::MemoryTexture::new(
                px,
                px,
                gtk4::gdk::MemoryFormat::R8g8b8a8,
                &gtk4::glib::Bytes::from(data.as_slice()),
                (px * 4) as usize,
            ))
            .map(|t| t.upcast::<gtk4::gdk::Texture>()),
            None => {
                let decoded = decode(name, px, theme);
                if let Some(ref pb) = decoded {
                    if let Some(raw) = rgba_square(pb, px) {
                        self.pixels.insert(name.to_string(), raw);
                        self.dirty = true;
                    }
                }
                decoded.map(|pb| gtk4::gdk::Texture::for_pixbuf(&pb).upcast())
            }
        };
        self.textures.insert(name.to_string(), made.clone());
        made
    }

    /// Write the file if anything new was decoded. Atomic, for the same reason the state files are:
    /// a truncated cache is worse than none, because it parses as "these icons are blank".
    pub fn save(&self) {
        if !self.dirty {
            return;
        }
        let path = &self.path;
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let bytes = (self.px * self.px * 4) as usize;
        let mut out: Vec<u8> = Vec::with_capacity(self.pixels.len() * (bytes + 32) + 32);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.stamp.to_le_bytes());
        let usable: Vec<(&String, &Vec<u8>)> =
            self.pixels.iter().filter(|(n, d)| d.len() == bytes && n.len() <= u16::MAX as usize).collect();
        out.extend_from_slice(&(usable.len() as u32).to_le_bytes());
        for (name, data) in usable {
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
        }
        let tmp = path.with_extension("bin.tmp");
        if let Ok(mut f) = std::fs::File::create(&tmp) {
            if f.write_all(&out).is_ok() && f.sync_all().is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
    }
}

/// Resolve a name through the icon theme and rasterise it to `px` square.
///
/// This is the slow path, and everything above exists to run it as rarely as possible. Vector and
/// raster take the same route deliberately: `IconPaintable` renders an SVG at the size asked for
/// and a PNG at the file's own size, so going through the pixbuf loader for both is what makes the
/// result uniformly small rather than uniformly whatever upstream shipped.
fn decode(name: &str, px: i32, theme: &gtk4::IconTheme) -> Option<gtk4::gdk_pixbuf::Pixbuf> {
    let path = theme
        .lookup_icon(name, &[], px, 1, gtk4::TextDirection::None, gtk4::IconLookupFlags::empty())
        .file()?
        .path()?;
    gtk4::gdk_pixbuf::Pixbuf::from_file_at_size(&path, px, px).ok()
}

/// Pixels as an exactly `px` by `px` RGBA block, which is what the cache format stores and what
/// `MemoryTexture` wants. A loader asked for a size gives back something that FITS it, preserving
/// aspect, so a wide icon comes back short -- centring it here means every entry is one fixed
/// length and the file needs no per-entry geometry.
fn rgba_square(pb: &gtk4::gdk_pixbuf::Pixbuf, px: i32) -> Option<Vec<u8>> {
    let (w, h) = (pb.width(), pb.height());
    if w <= 0 || h <= 0 || w > px || h > px {
        return None;
    }
    let src = pb.read_pixel_bytes();
    let stride = pb.rowstride() as usize;
    let chans = pb.n_channels() as usize;
    if chans != 3 && chans != 4 {
        return None;
    }
    let mut out = vec![0u8; (px * px * 4) as usize];
    let (ox, oy) = (((px - w) / 2) as usize, ((px - h) / 2) as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let s = y * stride + x * chans;
            if s + chans > src.len() {
                return None;
            }
            let d = ((y + oy) * px as usize + (x + ox)) * 4;
            out[d] = src[s];
            out[d + 1] = src[s + 1];
            out[d + 2] = src[s + 2];
            out[d + 3] = if chans == 4 { src[s + 3] } else { 255 };
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nixlaunch-icons-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&d).unwrap();
        d.join("icons.bin")
    }

    fn blank(path: PathBuf, px: i32, stamp: u64) -> Icons {
        Icons { px, stamp, path, pixels: HashMap::new(), textures: HashMap::new(), dirty: true }
    }

    /// A file this program did not write, or wrote in an older shape, reads as "no cache" rather
    /// than as garbage pixels.
    #[test]
    fn a_foreign_file_is_ignored() {
        let p = tmp("foreign");
        std::fs::write(&p, b"not a nixlaunch cache at all").unwrap();
        assert!(Icons::load_from(p, 20, 1).pixels.is_empty(), "garbage must not become icons");
    }

    /// The stamp IS the validity test, so a mismatched one discards everything -- otherwise an
    /// application that was uninstalled keeps its icon forever.
    #[test]
    fn a_stale_stamp_discards_everything() {
        let p = tmp("stale");
        let px = 4;
        let mut w = blank(p.clone(), px, 12345);
        w.pixels.insert("thing".into(), vec![7u8; (px * px * 4) as usize]);
        w.save();

        assert!(!Icons::load_from(p.clone(), px, 999).pixels.is_empty() == false, "a different stamp reads as empty");
        assert_eq!(Icons::load_from(p, px, 12345).pixels.len(), 1, "the matching stamp still reads");
    }

    /// Round trip: what is written comes back byte for byte.
    #[test]
    fn a_written_cache_reads_back_identical() {
        let p = tmp("roundtrip");
        let px = 4;
        let data: Vec<u8> = (0..(px * px * 4)).map(|i| (i % 251) as u8).collect();
        let mut w = blank(p.clone(), px, 77);
        w.pixels.insert("round.trip".into(), data.clone());
        w.save();

        assert_eq!(Icons::load_from(p, px, 77).pixels.get("round.trip"), Some(&data));
    }

    /// Nothing new decoded means nothing written -- a warm start must not rewrite the file it just
    /// read, which would be a needless write on the latency path every single launch.
    #[test]
    fn a_clean_cache_is_not_rewritten() {
        let p = tmp("clean");
        let mut w = blank(p.clone(), 4, 5);
        w.pixels.insert("x".into(), vec![0u8; 64]);
        w.save();
        let before = std::fs::metadata(&p).unwrap().len();
        let mut read_back = Icons::load_from(p.clone(), 4, 5);
        read_back.dirty = false;
        std::fs::write(&p, b"sentinel").unwrap();
        read_back.save();
        assert_eq!(std::fs::read(&p).unwrap(), b"sentinel", "a clean cache wrote nothing");
        assert!(before > 0);
    }
}
