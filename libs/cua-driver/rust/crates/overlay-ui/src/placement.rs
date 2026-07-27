#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn contains(self, point: Point) -> bool {
        point.x >= self.x
            && point.x <= self.x + self.width
            && point.y >= self.y
            && point.y <= self.y + self.height
    }
}

/// Place a surface close to the physical pointer while keeping both the
/// pointer and the whole card visible. Coordinates use a top-left origin.
pub fn place_near_pointer(pointer: Point, work_area: Rect, surface: Size) -> Point {
    const GAP: f64 = 18.0;
    const EDGE: f64 = 12.0;

    let right = pointer.x + GAP;
    let left = pointer.x - GAP - surface.width;
    let below = pointer.y + GAP;
    let above = pointer.y - GAP - surface.height;

    let x = if right + surface.width <= work_area.x + work_area.width - EDGE {
        right
    } else if left >= work_area.x + EDGE {
        left
    } else {
        (pointer.x - surface.width / 2.0).clamp(
            work_area.x + EDGE,
            work_area.x + work_area.width - surface.width - EDGE,
        )
    };
    let y = if below + surface.height <= work_area.y + work_area.height - EDGE {
        below
    } else if above >= work_area.y + EDGE {
        above
    } else {
        (pointer.y - surface.height / 2.0).clamp(
            work_area.y + EDGE,
            work_area.y + work_area.height - surface.height - EDGE,
        )
    };
    Point { x, y }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1440.0,
        height: 900.0,
    };
    const CARD: Size = Size {
        width: 420.0,
        height: 250.0,
    };

    #[test]
    fn prefers_below_and_right() {
        assert_eq!(
            place_near_pointer(Point { x: 200.0, y: 200.0 }, WORK, CARD),
            Point { x: 218.0, y: 218.0 }
        );
    }

    #[test]
    fn flips_at_bottom_right_edge() {
        assert_eq!(
            place_near_pointer(
                Point {
                    x: 1400.0,
                    y: 860.0
                },
                WORK,
                CARD
            ),
            Point { x: 962.0, y: 592.0 }
        );
    }

    #[test]
    fn respects_offset_work_areas() {
        let work = Rect {
            x: -1920.0,
            y: -200.0,
            width: 1920.0,
            height: 1080.0,
        };
        let point = place_near_pointer(
            Point {
                x: -1910.0,
                y: -190.0,
            },
            work,
            CARD,
        );
        assert!(point.x >= work.x + 12.0);
        assert!(point.y >= work.y + 12.0);
    }
}
