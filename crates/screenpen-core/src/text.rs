//! Plain text laid out with the system sans-serif font and turned into glyph outlines,
//! so text renders and hit-tests like every other annotation.

use std::sync::{Mutex, OnceLock};

use cosmic_text::{
    Attrs, Buffer, CacheKey, Family, FontSystem, Metrics, Shaping, Style, SwashCache, Weight,
};
use swash::zeno::Command;
use tiny_skia::{Path, PathBuilder};

use crate::annotation::TextStyle;
use crate::geom::Point;

struct Fonts {
    system: FontSystem,
    cache: SwashCache,
}

/// Loading system fonts takes a while, so it happens once, on first use.
fn fonts() -> &'static Mutex<Fonts> {
    static FONTS: OnceLock<Mutex<Fonts>> = OnceLock::new();
    FONTS.get_or_init(|| {
        Mutex::new(Fonts {
            system: FontSystem::new(),
            cache: SwashCache::new(),
        })
    })
}

fn family() -> Family<'static> {
    if cfg!(windows) {
        Family::Name("Segoe UI")
    } else {
        Family::SansSerif
    }
}

/// Distance between baselines.
pub fn line_height(font_size: f32) -> f32 {
    (font_size * 1.25).ceil()
}

/// Glyph outlines of `string` with the first line's box at `origin` (y-down).
/// Returns None for empty or whitespace-only text.
pub fn path(string: &str, font_size: f32, origin: Point) -> Option<Path> {
    path_styled(string, font_size, origin, &TextStyle::default())
}

/// Installed font families, sorted, without duplicates.
pub fn families() -> Vec<String> {
    let guard = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let mut names: Vec<String> = guard
        .system
        .db()
        .faces()
        .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// Like `path`, in the given family, weight and slant (falling back to the
/// default family when that one isn't installed).
pub fn path_styled(string: &str, font_size: f32, origin: Point, style: &TextStyle) -> Option<Path> {
    let mut guard = fonts().lock().unwrap_or_else(|e| e.into_inner());
    let Fonts { system, cache } = &mut *guard;

    let mut placed: Vec<(CacheKey, f32, f32)> = Vec::new();
    {
        let mut buffer = Buffer::new(system, Metrics::new(font_size, line_height(font_size)));
        let mut buffer = buffer.borrow_with(system);
        buffer.set_text(
            string,
            &Attrs::new()
                .family(match style.family.as_deref() {
                    Some(name) => Family::Name(name),
                    None => family(),
                })
                .weight(if style.bold {
                    Weight::BOLD
                } else {
                    Weight::NORMAL
                })
                .style(if style.italic {
                    Style::Italic
                } else {
                    Style::Normal
                }),
            Shaping::Advanced,
            None,
        );
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let key = glyph.physical((0.0, 0.0), 1.0).cache_key;
                let x = glyph.x + glyph.font_size * glyph.x_offset;
                let y = run.line_y + glyph.y - glyph.font_size * glyph.y_offset;
                placed.push((key, x, y));
            }
        }
    }

    let mut pb = PathBuilder::new();
    for (key, gx, gy) in placed {
        let Some(commands) = cache.get_outline_commands(system, key) else {
            continue;
        };
        // Outlines are y-up around the glyph origin; flip them onto the y-down baseline.
        let at = |v: swash::zeno::Vector| (origin.x + gx + v.x, origin.y + gy - v.y);
        for command in commands {
            match *command {
                Command::MoveTo(p) => {
                    let (x, y) = at(p);
                    pb.move_to(x, y);
                }
                Command::LineTo(p) => {
                    let (x, y) = at(p);
                    pb.line_to(x, y);
                }
                Command::QuadTo(c, p) => {
                    let ((cx, cy), (x, y)) = (at(c), at(p));
                    pb.quad_to(cx, cy, x, y);
                }
                Command::CurveTo(c1, c2, p) => {
                    let ((ax, ay), (bx, by), (x, y)) = (at(c1), at(c2), at(p));
                    pb.cubic_to(ax, ay, bx, by, x, y);
                }
                Command::Close => pb.close(),
            }
        }
    }
    pb.finish()
}
