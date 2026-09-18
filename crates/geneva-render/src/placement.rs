//! Where a clip lands in the frame: the affine map from its box to
//! output pixels, its inverse, and the sampling rules that go with it.
//! Shared by the CPU reference renderer and the GPU renderer, so that the
//! two agree on placement exactly and differ only in arithmetic.

use std::sync::Arc;

use geneva_timeline::schema::{Fit, ShapeKind};
use geneva_timeline::{ResolvedClip, ResolvedMask};

use crate::assets::Image;

/// Sub-pixel sample offsets: a 2x2 grid at quarter-pixel positions. A
/// pixel that is not aligned with the clip's box averages the paint at
/// these four points.
pub const SUBSAMPLES: [(f64, f64); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];

/// The part of a paint a clip shows, `[x, y, width, height]` in the
/// paint's own pixels: all of it, or its crop; `None` when the crop
/// leaves nothing.
pub fn crop_window(clip: &ResolvedClip, (w, h): (f64, f64)) -> Option<[f64; 4]> {
    match clip.crop {
        None => Some([0.0, 0.0, w, h]),
        Some(crop) => crop.to_px(w, h),
    }
}

/// The affine mapping from a clip's box to the output frame, and its
/// inverse. Every renderer places a clip through it, so the GPU renderer
/// draws the same pixels as the reference by construction rather than by
/// a second reading of the rules.
#[derive(Clone)]
pub struct Placement {
    /// The part of the paint shown, in paint coordinates: the clip's box.
    pub window: [f64; 4],
    /// Anchor in paint coordinates.
    pub anchor: [f64; 2],
    /// Anchor position in output coordinates.
    pub position: [f64; 2],
    /// Combined fit and user scale per axis.
    pub scale: [f64; 2],
    /// Cosine of the rotation.
    pub cos: f64,
    /// Sine of the rotation.
    pub sin: f64,
    /// Output-space bounding box `[x0, y0, x1, y1]`, clamped to the frame.
    pub bounds: [u32; 4],
    /// The same box before clamping, in output pixels.
    pub extent: [f64; 4],
    /// True when box pixels map one-to-one onto output pixels.
    pub pixel_aligned: bool,
    /// The clip's mask, evaluated in paint coordinates.
    pub(crate) mask: Option<MaskEval>,
}

/// A mask ready to evaluate at a point of the paint.
#[derive(Clone)]
pub(crate) struct MaskEval {
    /// The window (the clip's box) in paint coordinates.
    window: [f64; 4],
    spec: ResolvedMask,
    /// The shape's box in paint coordinates: left, top, width, height.
    rect: [f64; 4],
    luma: Option<Arc<Image>>,
}

impl MaskEval {
    /// Coverage in `[0, 1]` at a point of the paint.
    pub(crate) fn coverage(&self, u: f64, v: f64) -> f32 {
        let c = match &self.luma {
            Some(img) => {
                let [cx, cy, w, h] = self.window;
                // Stretched over the box, its edge texels extended past
                // the edges rather than fading into transparency.
                let (iw, ih) = (f64::from(img.width), f64::from(img.height));
                let p = img.sample(
                    ((u - cx) / w * iw).clamp(0.5, iw - 0.5),
                    ((v - cy) / h * ih).clamp(0.5, ih - 0.5),
                );
                // Premultiplied linear luma: white shows, black or
                // transparent hides.
                (0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b).clamp(0.0, 1.0)
            }
            None => {
                let [mx, my, mw, mh] = self.rect;
                // Signed distance to the shape, negative inside.
                let d = match self.spec.shape {
                    ShapeKind::Rect => {
                        let r = self.spec.radius.min(mw / 2.0).min(mh / 2.0);
                        let qx = (u - mx - mw / 2.0).abs() - (mw / 2.0 - r);
                        let qy = (v - my - mh / 2.0).abs() - (mh / 2.0 - r);
                        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                        outside + qx.max(qy).min(0.0) - r
                    }
                    ShapeKind::Ellipse => {
                        let nx = (u - mx - mw / 2.0) / (mw / 2.0);
                        let ny = (v - my - mh / 2.0) / (mh / 2.0);
                        ((nx * nx + ny * ny).sqrt() - 1.0) * (mw.min(mh) / 2.0)
                    }
                };
                if self.spec.feather > 0.0 {
                    (0.5 - d / self.spec.feather).clamp(0.0, 1.0) as f32
                } else if d <= 0.0 {
                    1.0
                } else {
                    0.0
                }
            }
        };
        if self.spec.invert { 1.0 - c } else { c }
    }
}

