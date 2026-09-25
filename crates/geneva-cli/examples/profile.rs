//! Times the CPU compositor and the plane conversion on the check scene
//! and on variants of it, so that the cost of each element shows, and
//! the GPU renderer rendering and packing the same frames when a device
//! can be opened (any device, a software one included).
//!
//!     cargo run --release -p geneva-cli --example profile

use std::time::Instant;

use geneva_render::{AssetSource, CpuRenderer, Frame, Image, RenderError, Renderer};
use geneva_timeline::{Composition, Ratio, load};

/// One image for every image asset: a frame-sized opaque gradient.
struct ImageAssets(Image);

impl AssetSource for ImageAssets {
    fn image(&mut self, _: &Composition, _: &str) -> Result<&Image, RenderError> {
        Ok(&self.0)
    }
}

fn gradient(width: u32, height: u32) -> Image {
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            data.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 128, 255]);
        }
    }
    Image::from_rgba8(width, height, &data)
}

const IMAGE_ALIGNED: &str =
    r##"{ "clips": [ { "source": { "kind": "image", "asset": "img" }, "duration": "10s" } ] }"##;
const IMAGE_HALF_PIXEL: &str = r##"{ "clips": [ { "source": { "kind": "image", "asset": "img" }, "duration": "10s",
        "transform": { "position": { "x": "50%", "y": "50%" }, "anchor": { "x": 0.5, "y": 0 } } } ] }"##;
const IMAGE_FADED: &str = r##"{ "clips": [ { "source": { "kind": "image", "asset": "img" }, "duration": "10s", "opacity": 0.5 } ] }"##;
const IMAGE_SCALED: &str = r##"{ "clips": [ { "source": { "kind": "image", "asset": "img" }, "duration": "10s", "fit": "cover", "transform": { "scale": 1.1 } } ] }"##;

const ELLIPSE: &str = r##"{ "clips": [ { "source": { "kind": "shape", "shape": "ellipse", "width": 240, "height": 240, "fill": "#ff8800" },
        "transform": { "position": { "keyframes": [ { "t": 0, "v": { "x": "15%", "y": "50%" }, "ease": "ease-in-out" }, { "t": "10s", "v": { "x": "85%", "y": "50%" } } ] },
                       "rotation": { "keyframes": [ { "t": 0, "v": 0 }, { "t": "10s", "v": 720 } ] } } } ] }"##;
const RECT: &str = r##"{ "clips": [ { "source": { "kind": "shape", "shape": "rect", "width": 400, "height": 90, "fill": "#101820c0", "radius": 16 },
        "transform": { "position": { "x": "50%", "y": "85%" } } } ] }"##;
const TEXT: &str = r##"{ "clips": [ { "source": { "kind": "text", "text": "geneva check", "size": 48, "color": "white" },
        "transform": { "position": { "x": "50%", "y": "85%" } } } ] }"##;
const FULL_IMAGE: &str = r##"{ "clips": [ { "source": { "kind": "composition", "composition": "bg" }, "duration": "10s" } ] }"##;

fn scene(width: u32, height: u32, layers: &[&str]) -> String {
    format!(
        r##"{{ "geneva": "1.0",
  "output": {{ "width": {width}, "height": {height}, "fps": 30, "duration": "10s", "background": "#1d2230" }},
  "assets": {{ "img": {{ "src": "img.png" }} }},
  "compositions": {{ "bg": {{ "width": {width}, "height": {height}, "background": "#304050", "layers": [ {RECT} ] }},
                     "small": {{ "width": 400, "height": 90, "background": "#304050", "layers": [ {RECT} ] }} }},
  "layers": [ {} ] }}"##,
        layers.join(", ")
    )
}

