//! The marking state: what dragging, the handles, the arrows and a typed size
//! do to the rectangle. Pure, in physical pixels local to the monitor.

use std::{error, fmt};

/// The smallest area: `nvh264enc` refuses frames under 160 wide.
pub const MIN_WIDTH: i32 = 160;
pub const MIN_HEIGHT: i32 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// Where a press lands on the rectangle: one of the eight handles, or inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
    Inside,
}

/// The typed size does not fit on the monitor, or is under the minimum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoesNotFit;

impl fmt::Display for DoesNotFit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "The size does not fit on the monitor")
    }
}

impl error::Error for DoesNotFit {}

/// What the current gesture does.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Drag {
    /// A new rectangle from `origin` to the pointer.
    New {
        origin: (i32, i32),
        pointer: (i32, i32),
    },
    /// The rectangle `start` moved by the pointer offset from `press`.
    Move { start: Rect, press: (i32, i32) },
    /// The sides of `handle` follow the pointer offset from `press`; the
    /// others stay.
    Resize {
        start: Rect,
        handle: Handle,
        press: (i32, i32),
    },
}

#[derive(Debug, Clone)]
pub struct MarkState {
    /// Physical size of the monitor.
    bounds: (i32, i32),
    rect: Option<Rect>,
    drag: Option<Drag>,
}

impl MarkState {
    pub fn new(bounds: (i32, i32), initial: Option<Rect>) -> Self {
        let mut state = Self {
            bounds,
            rect: None,
            drag: None,
        };
        state.rect = initial.map(|rect| {
            let (min_width, min_height) = state.min_size();
            state.fit(Rect {
                width: rect.width.max(min_width),
                height: rect.height.max(min_height),
                ..rect
            })
        });
        state
    }

    /// Even-sized and inside the bounds; at least the minimum except while a
    /// new rectangle is being dragged.
    pub fn rect(&self) -> Option<Rect> {
        self.rect
    }

    /// The handle under `(x, y)`, within `slop` pixels; handles first, then
    /// the inside.
    pub fn hit(&self, x: i32, y: i32, slop: i32) -> Option<Handle> {
        let rect = self.rect?;
        let (left, top) = (rect.x, rect.y);
        let (right, bottom) = (rect.x + rect.width, rect.y + rect.height);

        let near = |a: i32, b: i32| (a - b).abs() <= slop;
        let across = |a: i32, low: i32, high: i32| low - slop <= a && a <= high + slop;
        let vertical = if near(y, top) {
            Some(Handle::N)
        } else if near(y, bottom) {
            Some(Handle::S)
        } else {
            None
        };
        let horizontal = if near(x, left) {
            Some(Handle::W)
        } else if near(x, right) {
            Some(Handle::E)
        } else {
            None
        };

        match (vertical, horizontal) {
            (Some(Handle::N), Some(Handle::W)) => Some(Handle::NW),
            (Some(Handle::N), Some(Handle::E)) => Some(Handle::NE),
            (Some(Handle::S), Some(Handle::W)) => Some(Handle::SW),
            (Some(Handle::S), Some(Handle::E)) => Some(Handle::SE),
            (Some(side), None) if across(x, left, right) => Some(side),
            (None, Some(side)) if across(y, top, bottom) => Some(side),
            _ if left < x && x < right && top < y && y < bottom => Some(Handle::Inside),
            _ => None,
        }
    }

    /// On a handle, starts resizing or moving; elsewhere, a new rectangle.
    pub fn press(&mut self, x: i32, y: i32, slop: i32) {
        let point = self.clamp_point(x, y);
        self.drag = match (self.hit(x, y, slop), self.rect) {
            (Some(Handle::Inside), Some(start)) => Some(Drag::Move {
                start,
                press: point,
            }),
            (Some(handle), Some(start)) => Some(Drag::Resize {
                start,
                handle,
                press: point,
            }),
            _ => {
                self.rect = Some(Rect {
                    x: point.0,
                    y: point.1,
                    width: 0,
                    height: 0,
                });
                Some(Drag::New {
                    origin: point,
                    pointer: point,
                })
            }
        };
    }

