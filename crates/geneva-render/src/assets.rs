use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use geneva_color::hdr::HdrToSdr;
use geneva_color::{ColorTags, LinearRgba, Primaries, ResolvedTags, Transfer, primaries};
use geneva_timeline::{Composition, Ratio};

use crate::RenderError;

/// A decoded still image in the compositing format.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Premultiplied linear pixels, row-major.
    pub pixels: Vec<LinearRgba>,
    /// The sub-rectangle `[x, y, w, h]` outside which every pixel is
    /// transparent, when whatever drew the image knows one. Compositing
    /// skips the rest, so a picture with a lot of empty space around it
    /// costs what it draws rather than what it spans.
    pub content: Option<[u32; 4]>,
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
            content: None,
        }
    }

    /// Builds an image from 16-bit straight-alpha samples under `tags`:
    /// their transfer curve, their primaries brought into the working
    /// space, and HDR material tone-mapped.
    pub fn from_rgba16(width: u32, height: u32, data: &[u16], tags: ResolvedTags) -> Self {
        let hdr = HdrToSdr::new(tags, None);
        let to_working = primaries::conversion(tags.primaries, Primaries::Bt709)
            .map(|m| m.map(|row| row.map(|v| v as f32)));
        let lut: Vec<f32> = (0..=u16::MAX)
            .map(|c| tags.transfer.to_linear(f64::from(c) / f64::from(u16::MAX)) as f32)
            .collect();
        let pixels = data
            .chunks_exact(4)
            .map(|p| {
                let a = f32::from(p[3]) / f32::from(u16::MAX);
                let code = |c: u16| f32::from(c) / f32::from(u16::MAX);
                let [r, g, b] = match (&hdr, &to_working) {
                    (Some(h), _) => h.convert([code(p[0]), code(p[1]), code(p[2])]),
                    (None, Some(m)) => {
                        let [r, g, b] =
                            [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]];
                        [
                            m[0][0] * r + m[0][1] * g + m[0][2] * b,
                            m[1][0] * r + m[1][1] * g + m[1][2] * b,
                            m[2][0] * r + m[2][1] * g + m[2][2] * b,
                        ]
                    }
                    (None, None) => [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize]],
                };
                LinearRgba {
                    r: r * a,
                    g: g * a,
                    b: b * a,
                    a,
                }
            })
            .collect();
        Self {
            width,
            height,
            pixels,
            content: None,
        }
    }

    /// Wraps a rendered frame as an image, without conversion.
    pub fn from_frame(frame: &crate::Frame) -> Self {
        Self {
            width: frame.width(),
            height: frame.height(),
            pixels: frame.pixels().to_vec(),
            content: None,
        }
    }

    /// Takes a rendered frame's pixels as an image without copying.
    pub fn from_frame_pixels(frame: crate::Frame) -> Self {
        let (width, height) = (frame.width(), frame.height());
        Self {
            width,
            height,
            pixels: frame.into_pixels(),
            content: None,
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
        let (x0, tx) = split(fx);
        let (y0, ty) = split(fy);
        // The 2x2 neighbourhood is inside for every sample but those on
        // the outermost half-pixel, so the common case indexes straight
        // in rather than testing four bounds twice over.
        let (w, h) = (i64::from(self.width), i64::from(self.height));
        if x0 >= 0 && y0 >= 0 && x0 + 1 < w && y0 + 1 < h {
            let row = y0 as usize * self.width as usize + x0 as usize;
            let next = row + self.width as usize;
            let top = lerp(self.pixels[row], self.pixels[row + 1], tx);
            let bottom = lerp(self.pixels[next], self.pixels[next + 1], tx);
            return lerp(top, bottom, ty);
        }
        let top = lerp(self.texel(x0, y0), self.texel(x0 + 1, y0), tx);
        let bottom = lerp(self.texel(x0, y0 + 1), self.texel(x0 + 1, y0 + 1), tx);
        lerp(top, bottom, ty)
    }
}

#[inline]
pub(crate) fn lerp(a: LinearRgba, b: LinearRgba, t: f32) -> LinearRgba {
    LinearRgba {
        r: a.r + (b.r - a.r) * t,
        g: a.g + (b.g - a.g) * t,
        b: a.b + (b.b - a.b) * t,
        a: a.a + (b.a - a.a) * t,
    }
}

