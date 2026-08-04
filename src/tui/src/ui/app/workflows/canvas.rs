//! The graph cache and the cursor that walks it.
//!
//! The graph is read from the store and laid out once per selection change, not
//! once per frame: the store is files, and re-laying out on every repaint would
//! move boxes under the operator's cursor as soon as anything shifted a lane.
//!
//! The cursor is an index into the layout's reading order, and moving it is the
//! SDK's judgement ([`medulla::ui::workflows::GraphLayout::moved`]) — following
//! edges rather than jumping columns. What this module adds is the *viewport*:
//! the fold that wraps a long chain onto the next band down so the graph is
//! never wider than its pane, and the vertical scroll that keeps the selected
//! node on screen.

use medulla::ui::workflows::{GraphLayout, Move, PlacedNode};

use super::super::render::workflows::{
    BAND_GAP, FOLD_MARGIN, GUTTER_SPAN, LANE_STRIDE, MAX_NODE_WIDTH, MIN_LABEL_WIDTH, NODE_HEIGHT,
};
use super::super::types::App;
use unicode_width::UnicodeWidthStr;

impl App {
    /// Re-read the selected workflow's graph and lay it out.
    ///
    /// A workflow that cannot be read leaves the canvas empty rather than
    /// keeping the previous one's graph on screen under the new one's name,
    /// which would be the most misleading thing this pane could do.
    pub(in crate::ui::app) fn reload_workflow_graph(&mut self) {
        let Some(id) = self.selected_workflow().map(|workflow| workflow.id.clone()) else {
            self.wf.graph = None;
            self.wf.defaults = Default::default();
            self.wf.layout = GraphLayout::default();
            return;
        };
        match self.workflow_store().get(&id) {
            Ok(Some(record)) => {
                self.wf.layout = GraphLayout::build(&record.graph);
                self.wf.defaults = record.defaults;
                self.wf.graph = Some(Box::new(record.graph));
            }
            _ => {
                self.wf.graph = None;
                self.wf.defaults = Default::default();
                self.wf.layout = GraphLayout::default();
            }
        }
        self.wf.node_index = self
            .wf
            .node_index
            .min(self.wf.layout.nodes.len().saturating_sub(1));
    }

    /// The laid-out graph currently on the canvas.
    pub(in crate::ui::app) fn workflow_layout(&self) -> &GraphLayout {
        &self.wf.layout
    }

    /// The node under the canvas cursor.
    pub(in crate::ui::app) fn selected_graph_node(&self) -> Option<&PlacedNode> {
        self.wf.layout.node(self.wf.node_index)
    }

    /// Move the canvas cursor, keeping it on screen.
    ///
    /// A move with nowhere to go leaves the cursor where it is rather than
    /// wrapping: the graph has a first and a last step, and wrapping from one to
    /// the other reads as the cursor having jumped rather than stopped.
    pub(in crate::ui::app) fn move_graph_cursor(&mut self, direction: Move) {
        if let Some(next) = self.wf.layout.moved(self.wf.node_index, direction) {
            self.wf.node_index = next;
            self.wf.preview_scroll = 0;
            self.scroll_canvas_to_cursor();
        }
    }

    /// Scroll the canvas so the selected node is inside the viewport.
    ///
    /// Only vertically: the graph folds to the pane's width, so a node is never
    /// off to the side — a later layer is further *down*, on the next band.
    fn scroll_canvas_to_cursor(&mut self) {
        let Some(node) = self.wf.layout.node(self.wf.node_index) else {
            return;
        };
        let (layer, lane) = (node.layer, node.lane);
        let (_, row) = self.graph_cell(layer, lane);
        let rows = self.visible_rows();
        if row < self.wf.canvas_row {
            self.wf.canvas_row = row;
        } else if row + NODE_HEIGHT > self.wf.canvas_row + rows {
            self.wf.canvas_row = row + NODE_HEIGHT - rows;
        }
    }

    /// Where a node sits on the unscrolled canvas, in cells.
    ///
    /// This is the fold: layers run across the pane until one would leave it,
    /// then the graph continues on a band below. Alternate bands run *backwards*
    /// — boustrophedon, the way an ox ploughs a field — so a chain that wraps
    /// picks up directly under where it left off instead of throwing a wire all
    /// the way back across the pane to start again at the left margin.
    ///
    /// Shared with the renderer so navigation and drawing can never disagree
    /// about where a node is.
    pub(in crate::ui::app) fn graph_cell(&self, layer: usize, lane: usize) -> (usize, usize) {
        (
            FOLD_MARGIN + self.column_x(layer),
            self.band_top(self.band_of(layer)) + lane * LANE_STRIDE,
        )
    }

    /// The band a layer folds onto.
    pub(in crate::ui::app) fn band_of(&self, layer: usize) -> usize {
        layer / self.layers_per_band()
    }

    /// Whether a band runs right to left.
    pub(in crate::ui::app) fn band_reversed(&self, band: usize) -> bool {
        band % 2 == 1
    }

    /// Whether the layer at `layer` sits on a band that runs right to left.
    pub(in crate::ui::app) fn layer_reversed(&self, layer: usize) -> bool {
        self.band_reversed(self.band_of(layer))
    }

    /// The row each band starts on.
    ///
    /// Bands are only as tall as the lanes they actually use, rather than as
    /// tall as the widest band in the graph: one branch three lanes deep should
    /// not put two blank rows under every other band in the fold.
    pub(in crate::ui::app) fn band_top(&self, band: usize) -> usize {
        self.band_heights().into_iter().take(band).sum()
    }

