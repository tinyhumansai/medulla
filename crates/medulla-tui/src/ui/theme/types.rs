//! Data types for the `theme` module.
#[allow(unused_imports)]
use super::*;

fn pick_gradient_color(value: &Option<String>, fallback: Option<Color>) -> Option<Color> {
    match value {
        None => fallback,
        Some(value) if value.trim().is_empty() => None,
        Some(value) => parse_color(value).or(fallback),
    }
}
/// The resolved color roles used across the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Selection/highlight background, brand, panel titles, and primary accents.
    pub primary: Color,
    /// Secondary accent for inline overlays (prompt/resume borders).
    pub accent: Color,
    /// Foreground drawn on top of `primary` for selected rows.
    pub selection_fg: Color,
    /// Dim panel border color.
    pub dim_border: Color,
    /// Foreground used for cues that require operator attention.
    pub attention: Color,
    /// Whether cues that require operator attention blink.
    pub attention_blink: bool,
    /// Length of one attention pulse in milliseconds — bright for the first
    /// half, dim for the second.
    ///
    /// Held in milliseconds rather than the seconds the operator configures so
    /// [`Theme`] stays [`Eq`]: the pulse is compared for equality all over the
    /// render tests, and a float field would make that comparison partial.
    pub attention_blink_ms: u64,
    /// Colors and gradients used by resource panels and charts.
    pub resources: ResourcesTheme,
}

/// btop-style colors for resource panels and their graphs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourcesTheme {
    pub cpu_box: Color,
    pub mem_box: Color,
    pub net_box: Color,
    pub proc_box: Color,
    pub div_line: Color,
    pub meter_bg: Color,
    pub graph_text: Color,
    pub inactive_fg: Color,
    pub cpu: Gradient,
    pub temp: Gradient,
    pub used: Gradient,
    pub free: Gradient,
    pub cached: Gradient,
    pub available: Gradient,
    pub download: Gradient,
    pub upload: Gradient,
    pub process: Gradient,
}

impl Default for ResourcesTheme {
    fn default() -> Self {
        let color = |hex| parse_color(hex).expect("valid built-in theme color");
        Self {
            cpu_box: color("#556d59"),
            mem_box: color("#6c6c4b"),
            net_box: color("#5c588d"),
            proc_box: color("#805252"),
            div_line: color("#303030"),
            meter_bg: color("#404040"),
            graph_text: color("#606060"),
            inactive_fg: color("#404040"),
            cpu: Gradient::new(
                color("#77ca9b"),
                Some(color("#cbc06c")),
                Some(color("#dc4c4c")),
            ),
            temp: Gradient::new(
                color("#4897d4"),
                Some(color("#5474e8")),
                Some(color("#ff40b6")),
            ),
            used: Gradient::new(
                color("#592b26"),
                Some(color("#d9626d")),
                Some(color("#ff4769")),
            ),
            free: Gradient::new(
                color("#384f21"),
                Some(color("#b5e685")),
                Some(color("#dcff85")),
            ),
            cached: Gradient::new(
                color("#163350"),
                Some(color("#74e6fc")),
                Some(color("#26c5ff")),
            ),
            available: Gradient::new(
                color("#4e3f0e"),
                Some(color("#ffd77a")),
                Some(color("#ffb814")),
            ),
            download: Gradient::new(
                color("#291f75"),
                Some(color("#4f43a3")),
                Some(color("#b0a9de")),
            ),
            upload: Gradient::new(
                color("#620665"),
                Some(color("#7d4180")),
                Some(color("#dcafde")),
            ),
            process: Gradient::new(
                color("#80d0a3"),
                Some(color("#dcd179")),
                Some(color("#d45454")),
            ),
        }
    }
}

