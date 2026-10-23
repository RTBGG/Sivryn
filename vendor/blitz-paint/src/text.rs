use anyrender::PaintScene;
use blitz_dom::{BaseDocument, node::TextBrush, util::ToColorColor};
use kurbo::{Affine, Point, Stroke};
use parley::{Line, PositionedLayoutItem};
use peniko::Fill;
use style::values::computed::TextDecorationLine;

pub(crate) fn stroke_text<'a>(
    scale: f64,
    scene: &mut impl PaintScene,
    lines: impl Iterator<Item = Line<'a, TextBrush>>,
    doc: &BaseDocument,
    pos: Point,
) {
    stroke_text_cut(scale, scene, lines, doc, pos, None);
}

/// Sivryn patch: [`stroke_text`], cut off with an ellipsis where text would
/// pass `cut_at` (layout units). Drawn as three dots in the text's colour,
/// since a "…" glyph would need shaping here.
pub(crate) fn stroke_text_cut<'a>(
    scale: f64,
    scene: &mut impl PaintScene,
    lines: impl Iterator<Item = Line<'a, TextBrush>>,
    doc: &BaseDocument,
    pos: Point,
    cut_at: Option<f32>,
) {
    // Where the last glyph that fit ends, with the run's size and colour.
    let mut cut: Option<(f32, f32, f32, color::AlphaColor<color::Srgb>)> = None;
    // Sivryn patch: snap baselines to whole device pixels. Glyph hinting
    // aligns outlines vertically to the pixel grid, which only sharpens text
    // when the baseline itself sits on a pixel boundary.
    let transform = Affine::translate((pos.x * scale, (pos.y * scale).round()));
    for line in lines {
        for item in line.items() {
            if let PositionedLayoutItem::GlyphRun(glyph_run) = item {
                let mut x = glyph_run.offset();
                let y = glyph_run.baseline().round();

                let run = glyph_run.run();
                let font = run.font();
                let font_size = run.font_size();
                let metrics = run.metrics();
                let style = glyph_run.style();
                let synthesis = run.synthesis();
                let glyph_xform = synthesis
                    .skew()
                    .map(|angle| Affine::skew(angle.to_radians().tan() as f64, 0.0));

                // Styles
                let styles = doc
                    .get_node(style.brush.id)
                    .unwrap()
                    .primary_styles()
                    .unwrap();
                let itext_styles = styles.get_inherited_text();
                let text_styles = styles.get_text();
                let text_color = itext_styles.color.as_color_color();
                let text_decoration_color = text_styles
                    .text_decoration_color
                    .as_absolute()
                    .map(ToColorColor::as_color_color)
                    .unwrap_or(text_color);
                let text_decoration_brush = anyrender::Paint::from(text_decoration_color);
                let text_decoration_line = text_styles.text_decoration_line;
                let has_underline = text_decoration_line.contains(TextDecorationLine::UNDERLINE);
                let has_strikethrough =
                    text_decoration_line.contains(TextDecorationLine::LINE_THROUGH);

                // Room for the dots after the last glyph that fits.
                let limit = cut_at.map(|c| c - font_size * 0.9);
                let mut end = None;
                scene.draw_glyphs(
                    font,
                    font_size,
                    true, // hint
                    run.normalized_coords(),
                    Fill::NonZero,
                    &anyrender::Paint::from(text_color),
                    1.0, // alpha
                    transform,
                    glyph_xform,
                    glyph_run.glyphs().filter_map(|glyph| {
                        let gx = x + glyph.x;
                        let gy = y - glyph.y;
                        x += glyph.advance;
                        if limit.is_some_and(|l| x > l) {
                            return None;
                        }
                        end = Some(x);

                        Some(anyrender::Glyph {
                            id: glyph.id as _,
                            x: gx,
                            y: gy,
                        })
                    }),
                );
                if let Some(limit) = limit
                    && x > limit
                    && cut.is_none()
                {
                    cut = Some((end.unwrap_or(glyph_run.offset()), y, font_size, text_color));
                }

                let mut draw_decoration_line =
                    |offset: f32, size: f32, brush: &anyrender::Paint| {
                        let x = glyph_run.offset() as f64;
                        let w = glyph_run.advance() as f64;
                        let y = (glyph_run.baseline() - offset + size / 2.0) as f64;
                        let line = kurbo::Line::new((x, y), (x + w, y));
                        scene.stroke(&Stroke::new(size as f64), transform, brush, None, &line)
                    };

                if has_underline {
                    let offset = metrics.underline_offset;
                    let size = metrics.underline_size;

                    // TODO: intercept line when crossing an descending character like "gqy"
                    draw_decoration_line(offset, size, &text_decoration_brush);
                }
                if has_strikethrough {
                    let offset = metrics.strikethrough_offset;
                    let size = metrics.strikethrough_size;

                    draw_decoration_line(offset, size, &text_decoration_brush);
                }
            }
        }
    }
    if let Some((x, y, size, color)) = cut {
        let r = (size * 0.075).max(0.75) as f64;
        for i in 0..3 {
            let cx = (x + size * (0.18 + 0.27 * i as f32)) as f64;
            let dot = kurbo::Circle::new((cx, y as f64 - r), r);
            scene.fill(Fill::NonZero, transform, color, None, &dot);
        }
    }
}