    pub fn motion(&mut self, x: i32, y: i32) {
        let point = self.clamp_point(x, y);
        match self.drag {
            Some(Drag::New { origin, .. }) => {
                self.drag = Some(Drag::New {
                    origin,
                    pointer: point,
                });
                let (x, width) = span_from(
                    origin.0,
                    point.0 - origin.0,
                    even_down((point.0 - origin.0).abs()),
                );
                let (y, height) = span_from(
                    origin.1,
                    point.1 - origin.1,
                    even_down((point.1 - origin.1).abs()),
                );
                self.rect = Some(Rect {
                    x,
                    y,
                    width,
                    height,
                });
            }
            Some(Drag::Move { start, press }) => {
                self.rect = Some(self.fit(Rect {
                    x: start.x + point.0 - press.0,
                    y: start.y + point.1 - press.1,
                    ..start
                }));
            }
            Some(Drag::Resize {
                start,
                handle,
                press,
            }) => {
                self.rect = Some(self.resize(start, handle, point.0 - press.0, point.1 - press.1));
            }
            None => {}
        }
    }

    pub fn release(&mut self) {
        if let Some(Drag::New { origin, pointer }) = self.drag
            && let Some(rect) = self.rect
        {
            let (min_width, min_height) = self.min_size();
            let (x, width) = span_from(origin.0, pointer.0 - origin.0, rect.width.max(min_width));
            let (y, height) =
                span_from(origin.1, pointer.1 - origin.1, rect.height.max(min_height));
            self.rect = Some(self.fit(Rect {
                x,
                y,
                width,
                height,
            }));
        }
        self.drag = None;
    }

