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

    /// Resizes the frame if needed and clears it to `color`, keeping the
    /// pixel buffer when the size is unchanged.
    pub fn reset(&mut self, width: u32, height: u32, color: Color) {
        use rayon::prelude::*;
        self.width = width;
        self.height = height;
        let n = width as usize * height as usize;
        let c = color.to_linear();
        if self.pixels.len() == n && n >= 1 << 16 {
            // A frame-sized clear is memory-bound: rows go in parallel.
            let chunk = n.div_ceil(rayon::current_num_threads().max(1));
            self.pixels
                .par_chunks_mut(chunk.max(1))
                .for_each(|part| part.fill(c));
        } else {
            self.pixels.clear();
            self.pixels.resize(n, c);
        }
    }

    /// An empty frame that owns `pixels` as its buffer, so that a buffer
    /// can be used again by [`reset`](Self::reset) without a new
    /// allocation.
    pub fn from_pixels(pixels: Vec<LinearRgba>) -> Self {
        Self {
            width: 0,
            height: 0,
            pixels,
        }
    }

    /// The pixel buffer, giving up the frame.
    pub fn into_pixels(self) -> Vec<LinearRgba> {
        self.pixels
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
