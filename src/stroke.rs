//! Stroke style configuration and outline stroking.

use crate::font::Glyph;
use crate::math::{Line, Point};
use crate::OutlineBounds;
use alloc::vec::Vec;
use core::f32::consts::PI;
#[cfg(not(test))]
#[allow(unused_imports)]
use core_maths::CoreFloat;

const POINT_EPSILON_SQUARED: f32 = 1.0e-10;
const TURN_EPSILON: f32 = 1.0e-5;
const ROUND_TOLERANCE: f32 = 0.2;
const MAX_ARC_SEGMENTS: usize = 64;

/// The shape used where two segments of a stroked outline meet.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LineJoin {
    /// Connect offset edges with a circular arc.
    Round,
    /// Connect offset edges with a straight edge.
    Bevel,
    /// Extend offset edges to their intersection, falling back to a bevel when the configured
    /// miter limit would be exceeded.
    Miter,
}

/// Settings that control glyph outline stroking.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StrokeSettings {
    /// The full width of the centered stroke, in output pixels.
    pub width: f32,
    /// The shape used at outline corners.
    pub line_join: LineJoin,
    /// The maximum miter length as a multiple of half the stroke width.
    ///
    /// Values below `1.0` are treated as `1.0`. This value is ignored for round and bevel joins.
    pub miter_limit: f32,
}

impl StrokeSettings {
    /// Creates round-join stroke settings with the supplied full width.
    #[inline]
    #[must_use]
    pub const fn new(width: f32) -> Self {
        Self {
            width,
            line_join: LineJoin::Round,
            miter_limit: 4.0,
        }
    }

    /// Sets the corner join style.
    #[inline]
    #[must_use]
    pub const fn with_line_join(mut self, line_join: LineJoin) -> Self {
        self.line_join = line_join;
        self
    }

    /// Sets the miter limit.
    #[inline]
    #[must_use]
    pub const fn with_miter_limit(mut self, miter_limit: f32) -> Self {
        self.miter_limit = miter_limit;
        self
    }

    pub(crate) fn radius(self) -> Option<f32> {
        if self.width.is_finite() && self.width > 0.0 {
            Some(self.width * 0.5)
        } else {
            None
        }
    }

    fn effective_miter_limit(self) -> f32 {
        if self.miter_limit.is_finite() {
            self.miter_limit.max(1.0)
        } else {
            4.0
        }
    }
}

impl Default for StrokeSettings {
    fn default() -> Self {
        Self::new(1.0)
    }
}

pub(crate) struct StrokeGeometry {
    pub v_lines: Vec<Line>,
    pub m_lines: Vec<Line>,
    pub bounds: OutlineBounds,
}

impl StrokeGeometry {
    pub fn new(glyph: &Glyph, scale: f32, settings: StrokeSettings) -> Self {
        let Some(radius) = settings.radius() else {
            return Self::empty();
        };
        if glyph.contour_points.is_empty() || !scale.is_finite() || scale <= 0.0 {
            return Self::empty();
        }

        let mut builder = StrokePathBuilder::with_capacity(glyph.contour_points.len() * 3);
        let mut start = 0;
        for &end in &glyph.contour_ends {
            if end <= start || end > glyph.contour_points.len() {
                break;
            }
            builder.add_contour(&glyph.contour_points[start..end], scale, radius, settings);
            start = end;
        }
        builder.compile()
    }

    fn empty() -> Self {
        Self {
            v_lines: Vec::new(),
            m_lines: Vec::new(),
            bounds: OutlineBounds::default(),
        }
    }
}

struct StrokePathBuilder {
    points: Vec<Point>,
    contour_ends: Vec<usize>,
    clean: Vec<Point>,
    left: Vec<Point>,
    right: Vec<Point>,
}

