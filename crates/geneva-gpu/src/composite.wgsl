// The composite of one clip onto the frame: a transcription of the CPU
// renderer's sampling and blending rules (crates/geneva-render/src/cpu.rs
// and placement.rs), so that the two renderers draw the same pixels and
// differ only in arithmetic.
//
// One draw covers the placement's bounds with two triangles. The fragment
// shader maps each output pixel back into the clip's paint through the
// inverse affine, samples the paint there (a solid, a shape's signed
// distance, or an image read bilinearly with texel centers at half
// integers), scales it by the mask's coverage and the opacity, and either
// hands the premultiplied color to fixed-function blending ("over", "add",
// or a fade's veil) or, for a separable blend mode, reads the backdrop
// and composites in the shader.

struct Clip {
    // The output pixels drawn: x0, y0, x1, y1.
    bounds: vec4<f32>,
    // The part of the paint shown, in paint pixels: x, y, width, height.
    window: vec4<f32>,
    // Where one center sample stands in for the four subsamples when
    // `mode` is 2: x0, y0, x1, y1 in output pixels.
    interior: vec4<f32>,
    // The fill, premultiplied linear light.
    fill: vec4<f32>,
    // The stroke's color, premultiplied linear light.
    stroke: vec4<f32>,
    // The mask shape's box in paint pixels: x, y, width, height.
    mask_rect: vec4<f32>,
    // The anchor's place in the output.
    position: vec2<f32>,
    // The anchor, in paint pixels.
    anchor: vec2<f32>,
    // Fit and user scale per axis.
    scale: vec2<f32>,
    // Cosine and sine of the rotation.
    rotation: vec2<f32>,
    // The paint's width and height.
    size: vec2<f32>,
    // The frame's width and height.
    frame: vec2<f32>,
    stroke_width: f32,
    radius: f32,
    opacity: f32,
    mask_radius: f32,
    mask_feather: f32,
    // 0 solid, 1 rectangle, 2 ellipse, 3 image.
    kind: u32,
    // 0 four subsamples, 1 one center sample, 2 one center sample inside
    // `interior` and four subsamples elsewhere.
    mode: u32,
    has_stroke: u32,
    // 0 normal and 8 add (fixed-function), 1 multiply, 2 screen,
    // 3 overlay, 4 darken, 5 lighten, 6 difference, 7 soft light.
    blend: u32,
    // 0 none, 1 rectangle, 2 ellipse, 3 luma image.
    mask_kind: u32,
    mask_invert: u32,
};

@group(0) @binding(0) var<uniform> clip: Clip;
@group(0) @binding(1) var image: texture_2d<f32>;
@group(0) @binding(2) var mask: texture_2d<f32>;
@group(0) @binding(3) var backdrop: texture_2d<f32>;

struct Vertex {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> Vertex {
    // Two triangles over the bounds. Frame y grows down, clip-space y up.
    var corner = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corner[i];
    let px = mix(clip.bounds.x, clip.bounds.z, c.x);
    let py = mix(clip.bounds.y, clip.bounds.w, c.y);
    var out: Vertex;
    out.position = vec4<f32>(
        px / clip.frame.x * 2.0 - 1.0,
        1.0 - py / clip.frame.y * 2.0,
        0.0,
        1.0,
    );
    return out;
}

// Maps an output point back into paint coordinates (Placement::inverse).
fn inverse(x: f32, y: f32) -> vec2<f32> {
    let dx = x - clip.position.x;
    let dy = y - clip.position.y;
    let c = clip.rotation.x;
    let s = clip.rotation.y;
    let rx = c * dx + s * dy;
    let ry = -s * dx + c * dy;
    return vec2<f32>(clip.anchor.x + rx / clip.scale.x, clip.anchor.y + ry / clip.scale.y);
}

// a + (b - a) * t, in the CPU renderer's order of operations.
fn lerp(a: vec4<f32>, b: vec4<f32>, t: f32) -> vec4<f32> {
    return a + (b - a) * t;
}

// The texel at integer coordinates, transparent outside the image
// (Image::texel).
fn texel(x: i32, y: i32) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(image));
    if x < 0 || y < 0 || x >= d.x || y >= d.y {
        return vec4<f32>(0.0);
    }
    return textureLoad(image, vec2<i32>(x, y), 0);
}

