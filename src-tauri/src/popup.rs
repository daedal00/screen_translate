use crate::capture::LogicalRect;

/// Top-left of a w x h popup next to selection `sel`, in the same logical coords as `screen`
/// (monitor bounds). Prefers below the selection, then above, then pinned to the screen bottom;
/// x is clamped inside the screen. A popup bigger than the screen aligns to the top-left margin.
pub fn place(sel: LogicalRect, w: f64, h: f64, screen: LogicalRect, margin: f64) -> (f64, f64) {
    let (left, top) = (screen.x + margin, screen.y + margin);
    let (right, bottom) = (screen.x + screen.width - margin, screen.y + screen.height - margin);

    let x = if w > right - left { left } else { sel.x.clamp(left, right - w) };

    let below = sel.y + sel.height + margin;
    let above = sel.y - margin - h;
    let y = if h > bottom - top {
        top
    } else if below + h <= bottom {
        below
    } else if above >= top {
        above
    } else {
        bottom - h
    };
    (x, y)
}
