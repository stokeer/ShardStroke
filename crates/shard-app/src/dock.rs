//! The windows laid over the 3D viewport (Painter's "views"): Properties, Assets, Texture Set List, Layers and
//! Texture Set Settings are tabs of an `egui_dock` tree. They can be dragged to any edge of another window
//! (split), dropped on a tab bar (tab group) or torn off as a floating window; the arrangement is saved per
//! user and restored at the next start.
//!
//! The viewport is *not* one of them: it is the background of the program, fixed, drawn under everything.
//! The empty leaf of the tree (no tabs, so egui_dock draws nothing for it) only marks the free area the
//! windows leave open, where the scene is centred and the brush works.

use crate::theme;
use egui::{Color32, CornerRadius, Margin, Rect, Stroke};
use egui_dock::{DockState, Node, NodeIndex, OverlayType, Style, SurfaceIndex};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum Pane {
    Properties,
    Assets,
    TextureSetList,
    Layers,
    TextureSetSettings,
    DisplaySettings,
    ShaderSettings,
    History,
    Log,
}

impl Pane {
    /// Every window, in the order of the Window > Views menu.
    pub const ALL: [Pane; 9] = [
        Pane::Properties,
        Pane::Assets,
        Pane::TextureSetList,
        Pane::Layers,
        Pane::TextureSetSettings,
        Pane::DisplaySettings,
        Pane::ShaderSettings,
        Pane::History,
        Pane::Log,
    ];

    /// The windows of the default layout (the others open from the right strip or Window > Views).
    pub const DEFAULT: [Pane; 5] = [Pane::Properties, Pane::Assets, Pane::TextureSetList, Pane::Layers, Pane::TextureSetSettings];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Properties => "Properties",
            Pane::Assets => "Assets",
            Pane::TextureSetList => "Texture Set List",
            Pane::Layers => "Layers",
            Pane::TextureSetSettings => "Texture Set Settings",
            Pane::DisplaySettings => "Display Settings",
            Pane::ShaderSettings => "Shader Settings",
            Pane::History => "History",
            Pane::Log => "Log",
        }
    }
}

/// Painter's default arrangement: properties + assets on the left, the free viewport area in the middle,
/// texture set list over layers + texture set settings on the right.
pub fn default_layout() -> DockState<Pane> {
    // the root leaf starts with a placeholder tab and is emptied: it becomes the viewport's free area
    let mut state = DockState::new(vec![Pane::Assets]);
    let tree = state.main_surface_mut();
    if let Node::Leaf { tabs, .. } = &mut tree[NodeIndex::root()] {
        tabs.clear();
    }
    // egui_dock: the fraction is the share of the left / top node (the new one for `split_left`, the old
    // one for `split_right` / `split_below`): 20 % left, then 25 % of the window (31 % of the rest) right
    let [centre, _left] = tree.split_left(NodeIndex::root(), 0.20, vec![Pane::Properties, Pane::Assets]);
    // Display / Shader Settings, History and Log open on demand from the right strip (tabs beside the layers): in
    // the default columns more tabs overflow the tab bar, which then scrolls
    let [_centre, right] = tree.split_right(centre, 0.6875, vec![Pane::TextureSetList]);
    tree.split_below(right, 0.24, vec![Pane::Layers, Pane::TextureSetSettings]);
    state
}

fn is_free_area(n: &Node<Pane>) -> bool {
    n.is_leaf() && n.tabs_count() == 0
}

/// The free area left open between the windows: where the scene is centred and the brush paints.
pub fn spacer_rect(state: &DockState<Pane>) -> Option<Rect> {
    state.main_surface().iter().find(|n| is_free_area(n)).and_then(|n| n.rect())
}

/// Bring `pane` to front, or re-open it (next to the layers, else in the first window) when it was closed.
pub fn show(state: &mut DockState<Pane>, pane: Pane) {
    if let Some(at) = state.find_tab(&pane) {
        let _ = state.set_active_tab(at);
        return;
    }
    let tree = state.main_surface();
    let with_tabs = |i: usize| tree[NodeIndex(i)].is_leaf() && tree[NodeIndex(i)].tabs_count() > 0;
    let with_layers = state
        .find_tab(&Pane::Layers)
        .filter(|(s, _, _)| s.is_main())
        .map(|(_, n, _)| n.0);
    match with_layers.or_else(|| (0..tree.len()).find(|i| with_tabs(*i))) {
        Some(i) => {
            state.set_focused_node_and_surface((SurfaceIndex::main(), NodeIndex(i)));
            state.push_to_focused_leaf(pane);
        }
        None => {
            state.add_window(vec![pane]);
        }
    }
}

