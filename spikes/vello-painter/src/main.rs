//! The spike of plan step 8: the markup display list under `vello_cpu`.
//!
//! Four questions decide whether the painter moves to `vello`:
//!
//! 1. do `glifo` glyphs match `swash` glyphs within the golden tolerance;
//! 2. do per-corner radii, border sides and the gradient tile map without
//!    special cases;
//! 3. does an animated `clip-path` polygon cost what ours does;
//! 4. which `wgpu` major is `vello_gpu` on this week.
//!
//! This binary answers 2 and 3 by rendering the same documents through
//! both painters and comparing them, and prints what it could not
//! translate. Question 1 is `glyphs`, which draws one line of text both
//! ways. Question 4 is answered in the report, not here: `vello_gpu`
//! 0.1.0 on crates.io is a name reservation whose whole body is `fn
//! add(left: u64, right: u64)`, and the renderer is still
//! `vello_hybrid` 0.2.0, pinned to `wgpu` ^29.0.3 against the
//! compositor's 30.
//!
//! Run it from this directory:
//!
//! ```sh
//! cargo run --release -- emit   <dir>   # writes the scenes
//! # then, for each scene, geneva frame <dir>/<name>.json -o <dir>/ours-<name>.png
//! cargo run --release -- render <dir>   # renders and compares
//! ```

mod glyphs;
mod scenes;
mod translate;

use std::collections::BTreeMap;
use std::time::Instant;

use geneva_golden::Rgba8Image;
use geneva_html::Measure;
use geneva_render::{CpuRenderer, FileAssets, Renderer};
use geneva_timeline::Ratio;
use vello_cpu::{Level, Pixmap, RenderContext, RenderSettings};

/// A document with no text and no images needs nothing measured. The
/// scenes here are boxes only, so that what is compared is the drawing
/// and not two text engines disagreeing about a line's width.
struct NoText;

impl Measure for NoText {
    fn text(&mut self, _text: &str, _style: &geneva_html::Text, _width: Option<f32>) -> (f32, f32) {
        (0.0, 0.0)
    }