/// Splits a coordinate into its floor and fractional part. Truncation
/// with a correction for negatives avoids a library call per sample.
#[inline]
pub(crate) fn split(v: f64) -> (i64, f32) {
    let t = v as i64;
    let floor = if (t as f64) > v { t - 1 } else { t };
    (floor, (v - floor as f64) as f32)
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

/// A decoded video frame as the decoder holds it, for a renderer that
/// converts to linear light on its own device: 8-bit 4:2:0 Y'CbCr, the
/// chroma planes `ceil(width / 2)` by `ceil(height / 2)`, every plane
/// row-major with its own stride in bytes. This is the one layout the CPU
/// path converts without a widening pass ([`Transfer`], matrix and
/// range from `tags`), and only for SDR material shown unrotated; any
/// other frame comes through [`AssetSource::video_frame`] as a picture.
#[derive(Debug, Clone, Copy)]
pub struct VideoPlanes<'a> {
    /// Width in luma samples.
    pub width: u32,
    /// Height in luma rows.
    pub height: u32,
    /// Luma.
    pub y: &'a [u8],
    /// Cb.
    pub cb: &'a [u8],
    /// Cr.
    pub cr: &'a [u8],
    /// Bytes per luma row.
    pub y_stride: usize,
    /// Bytes per chroma row.
    pub c_stride: usize,
    /// The frame's color tags, resolved: transfer, matrix and range are
    /// what the conversion applies.
    pub tags: ResolvedTags,
}

/// Supplies decoded assets to a renderer.
pub trait AssetSource {
    /// Loads the image asset with the given id.
    fn image(&mut self, comp: &Composition, id: &str) -> Result<&Image, RenderError>;

    /// Loads a picture by its path under the asset root, for markup that
    /// points at one the way a page does.
    fn image_at(&mut self, path: &str) -> Result<&Image, RenderError> {
        Err(RenderError::Asset {
            id: path.to_owned(),
            reason: "this asset source cannot load pictures by path".to_owned(),
        })
    }

    /// Returns the bytes of a font asset.
    fn font(&mut self, comp: &Composition, id: &str) -> Result<Arc<Vec<u8>>, RenderError> {
        let _ = comp;
        Err(RenderError::Asset {
            id: id.to_owned(),
            reason: "this asset source cannot load fonts".to_owned(),
        })
    }

    /// Returns the bytes of a font file by its path under the asset root,
    /// for markup's `@font-face`.
    fn font_at(&mut self, path: &str) -> Result<Arc<Vec<u8>>, RenderError> {
        Err(RenderError::Asset {
            id: path.to_owned(),
            reason: "this asset source cannot load fonts by path".to_owned(),
        })
    }

    /// The planes of the frame of a video asset shown at `source_time`,
    /// when the decoder holds it in the layout [`VideoPlanes`] describes;
    /// `None` when it does not, and the frame is to be taken as a picture
    /// from [`video_frame`](Self::video_frame). The default has none.
    fn video_planes(
        &mut self,
        comp: &Composition,
        id: &str,
        source_time: Ratio,
    ) -> Result<Option<VideoPlanes<'_>>, RenderError> {
        let _ = (comp, id, source_time);
        Ok(None)
    }

    /// The size of a video asset's frames as displayed, when the source
    /// can tell it without decoding one; `None` otherwise, which is what
    /// the default says.
    fn video_size(
        &mut self,
        comp: &Composition,
        id: &str,
    ) -> Result<Option<(u32, u32)>, RenderError> {
        let _ = (comp, id);
        Ok(None)
    }

    /// The size a video asset is shown at, unrounded: a picture with
    /// non-square pixels is stretched to a width that need not be whole
    /// (853.33 for 720×480 at 32:27), while its frames have the rounded
    /// [`video_size`](Self::video_size). A clip is placed by this one, so
    /// it keeps its exact aspect. The default is `video_size`.
    fn video_display_size(
        &mut self,
        comp: &Composition,
        id: &str,
    ) -> Result<Option<(f64, f64)>, RenderError> {
        Ok(self
            .video_size(comp, id)?
            .map(|(w, h)| (f64::from(w), f64::from(h))))
    }

    /// The frame of a video asset shown at `source_time`, made smaller to
    /// `size` (as displayed) by whatever is cheapest before it is
    /// converted to linear light. For a picture drawn smaller than it
    /// is; the renderer places the frame by the size it gets, so a
    /// source may return it whole, as the default does.
    fn video_frame_shrunk(
        &mut self,
        comp: &Composition,
        id: &str,
        source_time: Ratio,
        size: [u32; 2],
    ) -> Result<&Image, RenderError> {
        let _ = size;
        self.video_frame(comp, id, source_time)
    }

    /// Returns the frame of a video asset shown at `source_time`.
    ///
    /// The default has no decoder and reports the asset as unavailable.
    fn video_frame(
        &mut self,
        comp: &Composition,
        id: &str,
        source_time: Ratio,
    ) -> Result<&Image, RenderError> {
        let _ = (comp, source_time);
        Err(RenderError::Asset {
            id: id.to_owned(),
            reason: "this asset source cannot decode video".to_owned(),
        })
    }
}