fn time(name: &str, width: u32, height: u32, layers: &[&str], frames: u32) {
    let loaded = load(&scene(width, height, layers));
    let comp = loaded.composition.expect("scene is valid");
    let mut renderer = CpuRenderer::new(ImageAssets(gradient(width, height)));
    let mut frame = Frame::new(width, height, geneva_color::Color::BLACK);
    let tags = geneva_media::output_tags_for(geneva_timeline::schema::VideoCodec::H264, comp.color);
    // Warm up: fonts and the text cache.
    renderer
        .render_into(&comp, Ratio::ZERO, &mut frame)
        .unwrap();
    let mut render_ms = 0.0;
    let mut convert_ms = 0.0;
    for n in 0..frames {
        let t = Ratio::new(i64::from(n), 30);
        let started = Instant::now();
        renderer.render_into(&comp, t, &mut frame).unwrap();
        render_ms += started.elapsed().as_secs_f64() * 1e3;
        let started = Instant::now();
        let planes = geneva_media::convert::frame_to_planes(
            &frame,
            tags,
            geneva_media::convert::PlaneFormat::Yuv420p8,
        );
        convert_ms += started.elapsed().as_secs_f64() * 1e3;
        std::hint::black_box(planes);
    }
    let gpu_ms = gpu_time(&comp, width, height, tags, frames);
    println!(
        "{name:<34} {width}x{height}  render {:6.2} ms  convert {:6.2} ms  gpu {}",
        render_ms / f64::from(frames),
        convert_ms / f64::from(frames),
        gpu_ms.map_or("n/a".to_owned(), |ms| format!("{ms:6.2} ms"))
    );
}

/// The frames rendered and packed on the device, prepared one ahead as
/// `geneva render` does, in milliseconds a frame; `None` without a
/// device.
fn gpu_time(
    comp: &Composition,
    width: u32,
    height: u32,
    tags: geneva_color::ResolvedTags,
    frames: u32,
) -> Option<f64> {
    use geneva_gpu::{Gpu, GpuRenderer, Preference};
    use geneva_media::convert::{PlaneFormat, Planes};
    use geneva_media::{PlaneRenderer, PlaneTarget};
    let gpu = Gpu::probe(Preference::from_env(Preference::Any)).ok()?;
    let mut renderer = GpuRenderer::new(gpu, ImageAssets(gradient(width, height)));
    let format = PlaneFormat::Yuv420p8;
    let mut planes = Planes::new(format, width, height);
    let mut draw = |t: Ratio, next: Option<Ratio>| {
        if let Some(next) = next {
            renderer.prepare_planes(comp, next, &[(tags, format)]);
        }
        renderer
            .render_planes(
                comp,
                t,
                &mut [PlaneTarget {
                    tags,
                    format,
                    planes: &mut planes,
                }],
                None,
            )
            .unwrap();
    };
    draw(Ratio::ZERO, None);
    let started = Instant::now();
    for n in 0..frames {
        let t = Ratio::new(i64::from(n), 30);
        let next = (n + 1 < frames).then(|| Ratio::new(i64::from(n + 1), 30));
        draw(t, next);
    }
    Some(started.elapsed().as_secs_f64() * 1e3 / f64::from(frames))
}

fn scaled(scale: &str, rotation: &str) -> String {
    format!(
        r##"{{ "clips": [ {{ "source": {{ "kind": "image", "asset": "img" }}, "duration": "10s", "fit": "cover", "transform": {{ "scale": {scale}, "rotation": {rotation} }} }} ] }}"##
    )
}

/// A video reader as an asset source, serving the frame at one time.
struct Reader(geneva_media::VideoReader, Ratio);

impl AssetSource for Reader {
    fn image(&mut self, _: &Composition, _: &str) -> Result<&Image, RenderError> {
        self.0.frame_at(self.1).map_err(|e| RenderError::Asset {
            id: "v".into(),
            reason: e.to_string(),
        })
    }
}

