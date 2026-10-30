/// One announced ei_device::Region — a single monitor's pixel rectangle
/// inside the seat's logical coordinate space. Multi-monitor sessions
/// announce one Region per output; we route each absolute (x, y) to the
/// region containing it so the cursor lands on the correct monitor.
#[derive(Clone, Copy, Debug)]
pub(super) struct RegionRect {
    pub(super) offset_x: f32,
    pub(super) offset_y: f32,
    pub(super) width: f32,
    pub(super) height: f32,
}

impl RegionRect {
    pub(super) fn contains(&self, x: f64, y: f64) -> bool {
        let xf = x as f32;
        let yf = y as f32;
        xf >= self.offset_x
            && xf < self.offset_x + self.width
            && yf >= self.offset_y
            && yf < self.offset_y + self.height
    }
}

/// First candidate whose regions contain `(x, y)`. `Ok(None)` means no
/// candidate announced any region (caller falls back). Regions that exist but
/// exclude the point are an error: EIS would silently discard the motion and
/// the button would land at the stale pointer position.
pub(super) fn route_absolute<'a, K>(
    candidates: impl IntoIterator<Item = (K, &'a [RegionRect])>,
    x: f64,
    y: f64,
) -> anyhow::Result<Option<K>> {
    let mut announced = false;
    for (key, regions) in candidates {
        if regions.iter().any(|r| r.contains(x, y)) {
            return Ok(Some(key));
        }
        announced |= !regions.is_empty();
    }
    if announced {
        anyhow::bail!("libei: ({x}, {y}) lies outside every announced pointer region");
    }
    Ok(None)
}