/// Loads assets from files under a root directory, caching decoded images.
#[derive(Debug)]
pub struct FileAssets {
    root: PathBuf,
    images: HashMap<String, Image>,
    fonts: HashMap<String, Arc<Vec<u8>>>,
}

impl FileAssets {
    /// Creates a loader resolving asset paths under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            images: HashMap::new(),
            fonts: HashMap::new(),
        }
    }

    /// The asset root.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Reads and converts a picture. Untagged images are sRGB; a tagged one,
/// or one with more than eight bits, goes through its tags at full depth.
fn decode(path: &std::path::Path, id: &str, color: ColorTags) -> Result<Image, RenderError> {
    let decoded = image::open(path).map_err(|e| RenderError::Asset {
        id: id.to_owned(),
        reason: format!("{} ({e})", path.display()),
    })?;
    let tags = image_tags(color);
    let deep = decoded.color().bits_per_pixel() > 32;
    Ok(if tags == ResolvedTags::SRGB && !deep {
        let rgba = decoded.to_rgba8();
        Image::from_rgba8(rgba.width(), rgba.height(), rgba.as_raw())
    } else {
        let rgba = decoded.to_rgba16();
        Image::from_rgba16(rgba.width(), rgba.height(), rgba.as_raw(), tags)
    })
}

impl AssetSource for FileAssets {
    fn font(&mut self, comp: &Composition, id: &str) -> Result<Arc<Vec<u8>>, RenderError> {
        if let Some(data) = self.fonts.get(id) {
            return Ok(Arc::clone(data));
        }
        let asset = comp.assets.get(id).ok_or_else(|| RenderError::Asset {
            id: id.to_owned(),
            reason: "not declared in the composition".to_owned(),
        })?;
        let path = self.root.join(&asset.src);
        let data = std::fs::read(&path).map_err(|e| RenderError::Asset {
            id: id.to_owned(),
            reason: format!("{} ({e})", path.display()),
        })?;
        let data = Arc::new(data);
        self.fonts.insert(id.to_owned(), Arc::clone(&data));
        Ok(data)
    }

    fn font_at(&mut self, path: &str) -> Result<Arc<Vec<u8>>, RenderError> {
        let key = format!("path:{path}");
        if let Some(data) = self.fonts.get(&key) {
            return Ok(Arc::clone(data));
        }
        let full = self.root.join(path);
        let data = Arc::new(std::fs::read(&full).map_err(|e| RenderError::Asset {
            id: path.to_owned(),
            reason: format!("{} ({e})", full.display()),
        })?);
        self.fonts.insert(key, Arc::clone(&data));
        Ok(data)
    }

    fn image_at(&mut self, path: &str) -> Result<&Image, RenderError> {
        // Keyed by path, which cannot collide with an asset id: an id
        // never contains a slash and a picture in markup is always under
        // the markup's own directory.
        let key = format!("path:{path}");
        if !self.images.contains_key(&key) {
            let img = decode(&self.root.join(path), path, ColorTags::default())?;
            self.images.insert(key.clone(), img);
        }
        Ok(&self.images[&key])
    }

    fn image(&mut self, comp: &Composition, id: &str) -> Result<&Image, RenderError> {
        if !self.images.contains_key(id) {
            let asset = comp.assets.get(id).ok_or_else(|| RenderError::Asset {
                id: id.to_owned(),
                reason: "not declared in the composition".to_owned(),
            })?;
            let img = decode(&self.root.join(&asset.src), id, asset.color)?;
            self.images.insert(id.to_owned(), img);
        }
        Ok(&self.images[id])
    }
}

/// The color tags of a still image: sRGB with BT.709 primaries unless
/// the asset says otherwise. Images are RGB, so the matrix and range
/// tags do not apply.
fn image_tags(color: ColorTags) -> ResolvedTags {
    ResolvedTags {
        primaries: color.primaries.unwrap_or(Primaries::Bt709),
        transfer: color.transfer.unwrap_or(Transfer::Srgb),
        ..ResolvedTags::SRGB
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
