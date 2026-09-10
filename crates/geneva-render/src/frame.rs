use geneva_color::{Color, LinearRgba};

/// A rendered frame: premultiplied linear-light RGBA, row-major, top-left
/// origin.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    width: u32,
    height: u32,
    pixels: Vec<LinearRgba>,
}

impl Frame {
    /// Creates a frame cleared to `color`.
    pub fn new(width: u32, height: u32, color: Color) -> Self {
        let n = width as usize * height as usize;
        Self {
            width,
            height,
            pixels: vec![color.to_linear(); n],
        }
    }

    /// Width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The pixel at `(x, y)`.
    pub fn get(&self, x: u32, y: u32) -> LinearRgba {
        self.pixels[y as usize * self.width as usize + x as usize]
    }

    /// Overwrites the pixel at `(x, y)`.
    pub fn set(&mut self, x: u32, y: u32, value: LinearRgba) {
        self.pixels[y as usize * self.width as usize + x as usize] = value;
    }

    /// All pixels, row-major.
    pub fn pixels(&self) -> &[LinearRgba] {
        &self.pixels
    }

    /// Mutable access to all pixels, row-major.
    pub fn pixels_mut(&mut self) -> &mut [LinearRgba] {
        &mut self.pixels
    }

    /// Converts to 8-bit straight-alpha sRGB, row-major RGBA bytes.
    pub fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.pixels.len() * 4);
        for p in &self.pixels {
            out.extend_from_slice(&p.to_srgb8());
        }
        out
    }

    /// Encodes the frame as a PNG.
    pub fn to_png(&self) -> Result<Vec<u8>, image::ImageError> {
        let img = image::RgbaImage::from_raw(self.width, self.height, self.to_rgba8())
            .expect("buffer size matches dimensions");
        let mut bytes = std::io::Cursor::new(Vec::new());
        img.write_to(&mut bytes, image::ImageFormat::Png)?;
        Ok(bytes.into_inner())
    }
}
