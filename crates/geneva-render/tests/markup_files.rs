//! Files markup points at: paths relative to the markup, as on a page.

use std::fs;
use std::path::PathBuf;

use geneva_render::{CpuRenderer, Frame, Renderer};
use geneva_timeline::{AssetInfo, Ratio, resolve_with};

/// An asset reader rooted at a directory, as the CLI's is.
struct Files(PathBuf);

impl AssetInfo for Files {
    fn duration(&self, _: &str, _: &str) -> Option<Ratio> {
        None
    }

    fn text(&self, _: &str, src: &str) -> Option<String> {
        fs::read_to_string(self.0.join(src)).ok()
    }

    fn read(&self, path: &str) -> Option<String> {
        fs::read_to_string(self.0.join(path)).ok()
    }

    fn exists(&self, path: &str) -> Option<bool> {
        Some(self.0.join(path).is_file())
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("geneva-markup-{name}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A four-by-four orange PNG.
fn write_logo(dir: &std::path::Path, name: &str) {
    let mut bytes = Vec::new();
    {
        let mut img = image::RgbaImage::new(4, 4);
        for p in img.pixels_mut() {
            *p = image::Rgba([255, 80, 0, 255]);
        }
        image::DynamicImage::ImageRgba8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
    }
    fs::write(dir.join(name), bytes).unwrap();
}

fn run(dir: &std::path::Path, markup: &str) -> (Vec<(&'static str, String)>, Option<Frame>) {
    fs::write(dir.join("card.html"), markup).unwrap();
    let text = r#"{"geneva":"1.0","output":{"width":60,"height":40,"fps":30,"duration":"1s",
        "background":"transparent"},"assets":{"c":{"src":"card.html"}},
        "layers":[{"clips":[{"source":{"kind":"html","asset":"c"},
        "transform":{"anchor":"top left","position":"0 0"}}]}]}"#;
    let timeline = geneva_timeline::parse(text).unwrap();
    let (comp, diags) = resolve_with(&timeline, &Files(dir.to_path_buf()));
    let codes = diags
        .iter()
        .filter(|d| d.is_error())
        .map(|d| (d.code, d.message.clone()))
        .collect();
    let frame = comp.map(|c| {
        CpuRenderer::with_asset_root(dir)
            .render_frame(&c, Ratio::ZERO)
            .unwrap()
    });
    (codes, frame)
}

#[test]
fn a_picture_beside_the_markup_is_drawn() {
    let dir = scratch("picture");
    write_logo(&dir, "logo.png");
    let (errors, frame) = run(
        &dir,
        "<style>img { width: 20px; height: 20px }</style><img src='logo.png'>",
    );
    assert!(errors.is_empty(), "{errors:?}");
    let px = frame.unwrap().get(10, 10);
    assert!(px.a > 0.9 && px.r > px.g && px.g > px.b, "{px:?}");
}

#[test]
fn a_linked_stylesheet_beside_the_markup_is_applied() {
    let dir = scratch("sheet");
    fs::write(
        dir.join("house.css"),
        ".p { background: #00ff00; height: 20px }",
    )
    .unwrap();
    let (errors, frame) = run(
        &dir,
        "<link rel='stylesheet' href='house.css'><div class='p'></div>",
    );
    assert!(errors.is_empty(), "{errors:?}");
    let px = frame.unwrap().get(30, 10);
    assert!(px.g > 0.9 && px.r < 0.1, "{px:?}");
}

#[test]
fn a_file_that_is_not_there_is_an_error_rather_than_a_hole() {
    let dir = scratch("missing");
    let (errors, _) = run(
        &dir,
        "<link rel='stylesheet' href='gone.css'><img src='gone.png'>",
    );
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors.iter().all(|(code, _)| *code == "E452"), "{errors:?}");
    assert!(errors.iter().any(|(_, m)| m.contains("gone.css")));
    assert!(errors.iter().any(|(_, m)| m.contains("gone.png")));
}

#[test]
fn a_path_out_of_the_root_is_refused() {
    let dir = scratch("escape");
    for bad in [
        "<img src='../secret.png'>",
        "<img src='/etc/passwd'>",
        "<link rel='stylesheet' href='https://example.com/x.css'>",
    ] {
        let (errors, _) = run(&dir, bad);
        assert!(
            errors
                .iter()
                .any(|(code, m)| *code == "E452" && m.contains("leaves the asset root")),
            "{bad}: {errors:?}"
        );
    }
}