/// Put back in the main surface every window that a saved arrangement keeps floating (torn off by an accidental
/// drag): the program starts with its windows docked; floating one is a choice of the session.
pub fn dock_floating(state: &mut DockState<Pane>) {
    for pane in Pane::ALL {
        let Some((surface, node, tab)) = state.find_tab(&pane) else { continue };
        if surface.is_main() {
            continue;
        }
        state.remove_tab((surface, node, tab));
        show(state, pane);
    }
}

fn layout_path() -> Option<std::path::PathBuf> {
    Some(crate::config::config_dir()?.join("layout.json"))
}

/// The saved arrangement, if there is one.
pub fn load_layout() -> Option<DockState<Pane>> {
    parse_layout(&std::fs::read_to_string(layout_path()?).ok()?)
}

pub fn parse_layout(json: &str) -> Option<DockState<Pane>> {
    // the rectangles of a fresh tree are infinite (`Rect::NOTHING`), which JSON writes as null
    fn finite(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(m) => {
                for key in ["x", "y"] {
                    if m.get(key).is_some_and(|c| c.is_null()) {
                        m.insert(key.into(), 0.0.into());
                    }
                }
                m.values_mut().for_each(finite);
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(finite),
            _ => {}
        }
    }
    let mut value: serde_json::Value = serde_json::from_str(json).ok()?;
    finite(&mut value);
    let state: DockState<Pane> = serde_json::from_value(value).ok()?;
    // a layout without the free area (an older file) is not usable: the viewport would be fully covered
    state.main_surface().iter().any(|n| is_free_area(n)).then_some(state)
}

pub fn layout_json(state: &DockState<Pane>) -> Option<String> {
    serde_json::to_string_pretty(state).ok()
}

pub fn save_layout_json(json: &str) {
    let Some(p) = layout_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, json);
}