// Bilinear sample with texel centers at half integers (Image::sample).
fn sample_image(x: f32, y: f32) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let top = lerp(texel(xi, yi), texel(xi + 1, yi), tx);
    let bottom = lerp(texel(xi, yi + 1), texel(xi + 1, yi + 1), tx);
    return lerp(top, bottom, ty);
}

// The same two, on the luma mask image.
fn mask_texel(x: i32, y: i32) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(mask));
    if x < 0 || y < 0 || x >= d.x || y >= d.y {
        return vec4<f32>(0.0);
    }
    return textureLoad(mask, vec2<i32>(x, y), 0);
}

fn sample_mask(x: f32, y: f32) -> vec4<f32> {
    let fx = x - 0.5;
    let fy = y - 0.5;
    let x0 = floor(fx);
    let y0 = floor(fy);
    let tx = fx - x0;
    let ty = fy - y0;
    let xi = i32(x0);
    let yi = i32(y0);
    let top = lerp(mask_texel(xi, yi), mask_texel(xi + 1, yi), tx);
    let bottom = lerp(mask_texel(xi, yi + 1), mask_texel(xi + 1, yi + 1), tx);
    return lerp(top, bottom, ty);
}

// Signed distance to a rounded rectangle, negative inside.
fn rounded_rect(u: f32, v: f32, w: f32, h: f32, radius: f32) -> f32 {
    let r = min(radius, min(w / 2.0, h / 2.0));
    let qx = abs(u - w / 2.0) - (w / 2.0 - r);
    let qy = abs(v - h / 2.0) - (h / 2.0 - r);
    let px = max(qx, 0.0);
    let py = max(qy, 0.0);
    let outside = sqrt(px * px + py * py);
    return outside + min(max(qx, qy), 0.0) - r;
}

// The paint's color at a point of its box, transparent outside the
// geometry (Paint::sample).
fn paint(u: f32, v: f32) -> vec4<f32> {
    let w = clip.size.x;
    let h = clip.size.y;
    if u < 0.0 || v < 0.0 || u >= w || v >= h {
        return vec4<f32>(0.0);
    }
    switch clip.kind {
        case 0u: {
            return clip.fill;
        }
        case 1u: {
            let d = rounded_rect(u, v, w, h, clip.radius);
            if d > 0.0 {
                return vec4<f32>(0.0);
            }
            if clip.has_stroke != 0u && d > -clip.stroke_width {
                return clip.stroke;
            }
            return clip.fill;
        }
        case 2u: {
            let nx = (u - w / 2.0) / (w / 2.0);
            let ny = (v - h / 2.0) / (h / 2.0);
            let f = sqrt(nx * nx + ny * ny);
            if f > 1.0 {
                return vec4<f32>(0.0);
            }
            if clip.has_stroke != 0u && (1.0 - f) * (min(w, h) / 2.0) < clip.stroke_width {
                return clip.stroke;
            }
            return clip.fill;
        }
        default: {
            return sample_image(u, v);
        }
    }
}

// The mask's coverage in [0, 1] at a point of the paint (MaskEval::coverage).
fn coverage(u: f32, v: f32) -> f32 {
    var c: f32;
    if clip.mask_kind == 3u {
        // Stretched over the window, its edge texels extended past the
        // edges rather than fading into transparency.
        let d = vec2<f32>(textureDimensions(mask));
        let cx = clip.window.x;
        let cy = clip.window.y;
        let w = clip.window.z;
        let h = clip.window.w;
        let p = sample_mask(
            clamp((u - cx) / w * d.x, 0.5, d.x - 0.5),
            clamp((v - cy) / h * d.y, 0.5, d.y - 0.5),
        );
        // Premultiplied linear luma: white shows, black or transparent
        // hides.
        c = clamp(0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b, 0.0, 1.0);
    } else {
        let mx = clip.mask_rect.x;
        let my = clip.mask_rect.y;
        let mw = clip.mask_rect.z;
        let mh = clip.mask_rect.w;
        var d: f32;
        if clip.mask_kind == 1u {
            d = rounded_rect(u - mx, v - my, mw, mh, clip.mask_radius);
        } else {
            let nx = (u - mx - mw / 2.0) / (mw / 2.0);
            let ny = (v - my - mh / 2.0) / (mh / 2.0);
            d = (sqrt(nx * nx + ny * ny) - 1.0) * (min(mw, mh) / 2.0);
        }
        if clip.mask_feather > 0.0 {
            c = clamp(0.5 - d / clip.mask_feather, 0.0, 1.0);
        } else if d <= 0.0 {
            c = 1.0;
        } else {
            c = 0.0;
        }
    }
    if clip.mask_invert != 0u {
        return 1.0 - c;
    }
    return c;
}

