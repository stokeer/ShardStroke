//! Persistent user configuration (`%APPDATA%\ShardStroke\config.ini`).
//!
//! Line format: `library<TAB>name<TAB>default(0/1)<TAB>path`. Libraries are only ever *referenced*:
//! ShardStroke reads their files in place and never copies or redistributes them.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Library {
    pub name: String,
    pub path: PathBuf,
    pub default: bool,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub libraries: Vec<Library>,
    /// Do not use the installed Substance renderer for procedurals (native approximations only).
    pub substance_engine_off: bool,
    /// Seconds between autosaves of an edited project (0 = off, the default: opt in from Settings; `autosave_every` replaces the old key, whose default of 120 wrote files nobody asked for).
    pub autosave_secs: u32,
    /// Highlight colour picked in Settings (None = the default purple).
    pub highlight: Option<[u8; 3]>,
    /// Animation level: 0 off, 1 basic, 2 advanced.
    pub animation: u8,
    /// Windows stay where they are: no X, no moving, tabbing, splitting or resizing.
    pub lock_layout: bool,
    /// Stripes over a texture set still loading, and its outline (sRGB).
    pub loading_fill: [u8; 3],
    pub loading_outline: [u8; 3],
    /// Outline around the picked layer / mask thumbnail (sRGB).
    pub selection_outline: [u8; 3],
    /// Antialiasing of the viewport, samples per pixel (1 = off; the GPU may offer fewer).
    pub viewport_msaa: u32,
    /// Show frame time, FPS and CPU / GPU usage in the viewport.
    pub show_stats: bool,
    /// Last Export Textures settings: folder, and `template,png,size,padding,normal` (size 0 = each set's own,
    /// padding -1 = infinite, normal 0 = the template's / 1 OpenGL / 2 DirectX).
    pub export_dir: Option<PathBuf>,
    pub export: [i32; 6],
    /// Neural denoiser (Open Image Denoise) on the path-traced picture once the view stops.
    pub ai_denoise: bool,
    /// Quick panel (right click in the viewport): kept open as a floating window, and where.
    pub quick_pinned: bool,
    pub quick_pos: Option<[f32; 2]>,
    /// Sections of the quick panel in display order, hidden ones prefixed by `-` (empty = the default).
    pub quick_sections: String,
    /// Assets window view: grid (1) or list (0), thumbnail size, group (0 kind, 1 library, 2 none), sort (0 name,
    /// 1 kind), names under thumbnails (1/0).
    pub assets_view: [u32; 5],
    /// Projects opened lately (most recent first), for the start screen.
    pub recent_projects: Vec<PathBuf>,
    /// Colours painted with lately (sRGB, most recent first).
    pub recent_colors: Vec<[u8; 3]>,
    /// Brush settings painted with lately: radius, hardness, flow, opacity, spacing (most recent first).
    pub recent_brushes: Vec<[f32; 5]>,
}

/// Default colours of a texture set still loading: dark grey stripes, a lighter grey outline.
pub const LOADING_FILL: [u8; 3] = [48, 48, 48];
pub const LOADING_OUTLINE: [u8; 3] = [150, 150, 150];
pub const SELECTION_OUTLINE: [u8; 3] = [0xE2, 0xE2, 0xE2];