/// Stages of the composited path on a real file: decode, decode and
/// convert, draw, pack to 4:2:0.
fn video_stages(path: &str) {
    use geneva_color::ColorTags;
    use geneva_media::VideoReader;
    let n = 120u32;
    let mut reader = VideoReader::open(std::path::Path::new(path), ColorTags::default()).unwrap();
    let dur = reader.frame_duration();
    let started = Instant::now();
    for k in 0..n {
        std::hint::black_box(
            reader
                .raw_frame_at(dur * Ratio::from_int(i64::from(k)))
                .unwrap(),
        );
    }
    println!(
        "decode only                 {:6.2} ms/frame",
        started.elapsed().as_secs_f64() * 1e3 / f64::from(n)
    );
    let mut reader = VideoReader::open(std::path::Path::new(path), ColorTags::default()).unwrap();
    let started = Instant::now();
    for k in 0..n {
        std::hint::black_box(
            reader
                .frame_at(dur * Ratio::from_int(i64::from(k)))
                .unwrap(),
        );
    }
    println!(
        "decode + convert            {:6.2} ms/frame",
        started.elapsed().as_secs_f64() * 1e3 / f64::from(n)
    );
    // Draw each converted frame onto an output frame and pack it.
    let (w, h) = {
        let img = reader.frame_at(Ratio::ZERO).unwrap();
        (img.width, img.height)
    };
    let loaded = load(&scene(w, h, &[IMAGE_ALIGNED]));
    let comp = loaded.composition.expect("scene is valid");
    let tags = geneva_media::output_tags_for(geneva_timeline::schema::VideoCodec::H264, comp.color);
    let mut renderer = CpuRenderer::new(Reader(
        VideoReader::open(std::path::Path::new(path), ColorTags::default()).unwrap(),
        Ratio::ZERO,
    ));
    let mut frame = Frame::new(w, h, geneva_color::Color::BLACK);
    let (mut render_ms, mut pack_ms) = (0.0, 0.0);
    for k in 0..n {
        let t = dur * Ratio::from_int(i64::from(k));
        renderer.assets_mut().1 = t;
        let started = Instant::now();
        renderer.render_into(&comp, t, &mut frame).unwrap();
        render_ms += started.elapsed().as_secs_f64() * 1e3;
        let started = Instant::now();
        std::hint::black_box(geneva_media::convert::frame_to_planes(
            &frame,
            tags,
            geneva_media::convert::PlaneFormat::Yuv420p8,
        ));
        pack_ms += started.elapsed().as_secs_f64() * 1e3;
    }
    println!(
        "decode + convert + draw     {:6.2} ms/frame",
        render_ms / f64::from(n)
    );
    println!(
        "pack to 4:2:0               {:6.2} ms/frame",
        pack_ms / f64::from(n)
    );
}

/// The two decoded-frame conversions on synthetic 1080p planes.
fn conversions() {
    use geneva_media::convert::{Planes16, Planes420, ycbcr16_into, yuv420p8_into};
    let (w, h) = (1920usize, 1080usize);
    let y: Vec<u8> = (0..w * h).map(|i| (i % 220) as u8 + 16).collect();
    let c: Vec<u8> = (0..(w / 2) * (h / 2))
        .map(|i| (i % 200) as u8 + 28)
        .collect();
    let mut img = Image {
        width: 0,
        height: 0,
        pixels: Vec::new(),
        content: None,
    };
    let tags = geneva_color::ResolvedTags::SDR_VIDEO;
    let n = 50;
    yuv420p8_into(
        &Planes420 {
            y: &y,
            cb: &c,
            cr: &c,
            y_stride: w,
            c_stride: w / 2,
        },
        w as u32,
        h as u32,
        tags,
        &mut img,
    );
    let started = Instant::now();
    for _ in 0..n {
        yuv420p8_into(
            &Planes420 {
                y: &y,
                cb: &c,
                cr: &c,
                y_stride: w,
                c_stride: w / 2,
            },
            w as u32,
            h as u32,
            tags,
            &mut img,
        );
    }
    println!(
        "yuv420p8_into (8-bit 4:2:0)   {:6.2} ms",
        started.elapsed().as_secs_f64() * 1e3 / f64::from(n)
    );
    let y16: Vec<u8> = (0..w * h)
        .flat_map(|i| (((i % 220) as u16 + 16) << 8).to_le_bytes())
        .collect();
    let started = Instant::now();
    for _ in 0..n {
        ycbcr16_into(
            &Planes16 {
                y: &y16,
                cb: &y16,
                cr: &y16,
                stride: w * 2,
            },
            w as u32,
            h as u32,
            tags,
            None,
            &mut img,
        );
    }
    println!(
        "ycbcr16_into (16-bit 4:4:4)   {:6.2} ms",
        started.elapsed().as_secs_f64() * 1e3 / f64::from(n)
    );
    println!(
        "threads available: {}",
        std::thread::available_parallelism().map_or(1, usize::from)
    );
}