impl StrokePathBuilder {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            points: Vec::with_capacity(capacity),
            contour_ends: Vec::new(),
            clean: Vec::new(),
            left: Vec::new(),
            right: Vec::new(),
        }
    }

    fn add_contour(&mut self, contour: &[Point], scale: f32, radius: f32, settings: StrokeSettings) {
        self.clean.clear();
        self.clean.reserve(contour.len());
        for point in contour {
            let scaled = Point::new(point.x * scale, point.y * scale);
            if self.clean.last().is_none_or(|last| distance_squared(*last, scaled) > POINT_EPSILON_SQUARED) {
                self.clean.push(scaled);
            }
        }
        if self.clean.len() > 1
            && distance_squared(self.clean[0], *self.clean.last().unwrap()) <= POINT_EPSILON_SQUARED
        {
            self.clean.pop();
        }
        if self.clean.len() < 2 {
            return;
        }

        self.left.clear();
        self.right.clear();
        self.left.reserve(self.clean.len());
        self.right.reserve(self.clean.len());
        build_border(&self.clean, 1.0, radius, settings, &mut self.left);
        build_border(&self.clean, -1.0, radius, settings, &mut self.right);

        let source_area = signed_area(&self.clean);
        let left_is_inner = source_area > 0.0;
        let right_is_inner = source_area < 0.0;
        // Once an inward offset collapses through itself its signed area reverses. Omitting that
        // invalid border keeps thick strokes filled instead of allowing the inverted loop to
        // recreate a counter that has already closed.
        let left_valid = !left_is_inner || signed_area(&self.left) * source_area > 0.0;
        let right_valid = !right_is_inner || signed_area(&self.right) * source_area > 0.0;
        let inward_must_collapse = radius * 2.0 >= minimum_extent(&self.clean);
        if !left_valid || !right_valid || inward_must_collapse {
            // Offset borders are the fast path, but an inward border can invert after a thick
            // stroke consumes a narrow contour. A primitive union has bounded local geometry and
            // remains correct after that topology change.
            add_primitive_stroke(&self.clean, radius, settings, &mut self.points, &mut self.contour_ends);
            return;
        }
        if self.left.len() >= 2 {
            self.points.extend_from_slice(&self.left);
            self.contour_ends.push(self.points.len());
        }
        if right_valid && self.right.len() >= 2 {
            self.points.extend(self.right.iter().rev().copied());
            self.contour_ends.push(self.points.len());
        }
    }

    fn compile(self) -> StrokeGeometry {
        if self.points.is_empty() {
            return StrokeGeometry::empty();
        }

        let mut xmin = f32::MAX;
        let mut xmax = f32::MIN;
        let mut ymin = f32::MAX;
        let mut ymax = f32::MIN;
        for point in &self.points {
            if !point.x.is_finite() || !point.y.is_finite() {
                continue;
            }
            xmin = xmin.min(point.x);
            xmax = xmax.max(point.x);
            ymin = ymin.min(point.y);
            ymax = ymax.max(point.y);
        }
        if xmin > xmax || ymin > ymax {
            return StrokeGeometry::empty();
        }

        let mut v_lines = Vec::with_capacity(self.points.len() / 4);
        let mut m_lines = Vec::with_capacity(self.points.len());
        let mut start = 0;
        for end in self.contour_ends {
            if end <= start + 1 {
                start = end;
                continue;
            }
            let contour = &self.points[start..end];
            let mut previous = contour[contour.len() - 1];
            for &point in contour {
                push_line(previous, point, xmin, ymax, &mut v_lines, &mut m_lines);
                previous = point;
            }
            start = end;
        }
        StrokeGeometry {
            v_lines,
            m_lines,
            bounds: OutlineBounds {
                xmin,
                ymin,
                width: xmax - xmin,
                height: ymax - ymin,
            },
        }
    }
}

fn build_border(
    contour: &[Point],
    side: f32,
    radius: f32,
    settings: StrokeSettings,
    output: &mut Vec<Point>,
) {
    let len = contour.len();
    for index in 0..len {
        let previous = contour[(index + len - 1) % len];
        let current = contour[index];
        let next = contour[(index + 1) % len];
        let Some(incoming) = normalize(sub(current, previous)) else {
            continue;
        };
        let Some(outgoing) = normalize(sub(next, current)) else {
            continue;
        };
        let incoming_normal = mul(Point::new(-incoming.y, incoming.x), radius * side);
        let outgoing_normal = mul(Point::new(-outgoing.y, outgoing.x), radius * side);
        let first = add(current, incoming_normal);
        let second = add(current, outgoing_normal);
        let turn = cross(incoming, outgoing);

        if turn.abs() <= TURN_EPSILON {
            if dot(incoming, outgoing) > 0.0 {
                push_unique(output, second);
            } else {
                // A reversal has no stable line intersection. Connect the two offset ends;
                // non-zero winding resolves the resulting local overlap.
                push_unique(output, first);
                push_unique(output, second);
            }
            continue;
        }

        let outer = side * turn < 0.0;
        if outer {
            match settings.line_join {
                LineJoin::Bevel => {
                    push_unique(output, first);
                    push_unique(output, second);
                }
                LineJoin::Miter => {
                    if let Some(intersection) = line_intersection(first, incoming, second, outgoing) {
                        if distance_squared(intersection, current)
                            <= square(radius * settings.effective_miter_limit())
                        {
                            push_unique(output, intersection);
                        } else {
                            push_unique(output, first);
                            push_unique(output, second);
                        }
                    } else {
                        push_unique(output, first);
                        push_unique(output, second);
                    }
                }
                LineJoin::Round => {
                    push_round_join(output, current, incoming_normal, outgoing_normal, turn, radius)
                }
            }
        } else if let Some(intersection) = line_intersection(first, incoming, second, outgoing) {
            // Extremely sharp inner corners can place the mathematical intersection arbitrarily
            // far away. A bevel is both bounded and equivalent after the offset path overlaps.
            if distance_squared(intersection, current) <= square(radius * 16.0) {
                push_unique(output, intersection);
            } else {
                push_unique(output, first);
                push_unique(output, second);
            }
        } else {
            push_unique(output, first);
            push_unique(output, second);
        }
    }
}