/// Look of the docks: translucent rounded cards over the viewport, tab bars as folder tabs.
pub fn style(ctx: &egui::Context) -> Style {
    let mut s = Style::from_egui(ctx.style().as_ref());
    let card = theme::RADIUS_CARD;
    let top = |r: u8| CornerRadius { nw: r, ne: r, sw: 0, se: 0 };
    let bottom = |r: u8| CornerRadius { nw: 0, ne: 0, sw: r, se: r };
    let hl = theme::accent_rgb();
    s.dock_area_padding = Some(Margin::same(theme::GAP as i8));
    s.main_surface_border_stroke = Stroke::NONE;
    s.main_surface_border_rounding = CornerRadius::ZERO;
    // the gap between two cards is the separator: invisible (the viewport shows through) until hovered
    s.separator.width = theme::GAP;
    s.separator.extra_interact_width = 2.0;
    s.separator.color_idle = Color32::TRANSPARENT;
    s.separator.color_hovered = Color32::from_rgba_unmultiplied(hl[0], hl[1], hl[2], 90);
    s.separator.color_dragged = Color32::from_rgba_unmultiplied(hl[0], hl[1], hl[2], 170);
    s.tab_bar.bg_fill = theme::card_alt();
    s.tab_bar.height = 30.0;
    s.tab_bar.corner_radius = top(card);
    s.tab_bar.hline_color = theme::BORDER;
    // the active tab shares the colour of the card body, the others sink into the bar
    s.tab.hline_below_active_tab_name = false;
    s.tab.minimum_width = None;
    for (st, bg, text) in [
        (&mut s.tab.inactive, theme::card_alt(), theme::TEXT_DIM),
        (&mut s.tab.hovered, theme::WIDGET, Color32::WHITE),
        (&mut s.tab.active, theme::card(), Color32::WHITE),
        (&mut s.tab.focused, theme::card(), Color32::WHITE),
        (&mut s.tab.active_with_kb_focus, theme::card(), Color32::WHITE),
        (&mut s.tab.focused_with_kb_focus, theme::card(), Color32::WHITE),
        (&mut s.tab.inactive_with_kb_focus, theme::card_alt(), theme::TEXT_DIM),
    ] {
        st.bg_fill = bg;
        st.outline_color = Color32::TRANSPARENT;
        st.text_color = text;
        st.corner_radius = top(8);
    }
    s.tab.tab_body.bg_fill = theme::card();
    s.tab.tab_body.stroke = Stroke::NONE;
    s.tab.tab_body.corner_radius = bottom(card);
    s.tab.tab_body.inner_margin = Margin::same(8);
    s.buttons.close_tab_color = theme::TEXT_DIM;
    s.buttons.close_tab_active_color = Color32::WHITE;
    s.buttons.close_tab_bg_fill = theme::WIDGET_HOVER;
    // drop targets as a cross of buttons (left / right / top / bottom / tab group) over the hovered window,
    // anywhere else floats the window: clearer than invisible zones
    s.overlay.overlay_type = OverlayType::Widgets;
    s.overlay.selection_color = Color32::from_rgba_unmultiplied(hl[0], hl[1], hl[2], 70);
    s.overlay.hovered_leaf_highlight.corner_radius = CornerRadius::same(card);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_has_every_window_once_and_the_free_area() {
        let state = default_layout();
        for p in Pane::DEFAULT {
            assert_eq!(state.iter_all_tabs().filter(|(_, t)| **t == p).count(), 1, "{p:?}");
        }
        let empty = state.main_surface().iter().filter(|n| is_free_area(n)).count();
        assert_eq!(empty, 1, "exactly one free area");
    }

    /// The windows outside the default layout (Display / Shader Settings, History, Log) open once each, docked
    /// beside the layers, and the free area stays.
    #[test]
    fn on_demand_windows_open_beside_the_layers() {
        let mut state = default_layout();
        for p in Pane::ALL.into_iter().filter(|p| !Pane::DEFAULT.contains(p)) {
            show(&mut state, p);
            show(&mut state, p);
            assert_eq!(state.iter_all_tabs().filter(|(_, t)| **t == p).count(), 1, "{p:?}");
            let (s, n, _) = state.find_tab(&p).unwrap();
            assert!(s.is_main());
            assert_eq!(Some(n), state.find_tab(&Pane::Layers).map(|t| t.1), "{p:?} beside the layers");
        }
        assert!(state.main_surface().iter().any(|n| is_free_area(n)));
    }

    #[test]
    fn layout_round_trips_and_needs_the_free_area() {
        let state = default_layout();
        let json = layout_json(&state).unwrap();
        let back = parse_layout(&json).expect("round trip");
        assert_eq!(back.iter_all_tabs().count(), state.iter_all_tabs().count());
        assert!(back.main_surface().iter().any(|n| is_free_area(n)));
        // a layout from before the viewport became the background (no free area) is refused
        let mut full = DockState::new(vec![Pane::Layers]);
        full.main_surface_mut().split_left(NodeIndex::root(), 0.5, vec![Pane::Assets]);
        assert!(parse_layout(&layout_json(&full).unwrap()).is_none());
        assert!(parse_layout("not json").is_none());
    }

    #[test]
    fn floating_windows_are_docked_at_start() {
        let mut state = default_layout();
        let at = state.find_tab(&Pane::TextureSetSettings).unwrap();
        state.remove_tab(at);
        state.add_window(vec![Pane::TextureSetSettings]);
        assert!(!state.find_tab(&Pane::TextureSetSettings).unwrap().0.is_main());
        dock_floating(&mut state);
        let (surface, _, _) = state.find_tab(&Pane::TextureSetSettings).unwrap();
        assert!(surface.is_main(), "docked again");
        assert!(state.main_surface().iter().any(|n| is_free_area(n)), "free area kept");
        for p in Pane::DEFAULT {
            assert_eq!(state.iter_all_tabs().filter(|(_, t)| **t == p).count(), 1, "{p:?}");
        }
    }

    #[test]
    fn a_closed_window_comes_back_and_the_free_area_stays() {
        let mut state = default_layout();
        let at = state.find_tab(&Pane::Assets).unwrap();
        state.remove_tab(at);
        assert!(state.find_tab(&Pane::Assets).is_none());
        show(&mut state, Pane::Assets);
        assert!(state.find_tab(&Pane::Assets).is_some());
        assert!(state.main_surface().iter().any(|n| is_free_area(n)), "free area kept");
    }
}