fn main() {
    let frames = 60;
    if std::env::args().nth(1).as_deref() == Some("convert") {
        return conversions();
    }
    if let (Some("video"), Some(path)) =
        (std::env::args().nth(1).as_deref(), std::env::args().nth(2))
    {
        return video_stages(&path);
    }
    match std::env::args().nth(1).as_deref() {
        // A few frames of one case at 720p, for a profiler.
        Some("nested") => return time("nested", 1280, 720, &[FULL_IMAGE], 4),
        Some("scaled") => return time("scaled", 1280, 720, &[&scaled("1.1", "0")], 4),
        _ => {}
    }
    if std::env::args().nth(1).as_deref() == Some("scale") {
        let (w, h) = (1920, 1080);
        for (scale, rot) in [
            ("1.0", "0"),
            ("1.001", "0"),
            ("1.1", "0"),
            ("1.5", "0"),
            ("2.0", "0"),
            ("0.5", "0"),
            ("1.0", "1"),
            ("1.0", "45"),
        ] {
            time(
                &format!("image scale {scale} rot {rot}"),
                w,
                h,
                &[&scaled(scale, rot)],
                frames,
            );
        }
        time(
            "small nested composition (400x90)",
            w,
            h,
            &[
                r##"{ "clips": [ { "source": { "kind": "composition", "composition": "small" }, "duration": "10s" } ] }"##,
            ],
            frames,
        );
        return;
    }
    for (w, h) in [(1280, 720), (1920, 1080)] {
        time("background only", w, h, &[], frames);
        time("ellipse (rotating, moving)", w, h, &[ELLIPSE], frames);
        time("rounded rect (static)", w, h, &[RECT], frames);
        time("text (static, cached)", w, h, &[TEXT], frames);
        time(
            "check scene (all three)",
            w,
            h,
            &[ELLIPSE, RECT, TEXT],
            frames,
        );
        time(
            "frame-sized nested composition",
            w,
            h,
            &[FULL_IMAGE],
            frames,
        );
        time("frame-sized image, aligned", w, h, &[IMAGE_ALIGNED], frames);
        time(
            "frame-sized image, half-pixel off",
            w,
            h,
            &[IMAGE_HALF_PIXEL],
            frames,
        );
        time(
            "frame-sized image, opacity 0.5",
            w,
            h,
            &[IMAGE_FADED],
            frames,
        );
        time(
            "frame-sized image, scaled 1.1",
            w,
            h,
            &[IMAGE_SCALED],
            frames,
        );
        let started = Instant::now();
        for _ in 0..frames {
            std::hint::black_box(Frame::new(w, h, geneva_color::Color::BLACK));
        }
        println!(
            "{:<34} {w}x{h}  {:6.2} ms",
            "Frame::new (alloc + clear)",
            started.elapsed().as_secs_f64() * 1e3 / f64::from(frames)
        );
        let f = Frame::new(w, h, geneva_color::Color::BLACK);
        let started = Instant::now();
        for _ in 0..frames {
            std::hint::black_box(Image::from_frame(&f));
        }
        println!(
            "{:<34} {w}x{h}  {:6.2} ms",
            "Image::from_frame (copy)",
            started.elapsed().as_secs_f64() * 1e3 / f64::from(frames)
        );
        println!();
    }
}