fn push_round_join(
    output: &mut Vec<Point>,
    center: Point,
    first_normal: Point,
    second_normal: Point,
    turn: f32,
    radius: f32,
) {
    let start_angle = first_normal.y.atan2(first_normal.x);
    let end_angle = second_normal.y.atan2(second_normal.x);
    let mut delta = end_angle - start_angle;
    if turn > 0.0 {
        while delta < 0.0 {
            delta += 2.0 * PI;
        }
    } else {
        while delta > 0.0 {
            delta -= 2.0 * PI;
        }
    }
    // Outer joins always use the turn's minor arc. Guard malformed reversals from producing an
    // almost-complete circle due to angle wrapping.
    if delta.abs() > PI {
        delta -= (2.0 * PI).copysign(delta);
    }

    let max_step = if radius <= ROUND_TOLERANCE {
        PI * 0.5
    } else {
        2.0 * (1.0 - ROUND_TOLERANCE / radius).clamp(-1.0, 1.0).acos()
    };
    let segments = (delta.abs() / max_step).ceil().max(1.0) as usize;
    let segments = segments.min(MAX_ARC_SEGMENTS);
    for segment in 0..=segments {
        let angle = start_angle + delta * (segment as f32 / segments as f32);
        push_unique(output, add(center, Point::new(angle.cos() * radius, angle.sin() * radius)));
    }
}

fn minimum_extent(contour: &[Point]) -> f32 {
    let mut xmin = f32::MAX;
    let mut xmax = f32::MIN;
    let mut ymin = f32::MAX;
    let mut ymax = f32::MIN;
    for point in contour {
        xmin = xmin.min(point.x);
        xmax = xmax.max(point.x);
        ymin = ymin.min(point.y);
        ymax = ymax.max(point.y);
    }
    (xmax - xmin).min(ymax - ymin)
}

fn add_primitive_stroke(
    contour: &[Point],
    radius: f32,
    settings: StrokeSettings,
    points: &mut Vec<Point>,
    contour_ends: &mut Vec<usize>,
) {
    let len = contour.len();
    let mut directions = Vec::with_capacity(len);
    for index in 0..len {
        let start = contour[index];
        let end = contour[(index + 1) % len];
        let Some(direction) = normalize(sub(end, start)) else {
            directions.push(Point::new(0.0, 0.0));
            continue;
        };
        directions.push(direction);
        let normal = mul(Point::new(-direction.y, direction.x), radius);
        let rectangle = [add(start, normal), add(end, normal), sub(end, normal), sub(start, normal)];
        append_clockwise(points, contour_ends, &rectangle);
    }

    let mut join = Vec::new();
    for index in 0..len {
        let incoming = directions[(index + len - 1) % len];
        let outgoing = directions[index];
        if dot(incoming, incoming) <= POINT_EPSILON_SQUARED
            || dot(outgoing, outgoing) <= POINT_EPSILON_SQUARED
        {
            continue;
        }
        let current = contour[index];
        let turn = cross(incoming, outgoing);
        if turn.abs() <= TURN_EPSILON {
            continue;
        }
        let side = if turn > 0.0 {
            -1.0
        } else {
            1.0
        };
        let first_normal = mul(Point::new(-incoming.y, incoming.x), radius * side);
        let second_normal = mul(Point::new(-outgoing.y, outgoing.x), radius * side);
        let first = add(current, first_normal);
        let second = add(current, second_normal);
        join.clear();
        match settings.line_join {
            LineJoin::Round => {
                let max_step = if radius <= ROUND_TOLERANCE {
                    PI * 0.5
                } else {
                    2.0 * (1.0 - ROUND_TOLERANCE / radius).clamp(-1.0, 1.0).acos()
                };
                let segments = ((2.0 * PI / max_step).ceil() as usize).clamp(4, MAX_ARC_SEGMENTS * 2);
                for segment in 0..segments {
                    let angle = -2.0 * PI * segment as f32 / segments as f32;
                    join.push(add(current, Point::new(angle.cos() * radius, angle.sin() * radius)));
                }
            }
            LineJoin::Bevel => join.extend_from_slice(&[current, first, second]),
            LineJoin::Miter => {
                if let Some(intersection) = line_intersection(first, incoming, second, outgoing) {
                    if distance_squared(intersection, current)
                        <= square(radius * settings.effective_miter_limit())
                    {
                        join.extend_from_slice(&[current, first, intersection, second]);
                    } else {
                        join.extend_from_slice(&[current, first, second]);
                    }
                } else {
                    join.extend_from_slice(&[current, first, second]);
                }
            }
        }
        append_clockwise(points, contour_ends, &join);
    }
}

