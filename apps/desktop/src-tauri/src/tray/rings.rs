//! Menu-bar quota rings, drawn as a straight-alpha RGBA image at @2x.
//!
//! The picture has no text: one ring per window, the rings of one profile side by side, a wider
//! gap between profiles. Colours follow the quota meters (normal, amber from 70 %, red from
//! 90 %); the track is a translucent neutral grey, legible on light and dark menu bars alike.

use std::f64::consts::TAU;

/// Image height in pixels: macOS draws tray images 18 pt tall, so this is @2x.
pub const HEIGHT: u32 = 36;
/// Outer ring diameter in pixels.
const DIAMETER: u32 = 30;
/// Ring stroke width in pixels.
const STROKE: f64 = 4.0;
/// Space between the rings of one profile.
const RING_GAP: u32 = 4;
/// Space between profiles.
const GROUP_GAP: u32 = 12;
/// Space left and right of the picture.
const PADDING: u32 = 2;
/// Samples per pixel side, for antialiasing.
const SAMPLES: u32 = 4;
/// Dashes around an unreadable ring (as many gaps).
const DASHES: f64 = 12.0;

/// How close a window is to its limit, as the quota meters grade it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// Under 70 %.
    Normal,
    /// From 70 %.
    Warning,
    /// From 90 %.
    Danger,
}

/// One ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ring {
    /// A window that was read.
    Used {
        /// Arc drawn, percent of the circle (0 to 100).
        filled: u8,
        /// Arc colour.
        level: Level,
    },
    /// The profile's quota cannot be read: a dashed track.
    Unknown,
}

impl Ring {
    /// The ring of a window `percent` used; over 100 draws a full circle.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped to 0..=100 first"
    )]
    pub fn used(percent: f64) -> Self {
        let level = if percent >= 90.0 {
            Level::Danger
        } else if percent >= 70.0 {
            Level::Warning
        } else {
            Level::Normal
        };
        let filled = if percent.is_nan() {
            0
        } else {
            percent.clamp(0.0, 100.0).round() as u8
        };
        Self::Used { filled, level }
    }
}

/// A rendered picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels ([`HEIGHT`]).
    pub height: u32,
    /// Row-major straight-alpha RGBA, `width * height * 4` bytes.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
struct Color([f64; 4]);

const TRACK: Color = Color([142.0, 142.0, 147.0, 0.5]);
const UNKNOWN: Color = Color([142.0, 142.0, 147.0, 0.9]);
const NORMAL: Color = Color([64.0, 120.0, 230.0, 1.0]);
const WARNING: Color = Color([230.0, 160.0, 0.0, 1.0]);
const DANGER: Color = Color([230.0, 70.0, 70.0, 1.0]);

impl Level {
    const fn color(self) -> Color {
        match self {
            Self::Normal => NORMAL,
            Self::Warning => WARNING,
            Self::Danger => DANGER,
        }
    }
}

/// Draws `groups` (the rings of each profile, in order); `None` when there is no ring at all.
#[must_use]
pub fn render(groups: &[Vec<Ring>]) -> Option<Rgba> {
    let groups: Vec<&Vec<Ring>> = groups.iter().filter(|g| !g.is_empty()).collect();
    let mut lefts = Vec::new();
    let mut x = PADDING;
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            x += GROUP_GAP - RING_GAP;
        }
        for ring in *group {
            lefts.push((x, *ring));
            x += DIAMETER + RING_GAP;
        }
    }
    if lefts.is_empty() {
        return None;
    }
    let width = x - RING_GAP + PADDING;
    let mut pixels = vec![0; usize_of(width) * usize_of(HEIGHT) * 4];
    let top = (HEIGHT - DIAMETER) / 2;
    for (left, ring) in lefts {
        for py in top..top + DIAMETER {
            for px in left..left + DIAMETER {
                let offset = (usize_of(py) * usize_of(width) + usize_of(px)) * 4;
                let cx = f64::from(left) + f64::from(DIAMETER) / 2.0;
                let cy = f64::from(top) + f64::from(DIAMETER) / 2.0;
                if let Some(slot) = pixels.get_mut(offset..offset + 4) {
                    slot.copy_from_slice(&pixel(ring, f64::from(px) - cx, f64::from(py) - cy));
                }
            }
        }
    }
    Some(Rgba {
        width,
        height: HEIGHT,
        pixels,
    })
}

/// The pixel whose top-left corner is `(dx, dy)` from the ring centre, supersampled.
fn pixel(ring: Ring, dx: f64, dy: f64) -> [u8; 4] {
    let outer = f64::from(DIAMETER) / 2.0;
    let inner = outer - STROKE;
    let mut sum = [0.0; 4];
    for sy in 0..SAMPLES {
        for sx in 0..SAMPLES {
            let across = dx + (f64::from(sx) + 0.5) / f64::from(SAMPLES);
            let down = dy + (f64::from(sy) + 0.5) / f64::from(SAMPLES);
            let radius = across.hypot(down);
            if radius < inner || radius > outer {
                continue;
            }
            // Clockwise from 12 o'clock, 0..1.
            let turn = (across.atan2(-down) / TAU).rem_euclid(1.0);
            let color = match ring {
                Ring::Used { filled, level } => {
                    if turn * 100.0 < f64::from(filled) {
                        level.color()
                    } else {
                        TRACK
                    }
                }
                Ring::Unknown if (turn * DASHES * 2.0).floor() % 2.0 == 0.0 => UNKNOWN,
                Ring::Unknown => continue,
            };
            let Color([red, green, blue, alpha]) = color;
            sum[0] += red * alpha;
            sum[1] += green * alpha;
            sum[2] += blue * alpha;
            sum[3] += alpha;
        }
    }
    let count = f64::from(SAMPLES * SAMPLES);
    if sum[3] <= 0.0 {
        return [0; 4];
    }
    [
        byte(sum[0] / sum[3]),
        byte(sum[1] / sum[3]),
        byte(sum[2] / sum[3]),
        byte(sum[3] / count * 255.0),
    ]
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=255 first"
)]
fn byte(value: f64) -> u8 {
    value.round().clamp(0.0, 255.0) as u8
}