    /// The height of every band, in rows, in order.
    pub(in crate::ui::app) fn band_heights(&self) -> Vec<usize> {
        let per_band = self.layers_per_band();
        let bands = self.wf.layout.layers.max(1).div_ceil(per_band);
        let mut lanes = vec![1usize; bands];
        for node in &self.wf.layout.nodes {
            if let Some(slot) = lanes.get_mut(node.layer / per_band) {
                *slot = (*slot).max(node.lane + 1);
            }
        }
        lanes
            .into_iter()
            .map(|lanes| lanes * LANE_STRIDE + BAND_GAP)
            .collect()
    }

    /// How many rows the whole folded graph occupies.
    pub(in crate::ui::app) fn folded_rows(&self) -> usize {
        // The last band needs no trailing gap, so one is taken back off.
        self.band_heights()
            .into_iter()
            .sum::<usize>()
            .saturating_sub(BAND_GAP)
    }

    /// How wide the canvas is, in cells, inside the panel's borders and the
    /// margin the folded wires turn in.
    ///
    /// Arithmetic rather than a measurement, so the key handler can fold and
    /// scroll without waiting for a frame to have been drawn. The sidebar is
    /// measured exactly the way the layout measures it; everything left of the
    /// panel's own borders is canvas, because the content pane now holds one
    /// view rather than sharing the row with a copilot column.
    pub(in crate::ui::app) fn canvas_width(&self) -> usize {
        const BORDERS: usize = 2;
        let rail = self.workflow_sidebar_width(self.area.width);
        (self.area.width as usize)
            .saturating_sub(rail as usize)
            .saturating_sub(BORDERS)
            .saturating_sub(FOLD_MARGIN)
            .max(MIN_LABEL_WIDTH)
    }

    /// How the canvas is divided into columns: how many layers a band holds,
    /// and the width of every layer's own column.
    ///
    /// Every column is sized to its own content — the longest label of any node
    /// in that layer — rather than every column taking the width of the graph's
    /// longest label. One wordy step used to widen every column in the fold,
    /// and the difference came out as connector: a short name followed by
    /// twenty cells of `─` before the next step.
    fn column_layout(&self) -> (usize, Vec<usize>) {
        let canvas = self.canvas_width();
        let layers = self.wf.layout.layers.max(1);
        let mut widths = vec![MIN_LABEL_WIDTH; layers];
        for node in &self.wf.layout.nodes {
            if let Some(slot) = widths.get_mut(node.layer) {
                // Plus one for the run-state glyph a run overlay appends, so
                // overlaying a run cannot change a column's width under the
                // reader.
                let label = node_label(node).width() + 1;
                *slot = (*slot).max(label.min(MAX_NODE_WIDTH));
            }
        }
        // The most columns the narrowest possible label would allow is the
        // ceiling; the first count at or below it whose own bands all fit is
        // the answer, so a band takes as many columns as its labels permit.
        let ceiling = (canvas / (MIN_LABEL_WIDTH + GUTTER_SPAN)).clamp(1, layers);
        for per_band in (1..=ceiling).rev() {
            if widths
                .chunks(per_band)
                .all(|band| band_width(band) <= canvas)
            {
                return (per_band, widths);
            }
        }
        (1, widths)
    }

    /// How many layers fit across the canvas before it folds.
    pub(in crate::ui::app) fn layers_per_band(&self) -> usize {
        self.column_layout().0
    }

    /// The columns the label of a node in `layer` is drawn in.
    pub(in crate::ui::app) fn column_width(&self, layer: usize) -> usize {
        let (_, widths) = self.column_layout();
        widths.get(layer).copied().unwrap_or(MIN_LABEL_WIDTH)
    }

    /// The left edge of a layer's column, relative to the canvas.
    ///
    /// Forward bands are laid from the left; reversed ones are laid from the
    /// right, so both end flush against the edge the fold between them turns
    /// at. Sizing each column to its own label and then packing the band this
    /// way is what keeps the connectors short: a wire only ever crosses the
    /// gutter, never the padding of a column sized for some other step's name.
    fn column_x(&self, layer: usize) -> usize {
        let (per_band, widths) = self.column_layout();
        let band = layer / per_band;
        let members = widths
            .chunks(per_band)
            .nth(band)
            .unwrap_or(&widths[..1.min(widths.len())]);
        let index = layer % per_band;
        let before: usize = members
            .iter()
            .take(index)
            .map(|width| width + GUTTER_SPAN)
            .sum();
        if !self.band_reversed(band) {
            return before;
        }
        // Right to left, ending at the same edge every reversed band ends at:
        // the widest band's. A band laid out within its own extent would start
        // wherever its own columns happened to reach — a short last band well
        // to the left of the one above it, turning a fold that should be a hop
        // straight down into a run back across the pane, and a band wider than
        // that extent overlapping its own labels.
        let extent = widths
            .chunks(per_band)
            .map(band_width)
            .max()
            .unwrap_or_else(|| band_width(members));
        extent.saturating_sub(before + members.get(index).copied().unwrap_or(0))
    }

    /// How many rows of canvas the graph panel has.
    pub(in crate::ui::app) fn visible_rows(&self) -> usize {
        if self.wf.graph_rows > 0 {
            return self.wf.graph_rows.max(1);
        }
        // Header, tab bar, hint row, footer, and the panel's own borders. No
        // measured graph exists before the first frame, so this fallback keeps
        // pre-render navigation safe. Every later move uses the exact inner
        // graph rectangle recorded by the renderer.
        const CHROME: usize = 9;
        (self.area.height as usize).saturating_sub(CHROME).max(1)
    }
}

/// The cells a band of these column widths occupies, gutters included.
fn band_width(columns: &[usize]) -> usize {
    columns
        .iter()
        .map(|width| width + GUTTER_SPAN)
        .sum::<usize>()
        // Nothing follows the last column, so it needs no gutter of its own.
        .saturating_sub(GUTTER_SPAN)
}