fn append_clockwise(points: &mut Vec<Point>, contour_ends: &mut Vec<usize>, contour: &[Point]) {
    if contour.len() < 3 {
        return;
    }
    if signed_area(contour) > 0.0 {
        points.extend(contour.iter().rev().copied());
    } else {
        points.extend_from_slice(contour);
    }
    contour_ends.push(points.len());
}

fn signed_area(contour: &[Point]) -> f32 {
    if contour.len() < 3 {
        return 0.0;
    }
    let mut area = 0.0;
    let mut previous = contour[contour.len() - 1];
    for &point in contour {
        area += previous.x * point.y - point.x * previous.y;
        previous = point;
    }
    area * 0.5
}

fn push_line(
    start: Point,
    end: Point,
    xmin: f32,
    ymax: f32,
    v_lines: &mut Vec<Line>,
    m_lines: &mut Vec<Line>,
) {
    if distance_squared(start, end) <= POINT_EPSILON_SQUARED {
        return;
    }
    let start = Point::new(start.x - xmin, ymax - start.y);
    let end = Point::new(end.x - xmin, ymax - end.y);
    if start.y.to_bits() == end.y.to_bits() {
        return;
    }
    let line = Line::new(start, end);
    if start.x.to_bits() == end.x.to_bits() {
        v_lines.push(line);
    } else {
        m_lines.push(line);
    }
}

#[inline]
fn line_intersection(
    first: Point,
    first_direction: Point,
    second: Point,
    second_direction: Point,
) -> Option<Point> {
    let denominator = cross(first_direction, second_direction);
    if denominator.abs() <= TURN_EPSILON {
        return None;
    }
    let distance = cross(sub(second, first), second_direction) / denominator;
    let intersection = add(first, mul(first_direction, distance));
    (intersection.x.is_finite() && intersection.y.is_finite()).then_some(intersection)
}

#[inline]
fn push_unique(points: &mut Vec<Point>, point: Point) {
    if point.x.is_finite()
        && point.y.is_finite()
        && points.last().is_none_or(|last| distance_squared(*last, point) > POINT_EPSILON_SQUARED)
    {
        points.push(point);
    }
}

#[inline]
fn normalize(point: Point) -> Option<Point> {
    let length_squared = dot(point, point);
    if !length_squared.is_finite() || length_squared <= POINT_EPSILON_SQUARED {
        None
    } else {
        Some(mul(point, 1.0 / length_squared.sqrt()))
    }
}

#[inline]
fn add(a: Point, b: Point) -> Point {
    Point::new(a.x + b.x, a.y + b.y)
}

#[inline]
fn sub(a: Point, b: Point) -> Point {
    Point::new(a.x - b.x, a.y - b.y)
}

#[inline]
fn mul(point: Point, scalar: f32) -> Point {
    Point::new(point.x * scalar, point.y * scalar)
}

#[inline]
fn dot(a: Point, b: Point) -> f32 {
    a.x * b.x + a.y * b.y
}

#[inline]
fn cross(a: Point, b: Point) -> f32 {
    a.x * b.y - a.y * b.x
}

#[inline]
fn distance_squared(a: Point, b: Point) -> f32 {
    square(a.x - b.x) + square(a.y - b.y)
}

#[inline]
fn square(value: f32) -> f32 {
    value * value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn miter_limit_falls_back_to_bevel() {
        let contour = [Point::new(-10.0, 0.0), Point::new(0.0, 0.0), Point::new(-10.0, 1.0)];
        let mut border = Vec::new();
        build_border(
            &contour,
            -1.0,
            2.0,
            StrokeSettings::new(4.0).with_line_join(LineJoin::Miter).with_miter_limit(2.0),
            &mut border,
        );
        assert!(border.iter().all(|point| distance_squared(*point, Point::new(0.0, 0.0)) <= 400.0));
    }

    #[test]
    fn round_join_is_adaptively_subdivided() {
        let contour = [Point::new(0.0, 0.0), Point::new(10.0, 0.0), Point::new(10.0, 10.0)];
        let mut small = Vec::new();
        let mut large = Vec::new();
        build_border(&contour, -1.0, 1.0, StrokeSettings::new(2.0), &mut small);
        build_border(&contour, -1.0, 20.0, StrokeSettings::new(40.0), &mut large);
        assert!(large.len() > small.len());
    }
}