impl Placement {
    /// Places `clip` at its local time `local` in a `frame_w`x`frame_h`
    /// frame, showing `window` of a paint whose marks lie within
    /// `content` (when known), through its mask. `None` when nothing of
    /// it lands in the frame.
    pub fn new(
        frame_w: u32,
        frame_h: u32,
        clip: &ResolvedClip,
        local: f64,
        window: [f64; 4],
        content: Option<[f64; 4]>,
        mask_image: Option<Arc<Image>>,
    ) -> Option<Self> {
        let [cx, cy, w, h] = window;
        let mask = clip.mask.as_ref().map(|m| {
            let [mx, my, mw, mh] = m.rect_px(w, h);
            MaskEval {
                window,
                spec: m.clone(),
                rect: [cx + mx, cy + my, mw, mh],
                luma: mask_image,
            }
        });
        let out_w = f64::from(frame_w);
        let out_h = f64::from(frame_h);
        let fit = match clip.fit {
            Fit::None => [1.0, 1.0],
            Fit::Contain => {
                let s = (out_w / w).min(out_h / h);
                [s, s]
            }
            Fit::Cover => {
                let s = (out_w / w).max(out_h / h);
                [s, s]
            }
            Fit::Fill => [out_w / w, out_h / h],
        };
        let user = clip.scale.sample(local);
        let scale = [fit[0] * user[0], fit[1] * user[1]];
        if scale[0] == 0.0 || scale[1] == 0.0 || !scale[0].is_finite() || !scale[1].is_finite() {
            return None;
        }
        // The anchor is a point of the box, which is the window.
        let anchor = [cx + clip.anchor.x.to_px(w), cy + clip.anchor.y.to_px(h)];
        let position = clip.position.sample(local);
        let angle = clip.rotation.sample(local).to_radians();
        let (sin, cos) = angle.sin_cos();

        let forward = |q: [f64; 2]| {
            let x = (q[0] - anchor[0]) * scale[0];
            let y = (q[1] - anchor[1]) * scale[1];
            [
                position[0] + cos * x - sin * y,
                position[1] + sin * x + cos * y,
            ]
        };
        // The extent is what the paint can actually mark, which for a box
        // of markup with a lot of empty space is far less than its window.
        let [ex, ey, ew, eh] = match content {
            Some([x, y, cw, ch]) => {
                let x0 = x.max(cx);
                let y0 = y.max(cy);
                let x1 = (x + cw).min(cx + w);
                let y1 = (y + ch).min(cy + h);
                if x1 <= x0 || y1 <= y0 {
                    return None;
                }
                [x0, y0, x1 - x0, y1 - y0]
            }
            None => window,
        };
        let corners = [
            forward([ex, ey]),
            forward([ex + ew, ey]),
            forward([ex, ey + eh]),
            forward([ex + ew, ey + eh]),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        );
        for [x, y] in corners {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        Self::finish(
            window,
            anchor,
            position,
            scale,
            (cos, sin),
            [x0, y0, x1, y1],
            (frame_w, frame_h),
            mask,
        )
    }

    /// Clamps the extent to the frame and decides pixel alignment.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        window: [f64; 4],
        anchor: [f64; 2],
        position: [f64; 2],
        scale: [f64; 2],
        (cos, sin): (f64, f64),
        extent: [f64; 4],
        (frame_w, frame_h): (u32, u32),
        mask: Option<MaskEval>,
    ) -> Option<Self> {
        let out_w = f64::from(frame_w);
        let out_h = f64::from(frame_h);
        let [x0, y0, x1, y1] = extent;
        if x1 <= 0.0 || y1 <= 0.0 || x0 >= out_w || y0 >= out_h {
            return None;
        }
        let bounds = [
            x0.floor().max(0.0) as u32,
            y0.floor().max(0.0) as u32,
            (x1.ceil().min(out_w)) as u32,
            (y1.ceil().min(out_h)) as u32,
        ];
        let rotation_is_identity = (cos - 1.0).abs() < 1e-12 && sin.abs() < 1e-12;
        let unit_scale = (scale[0] - 1.0).abs() < 1e-12 && (scale[1] - 1.0).abs() < 1e-12;
        let integer_offset = |v: f64| (v - v.round()).abs() < 1e-9;
        let pixel_aligned = rotation_is_identity
            && unit_scale
            && integer_offset(position[0] - anchor[0])
            && integer_offset(position[1] - anchor[1])
            && window.iter().all(|v| integer_offset(*v));
        Some(Self {
            window,
            anchor,
            position,
            scale,
            cos,
            sin,
            bounds,
            extent,
            pixel_aligned,
            mask,
        })
    }