fn parse_rgb(s: &str) -> Option<[u8; 3]> {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

impl Default for Config {
    fn default() -> Self {
        Config {
            libraries: Vec::new(),
            substance_engine_off: false,
            autosave_secs: 0,
            highlight: None,
            animation: 2,
            lock_layout: false,
            loading_fill: LOADING_FILL,
            loading_outline: LOADING_OUTLINE,
            selection_outline: SELECTION_OUTLINE,
            viewport_msaa: 4,
            show_stats: false,
            export_dir: None,
            export: [0, 1, 0, -1, 0, 0],
            ai_denoise: true,
            quick_pinned: false,
            quick_pos: None,
            quick_sections: String::new(),
            assets_view: [1, 96, 0, 0, 1],
            recent_projects: Vec::new(),
            recent_colors: Vec::new(),
            recent_brushes: Vec::new(),
        }
    }
}

/// Folder of the user configuration (`SHARD_CONFIG_DIR` or `%APPDATA%\ShardStroke`; the folder of the
/// former name, `%APPDATA%\PigmaStudio`, is moved there the first time).
pub fn config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("SHARD_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    let appdata = PathBuf::from(std::env::var_os("APPDATA")?);
    let dir = appdata.join("ShardStroke");
    let old = appdata.join("PigmaStudio");
    if !dir.exists() && old.is_dir() {
        let _ = std::fs::rename(&old, &dir);
    }
    Some(dir)
}

fn config_path() -> Option<PathBuf> {
    Some(config_dir()?.join("config.ini"))
}

impl Config {
    pub fn load() -> Self {
        let mut cfg = Config::default();
        let Some(text) = config_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return cfg };
        for line in text.lines() {
            let f: Vec<&str> = line.splitn(4, '\t').collect();
            if f.len() == 3 && f[0] == "option" && f[1] == "substance_engine" {
                cfg.substance_engine_off = f[2] == "0";
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "highlight" {
                if let Ok(v) = u32::from_str_radix(f[2].trim_start_matches('#'), 16) {
                    cfg.highlight = Some([(v >> 16) as u8, (v >> 8) as u8, v as u8]);
                }
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "lock_layout" {
                cfg.lock_layout = f[2] == "1";
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "animation" {
                cfg.animation = f[2].parse::<u8>().unwrap_or(2).min(2);
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "loading_fill" {
                cfg.loading_fill = parse_rgb(f[2]).unwrap_or(LOADING_FILL);
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "loading_outline" {
                cfg.loading_outline = parse_rgb(f[2]).unwrap_or(LOADING_OUTLINE);
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "selection_outline" {
                cfg.selection_outline = parse_rgb(f[2]).unwrap_or(SELECTION_OUTLINE);
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "export_dir" {
                cfg.export_dir = Some(PathBuf::from(f[2]));
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "export" {
                let v: Vec<i32> = f[2].split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if v.len() >= 5 {
                    cfg.export = [v[0], v[1], v[2], v[3], v[4], v.get(5).copied().unwrap_or(0)];
                }
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "show_stats" {
                cfg.show_stats = f[2] == "1";
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "ai_denoise" {
                cfg.ai_denoise = f[2] == "1";
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "viewport_msaa" {
                cfg.viewport_msaa = f[2].parse().unwrap_or(4);
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "autosave_every" {
                cfg.autosave_secs = f[2].parse().unwrap_or(0);
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "quick_pinned" {
                cfg.quick_pinned = f[2] == "1";
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "quick_pos" {
                let v: Vec<f32> = f[2].split(',').filter_map(|x| x.trim().parse().ok()).collect();
                cfg.quick_pos = (v.len() == 2).then(|| [v[0], v[1]]);
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "quick_sections" {
                cfg.quick_sections = f[2].to_string();
                continue;
            }
            if f.len() == 3 && f[0] == "option" && f[1] == "assets_view" {
                let v: Vec<u32> = f[2].split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if v.len() == 5 {
                    cfg.assets_view = [v[0].min(1), v[1].clamp(48, 192), v[2].min(2), v[3].min(1), v[4].min(1)];
                }
                continue;
            }
            if f.len() == 2 && f[0] == "recent_project" {
                cfg.recent_projects.push(PathBuf::from(f[1]));
                continue;
            }
            if f.len() == 2 && f[0] == "recent_color" {
                cfg.recent_colors.extend(parse_rgb(f[1]));
                continue;
            }
            if f.len() == 2 && f[0] == "recent_brush" {
                let v: Vec<f32> = f[1].split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if v.len() == 5 {
                    cfg.recent_brushes.push([v[0], v[1], v[2], v[3], v[4]]);
                }
                continue;
            }
            if f.len() == 4 && f[0] == "library" {
                cfg.libraries.push(Library { name: f[1].to_string(), path: PathBuf::from(f[3]), default: f[2] == "1" });
            }
        }
        cfg
    }

    pub fn save(&self) {
        let Some(p) = config_path() else { return };
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let mut out = String::from("# ShardStroke configuration\n");
        out.push_str(&format!("option\tsubstance_engine\t{}\n", if self.substance_engine_off { 0 } else { 1 }));
        out.push_str(&format!("option\tautosave_every\t{}\n", self.autosave_secs));
        out.push_str(&format!("option\tviewport_msaa\t{}\n", self.viewport_msaa));
        out.push_str(&format!("option\tshow_stats\t{}\n", self.show_stats as u8));
        if let Some(d) = &self.export_dir {
            out.push_str(&format!("option\texport_dir\t{}\n", d.display()));
        }
        let e = self.export;
        out.push_str(&format!("option\texport\t{},{},{},{},{},{}\n", e[0], e[1], e[2], e[3], e[4], e[5]));
        out.push_str(&format!("option\tai_denoise\t{}\n", self.ai_denoise as u8));
        out.push_str(&format!("option\tanimation\t{}\n", self.animation));
        out.push_str(&format!("option\tlock_layout\t{}\n", self.lock_layout as u8));
        if let Some([r, g, b]) = self.highlight {
            out.push_str(&format!("option\thighlight\t{r:02X}{g:02X}{b:02X}\n"));
        }
        for (key, [r, g, b]) in [
            ("loading_fill", self.loading_fill),
            ("loading_outline", self.loading_outline),
            ("selection_outline", self.selection_outline),
        ] {
            out.push_str(&format!("option\t{key}\t{r:02X}{g:02X}{b:02X}\n"));
        }
        out.push_str(&format!("option	quick_pinned	{}
", self.quick_pinned as u8));
        if let Some([x, y]) = self.quick_pos {
            out.push_str(&format!("option	quick_pos	{x:.0},{y:.0}
"));
        }
        if !self.quick_sections.is_empty() {
            out.push_str(&format!("option\tquick_sections\t{}\n", self.quick_sections));
        }
        let a = self.assets_view;
        out.push_str(&format!("option\tassets_view\t{},{},{},{},{}\n", a[0], a[1], a[2], a[3], a[4]));
        for p in &self.recent_projects {
            out.push_str(&format!("recent_project\t{}\n", p.display()));
        }
        for [r, g, b] in &self.recent_colors {
            out.push_str(&format!("recent_color	{r:02X}{g:02X}{b:02X}
"));
        }
        for v in &self.recent_brushes {
            out.push_str(&format!("recent_brush	{},{},{},{},{}
", v[0], v[1], v[2], v[3], v[4]));
        }
        for l in &self.libraries {
            out.push_str(&format!("library\t{}\t{}\t{}\n", l.name, l.default as u8, l.path.display()));
        }
        let _ = std::fs::write(p, out);
    }

    /// Puts `path` first in the recent projects (eight kept).
    pub fn note_recent(&mut self, path: &Path) {
        let p = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let p = PathBuf::from(p.display().to_string().trim_start_matches(r"\\?\"));
        self.recent_projects.retain(|q| *q != p);
        self.recent_projects.insert(0, p);
        self.recent_projects.truncate(8);
    }

    pub fn add_library(&mut self, name: &str, path: &Path) -> bool {
        if self.libraries.iter().any(|l| l.path == path) {
            return false;
        }
        let first = self.libraries.is_empty();
        self.libraries.push(Library { name: name.to_string(), path: path.to_path_buf(), default: first });
        true
    }

    pub fn remove_library(&mut self, i: usize) {
        if i < self.libraries.len() {
            let was_default = self.libraries.remove(i).default;
            if was_default {
                if let Some(l) = self.libraries.first_mut() {
                    l.default = true;
                }
            }
        }
    }

    pub fn set_default(&mut self, i: usize) {
        for (k, l) in self.libraries.iter_mut().enumerate() {
            l.default = k == i;
        }
    }
}

/// Library folders that exist on this machine, to offer as one-click additions (never added
/// automatically): the Painter installation's starter assets and the user's own assets folder.
pub fn detect_painter_libraries() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let mut try_add = |name: &str, p: PathBuf| {
        if p.is_dir() {
            out.push((name.to_string(), p));
        }
    };
    for pf in ["C:\\Program Files\\Adobe\\Adobe Substance 3D Painter", "C:\\Program Files (x86)\\Adobe\\Adobe Substance 3D Painter"] {
        try_add("starter_assets", PathBuf::from(pf).join("resources").join("starter_assets"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        let docs = PathBuf::from(home).join("Documents").join("Adobe").join("Adobe Substance 3D Painter");
        try_add("your_assets", docs.join("assets"));
    }
    out
}