fn usize_of(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(picture: &Rgba, x: u32, y: u32) -> u8 {
        let offset = (usize_of(y) * usize_of(picture.width) + usize_of(x)) * 4 + 3;
        picture.pixels[offset]
    }

    fn rgb(picture: &Rgba, x: u32, y: u32) -> [u8; 3] {
        let offset = (usize_of(y) * usize_of(picture.width) + usize_of(x)) * 4;
        [
            picture.pixels[offset],
            picture.pixels[offset + 1],
            picture.pixels[offset + 2],
        ]
    }

    #[test]
    fn levels_follow_the_meter_thresholds() {
        assert_eq!(
            Ring::used(69.9),
            Ring::Used {
                filled: 70,
                level: Level::Normal
            }
        );
        assert_eq!(
            Ring::used(70.0),
            Ring::Used {
                filled: 70,
                level: Level::Warning
            }
        );
        assert_eq!(
            Ring::used(140.0),
            Ring::Used {
                filled: 100,
                level: Level::Danger
            }
        );
        assert_eq!(
            Ring::used(f64::NAN),
            Ring::Used {
                filled: 0,
                level: Level::Normal
            }
        );
        assert_eq!(
            Ring::used(-3.0),
            Ring::Used {
                filled: 0,
                level: Level::Normal
            }
        );
    }

    #[test]
    fn nothing_to_draw_is_none() {
        assert_eq!(render(&[]), None);
        assert_eq!(render(&[vec![], vec![]]), None);
    }

    #[test]
    fn width_counts_rings_and_profile_gaps() -> Result<(), Box<dyn std::error::Error>> {
        let one = render(&[vec![Ring::used(10.0)]]).ok_or("no picture")?;
        assert_eq!((one.width, one.height), (PADDING * 2 + DIAMETER, HEIGHT));
        assert_eq!(one.pixels.len(), usize_of(one.width * HEIGHT * 4));

        let three = render(&[
            vec![Ring::used(10.0), Ring::used(20.0)],
            vec![Ring::Unknown],
        ])
        .ok_or("no picture")?;
        assert_eq!(
            three.width,
            PADDING * 2 + DIAMETER * 3 + RING_GAP + GROUP_GAP
        );
        Ok(())
    }

    #[test]
    fn the_arc_fills_clockwise_from_the_top() -> Result<(), Box<dyn std::error::Error>> {
        let picture = render(&[vec![Ring::used(50.0)]]).ok_or("no picture")?;
        let centre = PADDING + DIAMETER / 2;
        let top = (HEIGHT - DIAMETER) / 2;
        // Right side, mid-stroke: filled with the normal colour.
        assert_eq!(rgb(&picture, centre + 12, HEIGHT / 2), [64, 120, 230]);
        assert_eq!(alpha(&picture, centre + 12, HEIGHT / 2), 255);
        // Left side: the translucent track.
        assert_eq!(rgb(&picture, centre - 13, HEIGHT / 2), [142, 142, 147]);
        assert!(alpha(&picture, centre - 13, HEIGHT / 2) < 200);
        // The centre and the corners stay transparent.
        assert_eq!(alpha(&picture, centre, HEIGHT / 2), 0);
        assert_eq!(alpha(&picture, PADDING, top), 0);
        Ok(())
    }

    /// Alphas of 96 points around the middle of the stroke.
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "points inside the picture"
    )]
    fn around(picture: &Rgba) -> Vec<u8> {
        let centre = f64::from(PADDING + DIAMETER / 2);
        let middle = f64::from(DIAMETER) / 2.0 - STROKE / 2.0;
        (0..96)
            .map(|k| {
                let angle = f64::from(k) / 96.0 * TAU;
                let x = centre + middle * angle.sin();
                let y = f64::from(HEIGHT / 2) - middle * angle.cos();
                alpha(picture, x.floor() as u32, y.floor() as u32)
            })
            .collect()
    }

    #[test]
    fn an_unknown_ring_is_dashed() -> Result<(), Box<dyn std::error::Error>> {
        let dashed = around(&render(&[vec![Ring::Unknown]]).ok_or("no picture")?);
        assert!(dashed.contains(&0));
        assert!(dashed.iter().any(|a| *a > 200));
        let solid = around(&render(&[vec![Ring::used(30.0)]]).ok_or("no picture")?);
        assert!(solid.iter().all(|a| *a > 0));
        Ok(())
    }
}
