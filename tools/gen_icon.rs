//! Procedurally generates the app icon — a stylised stock-chart zigzag in
//! green on a transparent background. Run with:
//!
//!     cargo run --example gen-icon
//!
//! Writes:
//!   - assets/icon.ico        (multi-size: 256/128/64/48/32/16)
//!   - assets/icon-tray.png   (32x32, used by the system tray at runtime)
//!
//! Tweak the constants below (path points, line thickness, colour) and
//! re-run to refresh the assets.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

// 4 normalized (0..1) points defining the chart line. y=0 is top, y=1 bottom.
// Overall trend: rising from bottom-left to top-right with one realistic dip.
const POINTS: [(f32, f32); 4] = [
    (0.05, 0.75),
    (0.35, 0.45),
    (0.60, 0.60),
    (0.95, 0.10),
];

const LINE_THICKNESS_AT_256: f32 = 28.0; // scales linearly with output size
const LINE_COLOR: [u8; 3] = [0x4C, 0xAF, 0x50]; // theme.up green
const SHADOW_COLOR: [u8; 3] = [0x1B, 0x5E, 0x20]; // darker green, drawn under
const SHADOW_OFFSET: f32 = 0.018; // fraction of canvas

fn distance_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py) = p;
    let (ax, ay) = a;
    let (bx, by) = b;
    let abx = bx - ax;
    let aby = by - ay;
    let len_sq = abx * abx + aby * aby;
    let t = if len_sq > 0.0 {
        ((px - ax) * abx + (py - ay) * aby) / len_sq
    } else {
        0.0
    };
    let t = t.clamp(0.0, 1.0);
    let closest_x = ax + t * abx;
    let closest_y = ay + t * aby;
    let dx = px - closest_x;
    let dy = py - closest_y;
    (dx * dx + dy * dy).sqrt()
}

fn min_distance_to_polyline(p: (f32, f32), pts: &[(f32, f32)]) -> f32 {
    let mut min = f32::INFINITY;
    // Segments
    for w in pts.windows(2) {
        let d = distance_to_segment(p, w[0], w[1]);
        if d < min {
            min = d;
        }
    }
    // Round joints
    for &pt in pts {
        let dx = p.0 - pt.0;
        let dy = p.1 - pt.1;
        let d = (dx * dx + dy * dy).sqrt();
        if d < min {
            min = d;
        }
    }
    min
}

/// Coverage from anti-aliased distance: 1.0 fully inside the stroke,
/// 0.0 fully outside, smooth across a 1-pixel edge band.
fn coverage(dist: f32, half_thickness: f32) -> f32 {
    let outer = half_thickness + 0.5;
    let inner = half_thickness - 0.5;
    if dist <= inner {
        1.0
    } else if dist >= outer {
        0.0
    } else {
        (outer - dist).clamp(0.0, 1.0)
    }
}

fn blend(dst: &mut [u8; 4], rgb: [u8; 3], alpha: f32) {
    let a = (alpha * 255.0).round().clamp(0.0, 255.0) as u16;
    let inv = 255 - a;
    dst[0] = ((dst[0] as u16 * inv + rgb[0] as u16 * a) / 255) as u8;
    dst[1] = ((dst[1] as u16 * inv + rgb[1] as u16 * a) / 255) as u8;
    dst[2] = ((dst[2] as u16 * inv + rgb[2] as u16 * a) / 255) as u8;
    // Alpha: src over dst alpha
    let a_dst = dst[3] as u16;
    dst[3] = (a_dst + a * (255 - a_dst) / 255).min(255) as u8;
}

fn render_icon(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];

    let margin = 0.08 * s;
    let usable = s - 2.0 * margin;
    let pts: Vec<(f32, f32)> = POINTS
        .iter()
        .map(|&(x, y)| (margin + x * usable, margin + y * usable))
        .collect();

    let thickness = LINE_THICKNESS_AT_256 * (s / 256.0);
    let half = thickness * 0.5;
    let shadow_dx = SHADOW_OFFSET * s;
    let shadow_dy = SHADOW_OFFSET * s;

    let shadow_pts: Vec<(f32, f32)> =
        pts.iter().map(|&(x, y)| (x + shadow_dx, y + shadow_dy)).collect();

    for y in 0..size {
        for x in 0..size {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let idx = ((y * size + x) * 4) as usize;
            let mut px = [rgba[idx], rgba[idx + 1], rgba[idx + 2], rgba[idx + 3]];

            // Shadow stroke first (drawn under the main line)
            let d_shadow = min_distance_to_polyline(p, &shadow_pts);
            let cov_shadow = coverage(d_shadow, half);
            if cov_shadow > 0.0 {
                blend(&mut px, SHADOW_COLOR, cov_shadow * 0.55);
            }
            // Main stroke on top
            let d_line = min_distance_to_polyline(p, &pts);
            let cov_line = coverage(d_line, half);
            if cov_line > 0.0 {
                blend(&mut px, LINE_COLOR, cov_line);
            }

            rgba[idx..idx + 4].copy_from_slice(&px);
        }
    }
    rgba
}

fn write_ico(path: &Path, sizes: &[u32]) {
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for &size in sizes {
        let rgba = render_icon(size);
        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        dir.add_entry(ico::IconDirEntry::encode(&image).expect("ico encode"));
    }
    let file = File::create(path).expect("create .ico");
    dir.write(BufWriter::new(file)).expect("write .ico");
    println!("wrote {}", path.display());
}

fn write_png(path: &Path, size: u32) {
    let rgba = render_icon(size);
    let file = File::create(path).expect("create png");
    let mut encoder = png::Encoder::new(BufWriter::new(file), size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("png header");
    writer.write_image_data(&rgba).expect("png data");
    println!("wrote {}", path.display());
}

fn main() {
    std::fs::create_dir_all("assets").expect("mkdir assets");
    write_ico(
        Path::new("assets/icon.ico"),
        &[256, 128, 64, 48, 32, 16],
    );
    write_png(Path::new("assets/icon-tray.png"), 32);
}
