//! Times the direct decode-and-scale path without encoding.
//!
//! Usage: direct_bench TIMELINE.json [ROOT] [rgba]

use std::path::PathBuf;
use std::time::Instant;

use geneva_media::DirectSource;
use geneva_media::convert::PlaneFormat;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next().expect("timeline path"));
    let root = args.next().map_or_else(
        || path.parent().expect("a file path").to_path_buf(),
        PathBuf::from,
    );
    let rgb = args.next().is_some_and(|a| a == "rgba");
    let text = std::fs::read_to_string(&path).unwrap();
    let loaded = geneva_timeline::load(&text);
    for d in &loaded.diagnostics {
        eprintln!("{d:?}");
    }
    let comp = loaded.composition.expect("composition");
    let format = if rgb {
        PlaneFormat::Rgba8
    } else {
        PlaneFormat::Yuv420p8
    };
    let tags = if rgb {
        geneva_media::output_tags_for(geneva_timeline::schema::VideoCodec::Png, comp.color)
    } else {
        comp.color
    };
    let mut direct = DirectSource::open(&comp, &root, format, tags)
        .unwrap()
        .expect("direct path");
    let total = comp.frame_count();
    let started = Instant::now();
    let mut bytes = 0usize;
    for n in 0..total {
        let planes = direct.frame(comp.frame_time(n)).unwrap();
        bytes += planes.planes[0].data.len();
    }
    let secs = started.elapsed().as_secs_f64();
    println!(
        "{total} frames in {secs:.2}s ({:.0} fps), {} MB of luma",
        total as f64 / secs,
        bytes / 1_000_000
    );
}
