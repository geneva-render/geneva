use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use geneva_color::{LinearRgba, Transfer};
use geneva_timeline::Composition;

use crate::RenderError;

/// A decoded still image in the compositing format.
#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Premultiplied linear pixels, row-major.
    pub pixels: Vec<LinearRgba>,
}

impl Image {
    /// Builds an image from 8-bit straight-alpha sRGB bytes.
    pub fn from_rgba8(width: u32, height: u32, data: &[u8]) -> Self {
        let lut = srgb_lut();
        let pixels = data
            .chunks_exact(4)
            .map(|p| {
                let a = f32::from(p[3]) / 255.0;
                LinearRgba {
                    r: lut[p[0] as usize] * a,
                    g: lut[p[1] as usize] * a,
                    b: lut[p[2] as usize] * a,
                    a,
                }
            })
            .collect();
        Self {
            width,
            height,
            pixels,
        }
    }

    /// The pixel at integer coordinates, or transparent outside the image.
    pub fn texel(&self, x: i64, y: i64) -> LinearRgba {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return LinearRgba::TRANSPARENT;
        }
        self.pixels[y as usize * self.width as usize + x as usize]
    }

    /// Bilinear sample at continuous coordinates where texel centers sit at
    /// half-integers, so `(0.5, 0.5)` returns the first texel exactly.
    pub fn sample(&self, x: f64, y: f64) -> LinearRgba {
        let fx = x - 0.5;
        let fy = y - 0.5;
        let x0 = fx.floor();
        let y0 = fy.floor();
        let tx = (fx - x0) as f32;
        let ty = (fy - y0) as f32;
        let (x0, y0) = (x0 as i64, y0 as i64);
        let lerp = |a: LinearRgba, b: LinearRgba, t: f32| LinearRgba {
            r: a.r + (b.r - a.r) * t,
            g: a.g + (b.g - a.g) * t,
            b: a.b + (b.b - a.b) * t,
            a: a.a + (b.a - a.a) * t,
        };
        let top = lerp(self.texel(x0, y0), self.texel(x0 + 1, y0), tx);
        let bottom = lerp(self.texel(x0, y0 + 1), self.texel(x0 + 1, y0 + 1), tx);
        lerp(top, bottom, ty)
    }
}

fn srgb_lut() -> &'static [f32; 256] {
    static LUT: OnceLock<[f32; 256]> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut lut = [0.0f32; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = Transfer::Srgb.to_linear(i as f64 / 255.0) as f32;
        }
        lut
    })
}

/// Supplies decoded assets to a renderer.
pub trait AssetSource {
    /// Loads the image asset with the given id.
    fn image(&mut self, comp: &Composition, id: &str) -> Result<&Image, RenderError>;
}

/// Loads assets from files under a root directory, caching decoded images.
#[derive(Debug)]
pub struct FileAssets {
    root: PathBuf,
    images: HashMap<String, Image>,
}

impl FileAssets {
    /// Creates a loader resolving asset paths under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            images: HashMap::new(),
        }
    }

    /// The asset root.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl AssetSource for FileAssets {
    fn image(&mut self, comp: &Composition, id: &str) -> Result<&Image, RenderError> {
        if !self.images.contains_key(id) {
            let asset = comp.assets.get(id).ok_or_else(|| RenderError::Asset {
                id: id.to_owned(),
                reason: "not declared in the composition".to_owned(),
            })?;
            let path = self.root.join(&asset.src);
            let decoded = image::open(&path).map_err(|e| RenderError::Asset {
                id: id.to_owned(),
                reason: format!("{} ({e})", path.display()),
            })?;
            let rgba = decoded.to_rgba8();
            let img = Image::from_rgba8(rgba.width(), rgba.height(), rgba.as_raw());
            self.images.insert(id.to_owned(), img);
        }
        Ok(&self.images[id])
    }
}

/// An asset source with no assets, for compositions that use none.
#[derive(Debug, Default)]
pub struct NoAssets;

impl AssetSource for NoAssets {
    fn image(&mut self, _: &Composition, id: &str) -> Result<&Image, RenderError> {
        Err(RenderError::Asset {
            id: id.to_owned(),
            reason: "no asset source configured".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_sampling_hits_texel_centers_exactly() {
        let img = Image::from_rgba8(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert_eq!(img.sample(0.5, 0.5), img.texel(0, 0));
        assert_eq!(img.sample(1.5, 0.5), img.texel(1, 0));
        let mid = img.sample(1.0, 0.5);
        assert!((mid.r - 0.5).abs() < 1e-6 && (mid.b - 0.5).abs() < 1e-6);
    }

    #[test]
    fn outside_is_transparent() {
        let img = Image::from_rgba8(1, 1, &[255, 255, 255, 255]);
        assert_eq!(img.texel(-1, 0), LinearRgba::TRANSPARENT);
        assert_eq!(img.texel(0, 1), LinearRgba::TRANSPARENT);
        assert!(img.sample(0.0, 0.5).a < 0.51);
    }
}