// The paint at an output point, transparent outside the window, through
// the mask (Placement::sample).
fn sample_at(x: f32, y: f32) -> vec4<f32> {
    let p = inverse(x, y);
    let cx = clip.window.x;
    let cy = clip.window.y;
    let w = clip.window.z;
    let h = clip.window.w;
    if p.x < cx || p.y < cy || p.x >= cx + w || p.y >= cy + h {
        return vec4<f32>(0.0);
    }
    let color = paint(p.x, p.y);
    if clip.mask_kind != 0u {
        return color * coverage(p.x, p.y);
    }
    return color;
}

// Hard light: multiply for dark source values, screen for light ones.
fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        return cb * 2.0 * cs;
    }
    return cb + (2.0 * cs - 1.0) - cb * (2.0 * cs - 1.0);
}

// Soft light as defined by the W3C compositing specification.
fn soft_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 {
        return cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb);
    }
    var d: f32;
    if cb <= 0.25 {
        d = ((16.0 * cb - 12.0) * cb + 4.0) * cb;
    } else {
        d = sqrt(cb);
    }
    return cb + (2.0 * cs - 1.0) * (d - cb);
}

// The mode's blend function on straight colors (composite's table).
fn blend_channel(cb: f32, cs: f32) -> f32 {
    switch clip.blend {
        case 1u: {
            return cb * cs;
        }
        case 2u: {
            return cb + cs - cb * cs;
        }
        case 3u: {
            return hard_light(cs, cb);
        }
        case 4u: {
            return min(cb, cs);
        }
        case 5u: {
            return max(cb, cs);
        }
        case 6u: {
            return abs(cb - cs);
        }
        default: {
            return soft_light(cb, cs);
        }
    }
}

fn straight(c: f32, a: f32) -> f32 {
    if a > 0.0 {
        return c / a;
    }
    return 0.0;
}

// The W3C formula on premultiplied colors: each channel is
// Cs*as*(1-ab) + Cb*ab*(1-as) + as*ab*B(Cb, Cs); the alpha is "over".
fn separable(s: vec4<f32>, d: vec4<f32>) -> vec4<f32> {
    let sa = s.a;
    let ba = d.a;
    var out: vec4<f32>;
    out.r = s.r * (1.0 - ba) + d.r * (1.0 - sa) + sa * ba * blend_channel(straight(d.r, ba), straight(s.r, sa));
    out.g = s.g * (1.0 - ba) + d.g * (1.0 - sa) + sa * ba * blend_channel(straight(d.g, ba), straight(s.g, sa));
    out.b = s.b * (1.0 - ba) + d.b * (1.0 - sa) + sa * ba * blend_channel(straight(d.b, ba), straight(s.b, sa));
    out.a = sa + ba * (1.0 - sa);
    return out;
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // The fragment's position is the pixel's center.
    let px = floor(pos.x);
    let py = floor(pos.y);
    var center = clip.mode == 1u;
    if clip.mode == 2u {
        center = px >= clip.interior.x && px < clip.interior.z
            && py >= clip.interior.y && py < clip.interior.w;
    }
    var color: vec4<f32>;
    if center {
        color = sample_at(px + 0.5, py + 0.5);
    } else {
        // The 2x2 subsamples at quarter-pixel positions, averaged
        // (SUBSAMPLES and sample_pixel).
        let acc = sample_at(px + 0.25, py + 0.25)
            + sample_at(px + 0.75, py + 0.25)
            + sample_at(px + 0.25, py + 0.75)
            + sample_at(px + 0.75, py + 0.75);
        color = acc * 0.25;
    }
    let src = color * clip.opacity;
    if clip.blend >= 1u && clip.blend <= 7u {
        // The pipeline replaces the pixel with what is computed here,
        // from a copy of the frame taken before this draw.
        let dst = textureLoad(backdrop, vec2<i32>(i32(px), i32(py)), 0);
        return separable(src, dst);
    }
    return src;
}
