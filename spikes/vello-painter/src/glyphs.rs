//! Question 1 of the spike: do `glifo` glyphs match `swash` glyphs?
//!
//! The painter rasterizes a glyph with `swash`, unhinted, as a coverage
//! mask, and blits it at the pen position plus the placement swash
//! reports. `vello_cpu` rasterizes the same outline through `glifo`.
//! Both are asked for the same glyph of the same face at the same size,
//! with the pen at the same place, and what comes back is compared as
//! coverage: the placement and the antialiasing together, which is what
//! the goldens would see.

use geneva_golden::Rgba8Image;
use swash::FontRef;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Vector};
use vello_cpu::kurbo::Affine;
use vello_cpu::peniko::color::{AlphaColor, Srgb};
use vello_cpu::peniko::{Blob, FontData};
use vello_cpu::{Glyph, Level, PaintType, Pixmap, RenderContext, RenderSettings, Resources};

/// The surface each glyph is drawn on, and where the pen sits on it.
const SIZE: u32 = 160;
const PEN: (f32, f32) = (30.0, 120.0);

/// Draws each character at each size both ways and prints the table.
pub fn run(font_path: &str, out: &str) {
    let bytes = std::fs::read(font_path).expect("the font file");
    let font = FontRef::from_index(&bytes, 0).expect("a usable face");
    let font_data = FontData::new(Blob::new(std::sync::Arc::new(bytes.clone())), 0);
    let mut scale = ScaleContext::new();

    println!(
        "{:<6} {:>5} {:>10} {:>10} {:>9} {:>8}",
        "glyph", "size", "of the ink", "max diff", "pixels", "ink"
    );
    for size in [16.0f32, 24.0, 76.0] {
        for c in ['g', 'a', 'W', '3'] {
            let id = font.charmap().map(c);
            let mut scaler = scale.builder(font).size(size).hint(false).build();
            let rendered = Render::new(&[Source::Outline, Source::Bitmap(StrikeWith::BestFit)])
                .format(Format::Alpha)
                .offset(Vector::new(0.0, 0.0))
                .render(&mut scaler, id)
                .expect("swash rasterizes it");

            // Ours: the coverage mask laid at the pen, as the painter
            // lays it.
            let mut ours = vec![0u8; (SIZE * SIZE * 4) as usize];
            let left = PEN.0 as i32 + rendered.placement.left;
            let top = PEN.1 as i32 - rendered.placement.top;
            let w = rendered.placement.width as i32;
            for (i, v) in rendered.data.iter().enumerate() {
                let (x, y) = (left + i as i32 % w, top + i as i32 / w);
                if x < 0 || y < 0 || x >= SIZE as i32 || y >= SIZE as i32 {
                    continue;
                }
                let at = ((y as u32 * SIZE + x as u32) * 4) as usize;
                ours[at + 3] = *v;
            }

            // Theirs: the same glyph through vello_cpu.
            let mut ctx = RenderContext::new_with(
                SIZE as u16,
                SIZE as u16,
                RenderSettings {
                    level: Level::try_detect().unwrap_or_else(Level::baseline),
                    num_threads: 0,
                },
            );
            let mut resources = Resources::new();
            let mut pixmap = Pixmap::new(SIZE as u16, SIZE as u16);
            ctx.set_paint(PaintType::Solid(AlphaColor::<Srgb>::new([
                0.0, 0.0, 0.0, 1.0,
            ])));
            ctx.set_transform(Affine::IDENTITY);
            ctx.glyph_run(&mut resources, &font_data)
                .font_size(size)
                .fill_glyphs(
                    [Glyph {
                        id: u32::from(id),
                        x: PEN.0,
                        y: PEN.1,
                    }]
                    .into_iter(),
                );
            ctx.flush();
            ctx.render(&mut pixmap, &mut resources);
            let theirs: Vec<u8> = pixmap.data().iter().flat_map(|p| [0, 0, 0, p.a]).collect();

            let a = Rgba8Image::new(SIZE, SIZE, ours);
            let b = Rgba8Image::new(SIZE, SIZE, theirs);
            // The goldens allow two codes a channel and 0.2% of the
            // frame's pixels beyond that, so the difference is counted
            // the same way here. Over the glyph's own box rather than
            // the surface, since that is what scales to a page of text.
            let c_ = geneva_golden::compare(&a, &b, 2);
            let ink = a
                .data
                .chunks_exact(4)
                .zip(b.data.chunks_exact(4))
                .filter(|(p, q)| p[3] > 0 || q[3] > 0)
                .count();
            let over = a
                .data
                .chunks_exact(4)
                .zip(b.data.chunks_exact(4))
                .filter(|(p, q)| p[3].abs_diff(q[3]) > 2)
                .count();
            println!(
                "{c:<6} {size:>5} {:>9.2}% {:>10} {:>9} {:>8}",
                100.0 * over as f64 / ink.max(1) as f64,
                c_.max_channel_diff.iter().max().copied().unwrap_or(0),
                over,
                ink
            );
            if size == 76.0 {
                std::fs::write(
                    format!("{out}/glyph-{c}-swash.png"),
                    a.to_png().expect("encodes"),
                )
                .expect("writes");
                std::fs::write(
                    format!("{out}/glyph-{c}-glifo.png"),
                    b.to_png().expect("encodes"),
                )
                .expect("writes");
            }
        }
    }
}