    /// The same placement in another frame: output coordinates are moved
    /// by `offset` and then multiplied by `factor`, and the bounds are
    /// clamped to the new frame's size.
    pub(crate) fn moved(
        &self,
        offset: [f64; 2],
        factor: f64,
        frame_w: u32,
        frame_h: u32,
    ) -> Option<Self> {
        let position = [
            (self.position[0] + offset[0]) * factor,
            (self.position[1] + offset[1]) * factor,
        ];
        let scale = [self.scale[0] * factor, self.scale[1] * factor];
        let extent = [
            (self.extent[0] + offset[0]) * factor,
            (self.extent[1] + offset[1]) * factor,
            (self.extent[2] + offset[0]) * factor,
            (self.extent[3] + offset[1]) * factor,
        ];
        let mask = self.mask.as_ref().map(|m| MaskEval {
            window: m.window,
            spec: m.spec.clone(),
            rect: m.rect,
            luma: m.luma.clone(),
        });
        Self::finish(
            self.window,
            self.anchor,
            position,
            scale,
            (self.cos, self.sin),
            extent,
            (frame_w, frame_h),
            mask,
        )
    }

    /// Whether the placement only moves and scales (no rotation, or a
    /// half turn), so that source coordinates advance by a constant step
    /// along an output row.
    pub fn axis_aligned(&self) -> bool {
        self.sin.abs() < 1e-12
    }

    /// For an axis-aligned image of `img_w`×`img_h` texels: the output
    /// pixels `[x0, y0, x1, y1)` whose samples, and the texels those
    /// samples blend, all lie inside the window, so they can be read
    /// without checks. Empty when the image is too small.
    pub fn interior(&self, img_w: u32, img_h: u32) -> [i64; 4] {
        if img_w < 2 || img_h < 2 {
            return [0; 4];
        }
        let [cx, cy, w, h] = self.window;
        let along = |q: f64, axis: usize| {
            self.position[axis] + self.cos * (q - self.anchor[axis]) * self.scale[axis]
        };
        let (xa, xb) = (along(cx, 0), along(cx + w, 0));
        let (ya, yb) = (along(cy, 1), along(cy + h, 1));
        // A sample must stay 1.5 texels inside the window: its bilinear
        // footprint is one texel, and the subsamples sit up to 0.75 px
        // from the pixel's corner.
        let margin = |scale: f64| (1.5 * scale.abs()).ceil() + 1.0;
        let (mx, my) = (margin(self.scale[0]), margin(self.scale[1]));
        [
            (xa.min(xb).ceil() + mx) as i64,
            (ya.min(yb).ceil() + my) as i64,
            (xa.max(xb).floor() - mx) as i64,
            (ya.max(yb).floor() - my) as i64,
        ]
    }

    /// Whether the clip has a mask, shape or luma.
    pub fn has_mask(&self) -> bool {
        self.mask.is_some()
    }

    /// Maps an output point back into paint coordinates.
    pub fn inverse(&self, x: f64, y: f64) -> (f64, f64) {
        let dx = x - self.position[0];
        let dy = y - self.position[1];
        let rx = self.cos * dx + self.sin * dy;
        let ry = -self.sin * dx + self.cos * dy;
        (
            self.anchor[0] + rx / self.scale[0],
            self.anchor[1] + ry / self.scale[1],
        )
    }
}