impl ResourcesTheme {
    pub(crate) fn from_config(cfg: &medulla::config::ResourcesThemeConfig) -> Self {
        let defaults = Self::default();
        let pick = |value: &Option<String>, fallback: Color| {
            value.as_deref().and_then(parse_color).unwrap_or(fallback)
        };
        Self {
            cpu_box: pick(&cfg.cpu_box, defaults.cpu_box),
            mem_box: pick(&cfg.mem_box, defaults.mem_box),
            net_box: pick(&cfg.net_box, defaults.net_box),
            proc_box: pick(&cfg.proc_box, defaults.proc_box),
            div_line: pick(&cfg.div_line, defaults.div_line),
            meter_bg: pick(&cfg.meter_bg, defaults.meter_bg),
            graph_text: pick(&cfg.graph_text, defaults.graph_text),
            inactive_fg: pick(&cfg.inactive_fg, defaults.inactive_fg),
            cpu: Gradient::new(
                pick(&cfg.cpu_start, defaults.cpu.start),
                pick_gradient_color(&cfg.cpu_mid, defaults.cpu.mid),
                pick_gradient_color(&cfg.cpu_end, defaults.cpu.end),
            ),
            temp: Gradient::new(
                pick(&cfg.temp_start, defaults.temp.start),
                pick_gradient_color(&cfg.temp_mid, defaults.temp.mid),
                pick_gradient_color(&cfg.temp_end, defaults.temp.end),
            ),
            used: Gradient::new(
                pick(&cfg.used_start, defaults.used.start),
                pick_gradient_color(&cfg.used_mid, defaults.used.mid),
                pick_gradient_color(&cfg.used_end, defaults.used.end),
            ),
            free: Gradient::new(
                pick(&cfg.free_start, defaults.free.start),
                pick_gradient_color(&cfg.free_mid, defaults.free.mid),
                pick_gradient_color(&cfg.free_end, defaults.free.end),
            ),
            cached: Gradient::new(
                pick(&cfg.cached_start, defaults.cached.start),
                pick_gradient_color(&cfg.cached_mid, defaults.cached.mid),
                pick_gradient_color(&cfg.cached_end, defaults.cached.end),
            ),
            available: Gradient::new(
                pick(&cfg.available_start, defaults.available.start),
                pick_gradient_color(&cfg.available_mid, defaults.available.mid),
                pick_gradient_color(&cfg.available_end, defaults.available.end),
            ),
            download: Gradient::new(
                pick(&cfg.download_start, defaults.download.start),
                pick_gradient_color(&cfg.download_mid, defaults.download.mid),
                pick_gradient_color(&cfg.download_end, defaults.download.end),
            ),
            upload: Gradient::new(
                pick(&cfg.upload_start, defaults.upload.start),
                pick_gradient_color(&cfg.upload_mid, defaults.upload.mid),
                pick_gradient_color(&cfg.upload_end, defaults.upload.end),
            ),
            process: Gradient::new(
                pick(&cfg.process_start, defaults.process.start),
                pick_gradient_color(&cfg.process_mid, defaults.process.mid),
                pick_gradient_color(&cfg.process_end, defaults.process.end),
            ),
        }
    }

    /// Convert the resource palette and its gradients for a terminal's color depth.
    pub fn with_depth(self, depth: ColorDepth) -> Self {
        let convert = |color| depth.convert(color);
        Self {
            cpu_box: convert(self.cpu_box),
            mem_box: convert(self.mem_box),
            net_box: convert(self.net_box),
            proc_box: convert(self.proc_box),
            div_line: convert(self.div_line),
            meter_bg: convert(self.meter_bg),
            graph_text: convert(self.graph_text),
            inactive_fg: convert(self.inactive_fg),
            cpu: self.cpu.with_depth(depth),
            temp: self.temp.with_depth(depth),
            used: self.used.with_depth(depth),
            free: self.free.with_depth(depth),
            cached: self.cached.with_depth(depth),
            available: self.available.with_depth(depth),
            download: self.download.with_depth(depth),
            upload: self.upload.with_depth(depth),
            process: self.process.with_depth(depth),
        }
    }
}
