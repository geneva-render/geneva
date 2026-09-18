// The composite of one clip onto the frame: a transcription of the CPU
// renderer's sampling rules (crates/geneva-render/src/cpu.rs and
// placement.rs), so that the two renderers draw the same pixels and
// differ only in arithmetic.
//
// One draw covers the placement's bounds with two triangles. The fragment
// shader maps each output pixel back into the clip's paint through the
// inverse affine, samples the paint there (a solid, a shape's signed
// distance, or an image read bilinearly with texel centers at half
// integers), and hands the premultiplied color to fixed-function
// blending, which does "over" or "add".

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
    // 0 solid, 1 rectangle, 2 ellipse, 3 image.
    kind: u32,
    // 0 four subsamples, 1 one center sample, 2 one center sample inside
    // `interior` and four subsamples elsewhere.
    mode: u32,
    has_stroke: u32,
};

@group(0) @binding(0) var<uniform> clip: Clip;
@group(0) @binding(1) var image: texture_2d<f32>;

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

// The texel at integer coordinates, transparent outside the image
// (Image::texel).
fn texel(x: i32, y: i32) -> vec4<f32> {
    let d = vec2<i32>(textureDimensions(image));
    if x < 0 || y < 0 || x >= d.x || y >= d.y {
        return vec4<f32>(0.0);
    }
    return textureLoad(image, vec2<i32>(x, y), 0);
}

// a + (b - a) * t, in the CPU renderer's order of operations.
fn lerp(a: vec4<f32>, b: vec4<f32>, t: f32) -> vec4<f32> {
    return a + (b - a) * t;
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
            // Signed distance to a rounded rectangle centred in the box.
            let r = min(clip.radius, min(w / 2.0, h / 2.0));
            let qx = abs(u - w / 2.0) - (w / 2.0 - r);
            let qy = abs(v - h / 2.0) - (h / 2.0 - r);
            let px = max(qx, 0.0);
            let py = max(qy, 0.0);
            let outside = sqrt(px * px + py * py);
            let d = outside + min(max(qx, qy), 0.0) - r;
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

// The paint at an output point, transparent outside the window
// (Placement::sample).
fn sample_at(x: f32, y: f32) -> vec4<f32> {
    let p = inverse(x, y);
    let cx = clip.window.x;
    let cy = clip.window.y;
    let w = clip.window.z;
    let h = clip.window.w;
    if p.x < cx || p.y < cy || p.x >= cx + w || p.y >= cy + h {
        return vec4<f32>(0.0);
    }
    return paint(p.x, p.y);
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
    return color * clip.opacity;
}
