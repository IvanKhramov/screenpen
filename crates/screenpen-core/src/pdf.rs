//! A one-page PDF of a screen: the background as an image and every annotation as
//! vector paths (the same filled outlines the screen shows), so they stay sharp and
//! can be selected in an editor.

use std::fmt::Write as _;

use tiny_skia::PathSegment;

use crate::annotation::Annotation;
use crate::render::{Fill, render_parts};
use crate::style::Rgba;

/// An opaque RGB image, rows top to bottom.
pub struct RgbImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// A page `width` x `height` points (the screen size in points).
pub fn export(
    annotations: &[Annotation],
    width: f32,
    height: f32,
    background: Option<&RgbImage>,
) -> Vec<u8> {
    let mut content = String::new();
    if background.is_some() {
        // The image's unit square, scaled to the page; images are drawn top row up.
        let _ = writeln!(content, "q {width} 0 0 {height} 0 0 cm /Bg Do Q");
    }
    // Annotations are in y-down screen points; flip the page to match.
    let _ = writeln!(content, "1 0 0 -1 0 {height} cm");
    let mut alphas: Vec<f32> = Vec::new();
    for a in annotations {
        for part in render_parts(a) {
            let color = match part.fill {
                Fill::Style => a.style.color,
                Fill::Contrast => a.style.color.contrasting(),
                Fill::Plate => a.style.color.contrasting().with_alpha(0.55),
            };
            let alpha = (color.alpha * a.style.opacity).clamp(0.0, 1.0);
            let gs = alphas.iter().position(|&x| x == alpha).unwrap_or_else(|| {
                alphas.push(alpha);
                alphas.len() - 1
            });
            let Rgba {
                red, green, blue, ..
            } = color;
            let _ = writeln!(content, "/G{gs} gs {red:.4} {green:.4} {blue:.4} rg");
            path(&mut content, &part.path);
            content.push_str("f\n");
        }
    }

    let mut pdf = Writer::default();
    pdf.raw(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");
    pdf.object(1, "<< /Type /Catalog /Pages 2 0 R >>");
    pdf.object(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let states: String = alphas
        .iter()
        .enumerate()
        .map(|(i, _)| format!("/G{i} {} 0 R ", 6 + i))
        .collect();
    let image = if background.is_some() {
        "/XObject << /Bg 5 0 R >> "
    } else {
        ""
    };
    pdf.object(
        3,
        &format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents 4 0 R \
             /Resources << {image}/ExtGState << {states}>> >> >>"
        ),
    );
    pdf.stream(4, "", content.as_bytes());
    match background {
        Some(bg) => pdf.stream(
            5,
            &format!("/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8", bg.width, bg.height),
            &bg.rgb,
        ),
        // Keep object numbers fixed: an empty placeholder.
        None => pdf.object(5, "null"),
    }
    for (i, alpha) in alphas.iter().enumerate() {
        pdf.object(
            6 + i,
            &format!("<< /Type /ExtGState /ca {alpha} /CA {alpha} >>"),
        );
    }
    pdf.finish(1)
}

/// Appends PDF path operators for a tiny-skia path.
fn path(out: &mut String, path: &tiny_skia::Path) {
    let mut last = (0.0f32, 0.0f32);
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                let _ = writeln!(out, "{:.3} {:.3} m", p.x, p.y);
                last = (p.x, p.y);
            }
            PathSegment::LineTo(p) => {
                let _ = writeln!(out, "{:.3} {:.3} l", p.x, p.y);
                last = (p.x, p.y);
            }
            PathSegment::QuadTo(c, p) => {
                // PDF has only cubics: raise the quadratic.
                let c1 = (
                    last.0 + (c.x - last.0) * 2.0 / 3.0,
                    last.1 + (c.y - last.1) * 2.0 / 3.0,
                );
                let c2 = (p.x + (c.x - p.x) * 2.0 / 3.0, p.y + (c.y - p.y) * 2.0 / 3.0);
                let _ = writeln!(
                    out,
                    "{:.3} {:.3} {:.3} {:.3} {:.3} {:.3} c",
                    c1.0, c1.1, c2.0, c2.1, p.x, p.y
                );
                last = (p.x, p.y);
            }
            PathSegment::CubicTo(c1, c2, p) => {
                let _ = writeln!(
                    out,
                    "{:.3} {:.3} {:.3} {:.3} {:.3} {:.3} c",
                    c1.x, c1.y, c2.x, c2.y, p.x, p.y
                );
                last = (p.x, p.y);
            }
            PathSegment::Close => out.push_str("h\n"),
        }
    }
}

#[derive(Default)]
struct Writer {
    bytes: Vec<u8>,
    offsets: Vec<(usize, usize)>,
}

impl Writer {
    fn raw(&mut self, b: &[u8]) {
        self.bytes.extend_from_slice(b);
    }

    fn object(&mut self, id: usize, body: &str) {
        self.offsets.push((id, self.bytes.len()));
        self.raw(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
    }

    /// A compressed stream object; `dict` holds extra entries.
    fn stream(&mut self, id: usize, dict: &str, data: &[u8]) {
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(data, 6);
        self.offsets.push((id, self.bytes.len()));
        self.raw(
            format!(
                "{id} 0 obj\n<< {dict} /Length {} /Filter /FlateDecode >>\nstream\n",
                packed.len()
            )
            .as_bytes(),
        );
        self.raw(&packed);
        self.raw(b"\nendstream\nendobj\n");
    }

    fn finish(mut self, root: usize) -> Vec<u8> {
        self.offsets.sort();
        let count = self.offsets.len() + 1;
        let xref = self.bytes.len();
        let mut table = format!("xref\n0 {count}\n0000000000 65535 f \n");
        for (_, offset) in &self.offsets {
            let _ = writeln!(table, "{offset:010} 00000 n ");
        }
        let _ = write!(
            table,
            "trailer\n<< /Size {count} /Root {root} 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        );
        self.raw(table.as_bytes());
        self.bytes
    }
}
