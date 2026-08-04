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
    BAND_GAP, FOLD_MARGIN, LANE_STRIDE, LAYER_STRIDE, NODE_HEIGHT,
};
use super::super::types::App;

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
    /// This is the fold: layers run left to right until one would leave the
    /// pane, then the graph continues on a band below, the way a paragraph
    /// wraps. Shared with the renderer so navigation and drawing can never
    /// disagree about where a node is.
    pub(in crate::ui::app) fn graph_cell(&self, layer: usize, lane: usize) -> (usize, usize) {
        let per_band = self.layers_per_band();
        let (band, column) = (layer / per_band, layer % per_band);
        (
            FOLD_MARGIN + column * LAYER_STRIDE,
            band * self.band_stride() + lane * LANE_STRIDE,
        )
    }

    /// The band a layer folds onto.
    pub(in crate::ui::app) fn band_of(&self, layer: usize) -> usize {
        layer / self.layers_per_band()
    }

    /// How many layers fit across the canvas before it folds.
    pub(in crate::ui::app) fn layers_per_band(&self) -> usize {
        self.visible_layers()
    }

    /// Rows from one band's top edge to the next: every lane the graph has,
    /// plus a blank row for the wires that fold back to the left to run in.
    pub(in crate::ui::app) fn band_stride(&self) -> usize {
        self.wf.layout.lanes.max(1) * LANE_STRIDE + BAND_GAP
    }

    /// How many layers the canvas can show at the current terminal width.
    ///
    /// A node plus the gutter between columns is a known number of cells, so
    /// this is arithmetic rather than a measurement — which means the key
    /// handler can fold and scroll without waiting for a frame to have been
    /// drawn. The sidebar is measured exactly the way the layout measures it;
    /// everything left of the panel's own borders is canvas, because the content
    /// pane now holds one view rather than sharing the row with a copilot
    /// column.
    pub(in crate::ui::app) fn visible_layers(&self) -> usize {
        const BORDERS: usize = 2;
        let rail = self.workflow_sidebar_width(self.area.width);
        let canvas = (self.area.width as usize)
            .saturating_sub(rail as usize)
            .saturating_sub(BORDERS)
            .saturating_sub(FOLD_MARGIN);
        (canvas / super::super::render::workflows::LAYER_STRIDE).max(1)
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