    /// Forgets the rectangle: another monitor got the area.
    pub fn clear(&mut self) {
        self.rect = None;
        self.drag = None;
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Moves the rectangle without resizing it, stopping at the edges.
    pub fn nudge(&mut self, dx: i32, dy: i32) {
        if self.drag.is_none()
            && let Some(rect) = self.rect
        {
            self.rect = Some(self.fit(Rect {
                x: rect.x + dx,
                y: rect.y + dy,
                ..rect
            }));
        }
    }

    /// Resizes keeping the top-left corner, moved inward if needed.
    pub fn set_size(&mut self, width: i32, height: i32) -> Result<(), DoesNotFit> {
        let (width, height) = (even_down(width), even_down(height));
        if width < MIN_WIDTH
            || height < MIN_HEIGHT
            || width > self.bounds.0
            || height > self.bounds.1
        {
            return Err(DoesNotFit);
        }

        let (x, y) = self.rect.map_or((0, 0), |rect| (rect.x, rect.y));
        self.rect = Some(self.fit(Rect {
            x,
            y,
            width,
            height,
        }));
        self.drag = None;
        Ok(())
    }

    /// The minimum, or the whole monitor if it is smaller.
    fn min_size(&self) -> (i32, i32) {
        (
            MIN_WIDTH.min(even_down(self.bounds.0)),
            MIN_HEIGHT.min(even_down(self.bounds.1)),
        )
    }

    fn clamp_point(&self, x: i32, y: i32) -> (i32, i32) {
        (x.clamp(0, self.bounds.0), y.clamp(0, self.bounds.1))
    }

    /// Even-sized, no bigger than the monitor, and moved inward to fit.
    fn fit(&self, rect: Rect) -> Rect {
        let width = even_down(rect.width).min(even_down(self.bounds.0));
        let height = even_down(rect.height).min(even_down(self.bounds.1));
        Rect {
            x: rect.x.clamp(0, self.bounds.0 - width),
            y: rect.y.clamp(0, self.bounds.1 - height),
            width,
            height,
        }
    }

    /// `start` with the sides of `handle` moved by `(dx, dy)`, inside the
    /// monitor and at least the minimum; the opposite sides stay.
    fn resize(&self, start: Rect, handle: Handle, dx: i32, dy: i32) -> Rect {
        let (min_width, min_height) = self.min_size();
        let (west, east, north, south) = match handle {
            Handle::N => (false, false, true, false),
            Handle::S => (false, false, false, true),
            Handle::E => (false, true, false, false),
            Handle::W => (true, false, false, false),
            Handle::NE => (false, true, true, false),
            Handle::NW => (true, false, true, false),
            Handle::SE => (false, true, false, true),
            Handle::SW => (true, false, false, true),
            Handle::Inside => (false, false, false, false),
        };

        let (x, width) = resize_span(
            start.x,
            start.width,
            west,
            east,
            if west || east { dx } else { 0 },
            min_width,
            self.bounds.0,
        );
        let (y, height) = resize_span(
            start.y,
            start.height,
            north,
            south,
            if north || south { dy } else { 0 },
            min_height,
            self.bounds.1,
        );
        Rect {
            x,
            y,
            width,
            height,
        }
    }
}

/// One axis of a resize: the low side (`low`) or the high side (`high`)
/// moves by `delta`, the other one stays. Returns (start, length).
fn resize_span(
    start: i32,
    length: i32,
    low: bool,
    high: bool,
    delta: i32,
    min: i32,
    bound: i32,
) -> (i32, i32) {
    let end = start + length;
    if low {
        let new_start = (start + delta).clamp(0, (end - min).max(0));
        let length = even_down(end - new_start);
        (end - length, length)
    } else if high {
        let new_end = (end + delta).clamp((start + min).min(bound), bound);
        (start, even_down(new_end - start))
    } else {
        (start, length)
    }
}

/// The span of `length` that starts at `origin` and goes the way of
/// `direction`. Returns (start, length).
fn span_from(origin: i32, direction: i32, length: i32) -> (i32, i32) {
    if direction < 0 {
        (origin - length, length)
    } else {
        (origin, length)
    }
}

/// H.264 needs even sizes.
fn even_down(value: i32) -> i32 {
    value.max(0) & !1
}

/// `"1280x720"`, `"1280×720"` or `"1280 x 720"`; both sides positive.
pub fn parse_size(text: &str) -> Option<(i32, i32)> {
    let mut parts = text.split(['x', 'X', '×']);
    let width = parts.next()?.trim().parse::<i32>().ok()?;
    let height = parts.next()?.trim().parse::<i32>().ok()?;
    (parts.next().is_none() && width > 0 && height > 0).then_some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DP-3 in physical pixels.
    const DP3: (i32, i32) = (1680, 1050);
    const SLOP: i32 = 8;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn drag(state: &mut MarkState, from: (i32, i32), to: (i32, i32)) {
        state.press(from.0, from.1, SLOP);
        state.motion(to.0, to.1);
        state.release();
    }

    #[test]
    fn new_drag_up_and_left_is_normalized() {
        let mut state = MarkState::new(DP3, None);
        drag(&mut state, (900, 700), (500, 300));
        assert_eq!(state.rect(), Some(rect(500, 300, 400, 400)));
    }

    #[test]
    fn new_drag_is_clipped_to_the_monitor() {
        let mut state = MarkState::new(DP3, None);
        drag(&mut state, (1400, 800), (2000, 1200));
        assert_eq!(state.rect(), Some(rect(1400, 800, 280, 250)));

        drag(&mut state, (300, 200), (-50, -40));
        assert_eq!(state.rect(), Some(rect(0, 0, 300, 200)));
    }

    #[test]
    fn size_is_rounded_down_to_even_keeping_the_origin() {
        let mut state = MarkState::new(DP3, None);
        drag(&mut state, (100, 100), (401, 277));
        assert_eq!(state.rect(), Some(rect(100, 100, 300, 176)));

        // Dragged up and left, the origin is the bottom-right corner.
        let mut state = MarkState::new(DP3, None);
        drag(&mut state, (401, 277), (100, 100));
        assert_eq!(state.rect(), Some(rect(101, 101, 300, 176)));
    }

    #[test]
    fn minimum_is_applied_on_release() {
        let mut state = MarkState::new(DP3, None);
        state.press(100, 100, SLOP);
        state.motion(150, 130);
        // Drawn as it is while dragging.
        assert_eq!(state.rect(), Some(rect(100, 100, 50, 30)));
        state.release();
        assert_eq!(state.rect(), Some(rect(100, 100, MIN_WIDTH, MIN_HEIGHT)));

        // Up and left from the origin, moved inward when it does not fit.
        drag(&mut state, (100, 1000), (80, 990));
        assert_eq!(state.rect(), Some(rect(0, 880, MIN_WIDTH, MIN_HEIGHT)));
    }

    #[test]
    fn dragging_inside_moves_without_resizing() {
        let mut state = MarkState::new(DP3, Some(rect(100, 100, 400, 300)));
        drag(&mut state, (200, 200), (250, 150));
        assert_eq!(state.rect(), Some(rect(150, 50, 400, 300)));

        // Stops at the edges.
        drag(&mut state, (200, 200), (5000, 5000));
        assert_eq!(state.rect(), Some(rect(1280, 750, 400, 300)));
    }

    #[test]
    fn se_handle_resizes_the_bottom_right_corner() {
        let mut state = MarkState::new(DP3, Some(rect(100, 100, 400, 300)));
        drag(&mut state, (500, 400), (701, 551));
        assert_eq!(state.rect(), Some(rect(100, 100, 600, 450)));
    }

    #[test]
    fn n_handle_resizes_the_top_side_only() {
        let mut state = MarkState::new(DP3, Some(rect(100, 100, 400, 300)));
        drag(&mut state, (300, 100), (900, 40));
        assert_eq!(state.rect(), Some(rect(100, 40, 400, 360)));

        // Never under the minimum.
        drag(&mut state, (300, 40), (300, 1000));
        assert_eq!(state.rect(), Some(rect(100, 280, 400, MIN_HEIGHT)));
    }

    #[test]
    fn nudge_stops_at_the_edge() {
        let mut state = MarkState::new(DP3, Some(rect(5, 1000, 400, 40 + MIN_HEIGHT)));
        state.nudge(-10, 0);
        assert_eq!(state.rect().unwrap().x, 0);
        state.nudge(1, 1);
        assert_eq!(state.rect(), Some(rect(1, 890, 400, 160)));
    }

    #[test]
    fn set_size_keeps_the_corner_and_moves_inward() {
        let mut state = MarkState::new(DP3, Some(rect(1000, 100, 400, 300)));
        assert_eq!(state.set_size(1280, 720), Ok(()));
        assert_eq!(state.rect(), Some(rect(400, 100, 1280, 720)));

        assert_eq!(state.set_size(641, 481), Ok(()));
        assert_eq!(state.rect(), Some(rect(400, 100, 640, 480)));
    }

    #[test]
    fn set_size_that_does_not_fit_changes_nothing() {
        let mut state = MarkState::new(DP3, Some(rect(1000, 100, 400, 300)));
        assert_eq!(state.set_size(4000, 100), Err(DoesNotFit));
        assert_eq!(state.set_size(1280, 2000), Err(DoesNotFit));
        assert_eq!(state.set_size(100, 100), Err(DoesNotFit));
        assert_eq!(state.rect(), Some(rect(1000, 100, 400, 300)));
    }

    #[test]
    fn parse_size_formats() {
        assert_eq!(parse_size("1280x720"), Some((1280, 720)));
        assert_eq!(parse_size("1280×720"), Some((1280, 720)));
        assert_eq!(parse_size(" 1280 x 720 "), Some((1280, 720)));
        assert_eq!(parse_size("1280X720"), Some((1280, 720)));
        assert_eq!(parse_size("12x"), None);
        assert_eq!(parse_size("0x10"), None);
        assert_eq!(parse_size("abc"), None);
        assert_eq!(parse_size("-5x10"), None);
    }

    #[test]
    fn hit_prefers_a_handle_over_the_inside() {
        let state = MarkState::new(DP3, Some(rect(100, 100, 400, 300)));
        assert_eq!(state.hit(104, 104, SLOP), Some(Handle::NW));
        assert_eq!(state.hit(500, 400, SLOP), Some(Handle::SE));
        assert_eq!(state.hit(300, 95, SLOP), Some(Handle::N));
        assert_eq!(state.hit(497, 250, SLOP), Some(Handle::E));
        assert_eq!(state.hit(300, 250, SLOP), Some(Handle::Inside));
        assert_eq!(state.hit(50, 50, SLOP), None);
        assert_eq!(MarkState::new(DP3, None).hit(300, 250, SLOP), None);
    }

    #[test]
    fn initial_rect_is_made_even_and_fitted() {
        let state = MarkState::new(DP3, Some(rect(1600, 1000, 301, 201)));
        assert_eq!(state.rect(), Some(rect(1380, 850, 300, 200)));
    }

    #[test]
    fn clear_removes_the_rect() {
        let mut state = MarkState::new(DP3, Some(rect(100, 100, 400, 300)));
        state.clear();
        assert_eq!(state.rect(), None);
        assert!(!state.is_dragging());
    }
}