    fn image(&mut self, _src: &str) -> Option<(f32, f32)> {
        None
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let what = args.next().unwrap_or_default();
    let dir = args.next().unwrap_or_else(|| ".".to_owned());
    match what.as_str() {
        "emit" => emit(&dir),
        "render" => render(&dir),
        "glyphs" => glyphs::run("../../tests/golden/fonts/LiberationSans-Regular.ttf", &dir),
        _ => eprintln!("usage: vello-painter-spike (emit|render|glyphs) <dir>"),
    }
}

/// Writes each scene's markup and a timeline that draws it, so the CLI
/// can render the same document through our own painter.
fn emit(dir: &str) {
    std::fs::create_dir_all(dir).expect("the output directory");
    for scene in scenes::all() {
        let html = format!("{dir}/{}.html", scene.name);
        std::fs::write(&html, scene.html).expect("writing the markup");
        let json = format!(
            "{{ \"geneva\": \"0.4\",\n  \"output\": {{ \"width\": {}, \"height\": {}, \"fps\": 30, \"duration\": \"1s\", \"background\": \"black\" }},\n  \"assets\": {{ \"m\": {{ \"src\": \"{}.html\" }} }},\n  \"layers\": [ {{ \"clips\": [ {{ \"source\": {{ \"kind\": \"html\", \"asset\": \"m\" }}, \"duration\": \"1s\" }} ] }} ] }}\n",
            scene.width, scene.height, scene.name
        );
        std::fs::write(format!("{dir}/{}.json", scene.name), json).expect("writing the timeline");
    }
    println!("wrote {} scenes to {dir}", scenes::all().len());
}

/// Renders every scene through `vello_cpu` and compares it with the PNG
/// our painter left beside it.
fn render(dir: &str) {
    println!(
        "{:<22} {:>9} {:>9} {:>10} {:>8}  not translated",
        "scene", "vello ms", "ours ms", "differing", "max diff"
    );
    for scene in scenes::all() {
        let prepared = geneva_html::prepare(scene.html, "", &BTreeMap::new()).expect("prepares");
        let laid = prepared
            .layout(
                Some(scene.width as f32),
                Some(scene.height as f32),
                &mut NoText,
            )
            .expect("lays out");
        let mut missing: Vec<&str> = Vec::new();
        let mut ctx = RenderContext::new_with(
            scene.width as u16,
            scene.height as u16,
            RenderSettings {
                level: Level::try_detect().unwrap_or_else(Level::baseline),
                // One thread, as our own painter is, so the two times
                // are the same shape of measurement. A filter layer
                // panics in vello_cpu 0.2 when several threads run.
                num_threads: 0,
            },
        );
        let mut pixmap = Pixmap::new(scene.width as u16, scene.height as u16);
        let mut resources = vello_cpu::Resources::new();

        // Warm once, then time the run that counts, as the profile of
        // our own painter does.
        for round in 0..6 {
            let start = Instant::now();
            ctx.reset();
            missing.clear();
            // Groups open and close as the painter's own walk does: a
            // group is open while the boxes inside it are drawn.
            let mut stack: Vec<(usize, usize)> = Vec::new();
            for b in &laid.boxes {
                let chain = chain_of(&laid, b.group);
                while let Some((top, layers)) = stack.last().copied() {
                    if chain.contains(&top) {
                        break;
                    }
                    stack.pop();
                    for _ in 0..layers {
                        ctx.pop_layer();
                    }
                }
                for g in chain {
                    if stack.iter().any(|(open, _)| *open == g) {
                        continue;
                    }
                    let layers = translate::open_group(&mut ctx, &laid.groups[g]);
                    stack.push((g, layers));
                }
                missing.extend(translate::box_into(&mut ctx, b));
            }
            while let Some((_, layers)) = stack.pop() {
                for _ in 0..layers {
                    ctx.pop_layer();
                }
            }
            ctx.flush();
            ctx.render(&mut pixmap, &mut resources);
            if round == 5 {
                let ms = start.elapsed().as_secs_f64() * 1e3;
                let ours = ours_ms(dir, scene.name).unwrap_or(f64::NAN);
                let reference = std::fs::read(format!("{dir}/ours-{}.png", scene.name)).ok();
                let mine = to_rgba8(&pixmap, scene.width, scene.height);
                std::fs::write(
                    format!("{dir}/vello-{}.png", scene.name),
                    mine.to_png().expect("encodes"),
                )
                .expect("writing the picture");
                missing.sort_unstable();
                missing.dedup();
                let note = if missing.is_empty() {
                    "everything".to_owned()
                } else {
                    missing.join(", ")
                };
                match reference {
                    Some(bytes) => {
                        let theirs = Rgba8Image::from_png(&bytes).expect("reads");
                        let c = geneva_golden::compare(&mine, &theirs, 0);
                        println!(
                            "{:<22} {ms:>9.2} {ours:>9.2} {:>9.3}% {:>8}  {note}",
                            scene.name,
                            c.mismatch_fraction * 100.0,
                            c.max_channel_diff.iter().max().copied().unwrap_or(0),
                        );
                    }
                    None => println!(
                        "{:<22} {ms:>9.2} {ours:>9.2} {:>10} {:>8}  {note}",
                        scene.name, "-", "-"
                    ),
                }
            }
        }
    }
}

/// A group and every group it sits inside, outermost first.
fn chain_of(laid: &geneva_html::Laid, group: Option<usize>) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut g = group;
    while let Some(i) = g {
        chain.push(i);
        g = laid.groups[i].parent;
    }
    chain.reverse();
    chain
}

/// Our own renderer on the same scene: a whole frame from a renderer
/// with nothing in its caches, which is parse, layout, paint, one
/// full-frame composite and the turn into linear light. A markup box
/// that does not move is painted once and kept, so timing a second
/// render of the same frame times the cache and not the painter, and a
/// renderer of its own is built for each round.
fn ours_ms(dir: &str, name: &str) -> Option<f64> {
    let text = std::fs::read_to_string(format!("{dir}/{name}.json")).ok()?;
    let timeline = geneva_timeline::parse(&text).ok()?;
    let (comp, _) = geneva_timeline::resolve(&timeline);
    let comp = comp?;
    let mut ms = f64::MAX;
    for _ in 0..6 {
        let mut renderer = CpuRenderer::new(FileAssets::new(dir));
        let start = Instant::now();
        renderer.render_frame(&comp, Ratio::ZERO).ok()?;
        ms = ms.min(start.elapsed().as_secs_f64() * 1e3);
    }
    Some(ms)
}

/// The pixmap as the golden comparator takes it. `vello_cpu` hands back
/// premultiplied sRGB bytes; the comparator wants straight alpha.
fn to_rgba8(pixmap: &Pixmap, width: u32, height: u32) -> Rgba8Image {
    let mut out = Vec::with_capacity((width * height * 4) as usize);
    for p in pixmap.data() {
        let a = p.a;
        let un = |v: u8| {
            if a == 0 {
                0
            } else {
                ((u16::from(v) * 255 + u16::from(a) / 2) / u16::from(a)).min(255) as u8
            }
        };
        out.extend_from_slice(&[un(p.r), un(p.g), un(p.b), a]);
    }
    Rgba8Image::new(width, height, out)
}
