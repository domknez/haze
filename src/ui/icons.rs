//! Small vector icons painted with egui shapes. Each takes the icon's square rect.

use eframe::egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};
use std::f32::consts::{FRAC_PI_2, PI};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Play,
    Pause,
    Prev,
    Next,
    Volume,
    Mute,
    AirPlay,
    Search,
    Sun,
    Moon,
    Check,
    /// Gear: settings.
    Settings,
    /// A speaker cabinet: outputs in "Play on".
    Speaker,
    Close,
    /// Small up/down chevrons: "opens a menu".
    Chevrons,
    /// Add (playlists).
    Plus,
    Minus,
    Star,
    /// Star outline: "not a favourite yet".
    StarOutline,
    /// Corner brackets pointing out: full screen.
    Fullscreen,
    /// Corner brackets pointing in: leave full screen.
    Windowed,
    /// An arrow curling back to a bar: watch from the start.
    FromStart,
    /// Lines with a dot: the channel list.
    List,
    /// A bin: remove.
    Trash,
    /// A pencil: edit.
    Edit,
    /// Speech bubble lines: audio track.
    Audio,
    /// A link chain: a playlist link.
    Link,
    /// A key: a provider login.
    Key,
    /// A document: a playlist file.
    File,
}

fn p(r: Rect, x: f32, y: f32) -> Pos2 {
    pos2(r.left() + x * r.width(), r.top() + y * r.height())
}

fn arc(center: Pos2, radius: f32, from: f32, to: f32) -> Vec<Pos2> {
    let n = 16;
    (0..=n).map(|i| from + (to - from) * i as f32 / n as f32).map(|a| center + vec2(a.cos(), a.sin()) * radius).collect()
}

/// Crescent: a disc of radius `big` minus a disc offset towards the upper right.
/// Returns its two edges, both running from one horn to the other: the outer arc
/// of the big disc and the inner arc of the bite.
fn crescent(center: Pos2, big: f32) -> (Vec<Pos2>, Vec<Pos2>) {
    let small = big * 0.85;
    let theta = -PI / 4.0; // direction of the bite
    let d = big * 0.55; // distance between the two centres
    let dir = vec2(theta.cos(), theta.sin());
    let bite = center + dir * d;
    // Angles of the horns (where the circles cross), measured from each centre.
    let x = (d * d + big * big - small * small) / (2.0 * d);
    let alpha = (x / big).clamp(-1.0, 1.0).acos();
    let beta = ((x - d) / small).clamp(-1.0, 1.0).acos();
    let outer = arc(center, big, theta + alpha, theta + 2.0 * PI - alpha);
    // The bite's arc on the side facing the big disc's centre (through theta + PI).
    let inner = arc(bite, small, theta + beta, theta + 2.0 * PI - beta);
    (outer, inner)
}

/// Fill the area between two edges with matching point counts (concave shapes
/// egui can't fill as a polygon).
fn fill_between(painter: &Painter, a: &[Pos2], b: &[Pos2], color: Color32) {
    let mut mesh = Mesh::default();
    for (p, q) in a.iter().zip(b) {
        mesh.colored_vertex(*p, color);
        mesh.colored_vertex(*q, color);
    }
    for i in 0..a.len().saturating_sub(1) as u32 {
        let k = i * 2;
        mesh.add_triangle(k, k + 1, k + 2);
        mesh.add_triangle(k + 1, k + 3, k + 2);
    }
    painter.add(Shape::mesh(mesh));
}

