//! Scaling packed plane buffers between sizes: the renditions of a
//! multi-output render are made from one composited frame.

use ffmpeg_next::software::scaling;
use ffmpeg_next::util::frame;

use super::encode::pixel_of;
use super::{codec_error, ffi};
use crate::MediaError;
use crate::convert::{PlaneFormat, PlanePool, Planes};

/// A scaler from one plane size to another in the same layout.
pub struct PlaneScaler {
    scaler: ffi::ThreadedScaler,
    src: frame::Video,
    dst: frame::Video,
    format: PlaneFormat,
    width: u32,
    height: u32,
}

impl PlaneScaler {
    /// A scaler for `format` from `from` to `to` (width, height).
    pub fn new(format: PlaneFormat, from: (u32, u32), to: (u32, u32)) -> Result<Self, MediaError> {
        let pixel = pixel_of(format);
        let threads = std::thread::available_parallelism().map_or(1, usize::from);
        let scaler = ffi::ThreadedScaler::new(
            pixel,
            from,
            pixel,
            to,
            scaling::Flags::BICUBIC | scaling::Flags::ACCURATE_RND,
            threads,
        )
        .map_err(|e| codec_error("scaling", e))?;
        Ok(Self {
            scaler,
            src: frame::Video::new(pixel, from.0, from.1),
            dst: frame::Video::new(pixel, to.0, to.1),
            format,
            width: to.0,
            height: to.1,
        })
    }

    /// The size the scaler writes.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Scales `planes` (at the source size) into a buffer from `pool`.
    pub fn scale(&mut self, planes: &Planes, pool: &mut PlanePool) -> Result<Planes, MediaError> {
        for (i, plane) in planes.planes.iter().enumerate() {
            let stride = self.src.stride(i);
            let data = self.src.data_mut(i);
            for row in 0..plane.height {
                data[row * stride..row * stride + plane.stride]
                    .copy_from_slice(&plane.data[row * plane.stride..(row + 1) * plane.stride]);
            }
        }
        self.scaler
            .run(&self.src, &mut self.dst)
            .map_err(|e| codec_error("scaling", e))?;
        let mut out = pool.take(self.format, self.width, self.height);
        for (i, plane) in out.planes.iter_mut().enumerate() {
            let stride = self.dst.stride(i);
            let data = self.dst.data(i);
            for row in 0..plane.height {
                plane.data[row * plane.stride..(row + 1) * plane.stride]
                    .copy_from_slice(&data[row * stride..row * stride + plane.stride]);
            }
        }
        Ok(out)
    }
}
