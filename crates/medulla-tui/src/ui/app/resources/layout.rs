use super::types::{BoxRects, LayoutOptions, ShownBoxes};
use ratatui::layout::Rect;

/// Calculate btop-style panel rectangles within the content area.
pub(super) fn calc_sizes(area: Rect, shown: ShownBoxes, opts: &LayoutOptions) -> BoxRects {
    let mut out = BoxRects::default();
    if shown.cpu && (area.width < 60 || area.height < 8) {
        out.too_small = Some(60);
        return out;
    }
    if shown.cpu && !shown.mem && !shown.net && !shown.agents {
        out.cpu = area;
        return out;
    }
    let rest = if shown.cpu {
        let h = (u32::from(area.height) * 32)
            .div_ceil(100)
            .max(8)
            .min(u32::from(area.height)) as u16;
        let cpu_y = if opts.cpu_bottom {
            area.bottom() - h
        } else {
            area.y
        };
        out.cpu = Rect::new(area.x, cpu_y, area.width, h);
        if opts.cpu_bottom {
            Rect::new(area.x, area.y, area.width, area.height - h)
        } else {
            Rect::new(area.x, area.y + h, area.width, area.height - h)
        }
    } else {
        area
    };
    if shown.agents && (rest.width < 44 || rest.height < 16) {
        if rest.width >= 44 && rest.height >= 16 {
            out.agents = rest;
            return out;
        }
        out.too_small = Some(44);
        return out;
    }
    if shown.agents && (u32::from(rest.width) * 55 / 100 < 44 || rest.height < 16) {
        // Agents has priority over Mem and Net. When the normal split would
        // make its panel unusable, retain it and suppress the lower-priority
        // panels as complete boxes.
        out.agents = rest;
        return out;
    }
    if shown.agents && shown.mem && shown.net {
        let left_w = (u32::from(rest.width) * 45).div_ceil(100) as u16;
        let right_w = rest.width.saturating_sub(left_w);
        let mem_h = if shown.cpu {
            u32::from(area.height) * 40 / 100
        } else {
            u32::from(rest.height) * 72 / 100
        } as u16;
        let net_h = rest.height.saturating_sub(mem_h);
        if opts.agents_left {
            out.agents = Rect::new(rest.x, rest.y, right_w, rest.height);
            out.mem = Rect::new(
                rest.x + right_w,
                if opts.mem_below_net {
                    rest.y + net_h
                } else {
                    rest.y
                },
                left_w,
                mem_h,
            );
            out.net = if opts.mem_below_net {
                Rect::new(rest.x + right_w, rest.y, left_w, net_h)
            } else {
                Rect::new(rest.x + right_w, rest.y + mem_h, left_w, net_h)
            };
        } else {
            out.mem = Rect::new(
                rest.x,
                if opts.mem_below_net {
                    rest.y + net_h
                } else {
                    rest.y
                },
                left_w,
                mem_h,
            );
            out.net = if opts.mem_below_net {
                Rect::new(rest.x, rest.y, left_w, net_h)
            } else {
                Rect::new(rest.x, rest.y + mem_h, left_w, net_h)
            };
            out.agents = Rect::new(rest.x + left_w, rest.y, right_w, rest.height);
        }
    } else if shown.agents {
        if shown.mem || shown.net {
            let left_w = (u32::from(rest.width) * 45).div_ceil(100) as u16;
            let right_w = rest.width.saturating_sub(left_w);
            let (panel_x, agent_x) = if opts.agents_left {
                (rest.x + right_w, rest.x)
            } else {
                (rest.x, rest.x + left_w)
            };
            out.agents = Rect::new(agent_x, rest.y, right_w, rest.height);
            if shown.mem {
                out.mem = Rect::new(panel_x, rest.y, left_w, rest.height);
            }
            if shown.net {
                out.net = Rect::new(panel_x, rest.y, left_w, rest.height);
            }
        } else {
            out.agents = rest;
        }
    } else if shown.mem && shown.net {
        let mh = if shown.cpu {
            u32::from(area.height) * 40 / 100
        } else {
            u32::from(rest.height) * 72 / 100
        } as u16;
        out.mem = if opts.mem_below_net {
            Rect::new(rest.x, rest.y + rest.height - mh, rest.width, mh)
        } else {
            Rect::new(rest.x, rest.y, rest.width, mh)
        };
        out.net = if opts.mem_below_net {
            Rect::new(rest.x, rest.y, rest.width, rest.height - mh)
        } else {
            Rect::new(rest.x, rest.y + mh, rest.width, rest.height - mh)
        };
    } else if shown.mem {
        out.mem = rest;
    } else if shown.net {
        out.net = rest;
    }
    out
}