pub fn paint(painter: &Painter, r: Rect, icon: Icon, color: Color32) {
    let w = r.width();
    let stroke = Stroke::new((w * 0.09).max(1.4), color);
    let line = |a: Pos2, b: Pos2| painter.line_segment([a, b], stroke);
    let poly = |pts: Vec<Pos2>| painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
    match icon {
        Icon::Play => {
            poly(vec![p(r, 0.26, 0.14), p(r, 0.86, 0.5), p(r, 0.26, 0.86)]);
        }
        Icon::Pause => {
            painter.rect_filled(Rect::from_min_max(p(r, 0.22, 0.16), p(r, 0.42, 0.84)), 1.5, color);
            painter.rect_filled(Rect::from_min_max(p(r, 0.58, 0.16), p(r, 0.78, 0.84)), 1.5, color);
        }
        Icon::Prev | Icon::Next => {
            let flip = |x: f32| if icon == Icon::Next { 1.0 - x } else { x };
            painter.rect_filled(Rect::from_two_pos(p(r, flip(0.18), 0.2), p(r, flip(0.28), 0.8)), 1.0, color);
            poly(if icon == Icon::Next {
                vec![p(r, flip(0.86), 0.2), p(r, flip(0.3), 0.5), p(r, flip(0.86), 0.8)]
            } else {
                vec![p(r, 0.3, 0.5), p(r, 0.86, 0.2), p(r, 0.86, 0.8)]
            });
        }
        Icon::Volume | Icon::Mute => {
            poly(vec![p(r, 0.12, 0.38), p(r, 0.28, 0.38), p(r, 0.5, 0.18), p(r, 0.5, 0.82), p(r, 0.28, 0.62), p(r, 0.12, 0.62)]);
            if icon == Icon::Mute {
                line(p(r, 0.62, 0.36), p(r, 0.88, 0.64));
                line(p(r, 0.88, 0.36), p(r, 0.62, 0.64));
            } else {
                let c = p(r, 0.5, 0.5);
                painter.add(Shape::line(arc(c, w * 0.18, -0.9, 0.9), stroke));
                painter.add(Shape::line(arc(c, w * 0.34, -0.9, 0.9), stroke));
            }
        }
        Icon::AirPlay => {
            let s = Stroke::new(stroke.width, color);
            painter.add(Shape::line(
                vec![p(r, 0.3, 0.68), p(r, 0.12, 0.68), p(r, 0.12, 0.16), p(r, 0.88, 0.16), p(r, 0.88, 0.68), p(r, 0.7, 0.68)],
                s,
            ));
            poly(vec![p(r, 0.5, 0.56), p(r, 0.76, 0.88), p(r, 0.24, 0.88)]);
        }
        Icon::Search => {
            painter.circle_stroke(p(r, 0.43, 0.43), w * 0.26, stroke);
            line(p(r, 0.62, 0.62), p(r, 0.86, 0.86));
        }
        Icon::Sun => {
            let c = p(r, 0.5, 0.5);
            painter.circle_filled(c, w * 0.17, color);
            for k in 0..8 {
                let a = k as f32 * PI / 4.0;
                let d = vec2(a.cos(), a.sin());
                line(c + d * w * 0.3, c + d * w * 0.42);
            }
        }
        Icon::Moon => {
            let (outer, inner) = crescent(p(r, 0.5, 0.5), w * 0.38);
            fill_between(painter, &outer, &inner, color);
        }
        Icon::Check => {
            painter.add(Shape::line(vec![p(r, 0.18, 0.52), p(r, 0.4, 0.74), p(r, 0.84, 0.28)], stroke));
        }
        Icon::Settings => {
            let c = r.center();
            let s = Stroke::new(stroke.width, color);
            painter.circle_stroke(c, w * 0.26, s);
            painter.circle_stroke(c, w * 0.09, s);
            for i in 0..8 {
                let a = i as f32 * PI / 4.0;
                let d = vec2(a.cos(), a.sin());
                line(c + d * w * 0.3, c + d * w * 0.42);
            }
        }
        Icon::Speaker => {
            let s = Stroke::new(stroke.width * 0.85, color);
            painter.rect_stroke(Rect::from_min_max(p(r, 0.24, 0.1), p(r, 0.76, 0.9)), w * 0.1, s, eframe::egui::StrokeKind::Middle);
            painter.circle_stroke(p(r, 0.5, 0.6), w * 0.14, s);
            painter.circle_filled(p(r, 0.5, 0.28), w * 0.05, color);
        }
        Icon::Close => {
            line(p(r, 0.22, 0.22), p(r, 0.78, 0.78));
            line(p(r, 0.78, 0.22), p(r, 0.22, 0.78));
        }
        Icon::Minus => {
            line(p(r, 0.2, 0.5), p(r, 0.8, 0.5));
        }
        Icon::Plus => {
            line(p(r, 0.5, 0.2), p(r, 0.5, 0.8));
            line(p(r, 0.2, 0.5), p(r, 0.8, 0.5));
        }
        Icon::Star | Icon::StarOutline => {
            let c = p(r, 0.5, 0.54);
            let pts: Vec<Pos2> = (0..10)
                .map(|i| {
                    let a = -FRAC_PI_2 + i as f32 * PI / 5.0;
                    let rad = if i % 2 == 0 { w * 0.42 } else { w * 0.18 };
                    c + vec2(a.cos(), a.sin()) * rad
                })
                .collect();
            if icon == Icon::Star {
                // Concave: a fan of triangles from the centre.
                let mut mesh = Mesh::default();
                mesh.colored_vertex(c, color);
                for q in &pts {
                    mesh.colored_vertex(*q, color);
                }
                for i in 0..10u32 {
                    mesh.add_triangle(0, 1 + i, 1 + (i + 1) % 10);
                }
                painter.add(Shape::mesh(mesh));
            } else {
                painter.add(Shape::closed_line(pts, Stroke::new((w * 0.075).max(1.2), color)));
            }
        }
        Icon::Fullscreen | Icon::Windowed => {
            let s = Stroke::new(stroke.width, color);
            let out = icon == Icon::Fullscreen;
            for (x, y, dx, dy) in [(0.16, 0.16, 1.0, 1.0), (0.84, 0.16, -1.0, 1.0), (0.16, 0.84, 1.0, -1.0), (0.84, 0.84, -1.0, -1.0)] {
                let corner = if out { p(r, x, y) } else { p(r, x, y) + vec2(dx, dy) * w * 0.22 };
                let (ex, ey) = if out { (dx, dy) } else { (-dx, -dy) };
                painter.add(Shape::line(vec![corner + vec2(0.0, ey * w * 0.22), corner, corner + vec2(ex * w * 0.22, 0.0)], s));
            }
        }
        Icon::FromStart => {
            let c = p(r, 0.54, 0.54);
            painter.add(Shape::line(arc(c, w * 0.3, PI * 0.95, PI * 2.6), stroke));
            let tip = c + vec2((PI * 0.95).cos(), (PI * 0.95).sin()) * w * 0.3;
            poly(vec![tip + vec2(-w * 0.13, -w * 0.02), tip + vec2(w * 0.11, -w * 0.02), tip + vec2(-w * 0.01, -w * 0.18)]);
            line(p(r, 0.54, 0.4), p(r, 0.54, 0.56));
            line(p(r, 0.54, 0.56), p(r, 0.66, 0.62));
        }
        Icon::List => {
            for y in [0.26, 0.5, 0.74] {
                painter.circle_filled(p(r, 0.16, y), w * 0.06, color);
                line(p(r, 0.32, y), p(r, 0.88, y));
            }
        }
        Icon::Trash => {
            let s = Stroke::new(stroke.width * 0.9, color);
            line(p(r, 0.14, 0.24), p(r, 0.86, 0.24));
            line(p(r, 0.4, 0.12), p(r, 0.6, 0.12));
            painter.add(Shape::line(vec![p(r, 0.24, 0.24), p(r, 0.3, 0.88), p(r, 0.7, 0.88), p(r, 0.76, 0.24)], s));
            line(p(r, 0.44, 0.4), p(r, 0.45, 0.72));
            line(p(r, 0.56, 0.4), p(r, 0.55, 0.72));
        }
        Icon::Edit => {
            line(p(r, 0.2, 0.8), p(r, 0.74, 0.26));
            line(p(r, 0.66, 0.18), p(r, 0.82, 0.34));
            line(p(r, 0.14, 0.86), p(r, 0.2, 0.68));
            line(p(r, 0.14, 0.86), p(r, 0.32, 0.8));
        }
        Icon::Audio => {
            let s = Stroke::new(stroke.width, color);
            painter.add(Shape::line(
                vec![
                    p(r, 0.36, 0.72),
                    p(r, 0.88, 0.72),
                    p(r, 0.88, 0.16),
                    p(r, 0.12, 0.16),
                    p(r, 0.12, 0.72),
                    p(r, 0.2, 0.72),
                    p(r, 0.2, 0.9),
                    p(r, 0.36, 0.72),
                ],
                s,
            ));
            for (x, h) in [(0.32, 0.12), (0.44, 0.2), (0.56, 0.08), (0.68, 0.16)] {
                line(p(r, x, 0.44 - h), p(r, x, 0.44 + h));
            }
        }
        Icon::Link => {
            let s = Stroke::new(stroke.width, color);
            for (a, b) in [((0.14, 0.58), (0.5, 0.22)), ((0.5, 0.78), (0.86, 0.42))] {
                let (pa, pb) = (p(r, a.0, a.1), p(r, b.0, b.1));
                let d = (pb - pa).normalized();
                let n = vec2(-d.y, d.x) * w * 0.1;
                painter.add(Shape::closed_line(vec![pa + n, pb + n, pb - n, pa - n], s));
            }
            line(p(r, 0.38, 0.62), p(r, 0.62, 0.38));
        }
        Icon::Key => {
            painter.circle_stroke(p(r, 0.32, 0.5), w * 0.17, stroke);
            line(p(r, 0.49, 0.5), p(r, 0.9, 0.5));
            line(p(r, 0.78, 0.5), p(r, 0.78, 0.68));
            line(p(r, 0.66, 0.5), p(r, 0.66, 0.64));
        }
        Icon::File => {
            let s = Stroke::new(stroke.width * 0.9, color);
            painter.add(Shape::closed_line(vec![p(r, 0.2, 0.1), p(r, 0.6, 0.1), p(r, 0.8, 0.3), p(r, 0.8, 0.9), p(r, 0.2, 0.9)], s));
            line(p(r, 0.34, 0.5), p(r, 0.66, 0.5));
            line(p(r, 0.34, 0.68), p(r, 0.66, 0.68));
        }
        Icon::Chevrons => {
            let s = Stroke::new(stroke.width, color);
            painter.add(Shape::line(vec![p(r, 0.28, 0.4), p(r, 0.5, 0.2), p(r, 0.72, 0.4)], s));
            painter.add(Shape::line(vec![p(r, 0.28, 0.6), p(r, 0.5, 0.8), p(r, 0.72, 0.6)], s));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crescent_edges_meet_at_the_horns() {
        let c = pos2(0.0, 0.0);
        let (outer, inner) = crescent(c, 10.0);
        assert_eq!(outer.len(), inner.len());
        for (a, b) in [(outer[0], inner[0]), (*outer.last().unwrap(), *inner.last().unwrap())] {
            assert!((a - b).length() < 0.01, "horns {a:?} and {b:?} don't meet");
        }
        // The inner edge stays inside the disc: it's the bite, not its far side.
        assert!(inner.iter().all(|p| (*p - c).length() <= 10.01));
        // The lit part faces away from the bite (lower left).
        let mid = outer[outer.len() / 2];
        assert!(mid.x < 0.0 && mid.y > 0.0);
    }
}
