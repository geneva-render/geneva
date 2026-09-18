// The composite of a markup box's layers: a transcription of the CPU
// painter's `composite` (crates/geneva-render/src/html.rs), which lays
// a group's buffer into what holds it under its opacity, blur, clips
// and transform, and of the decode that turns the finished box into
// linear light once (`to_linear`).
//
// Everything here is in the painter's working space, sRGB-encoded
// channels premultiplied by alpha, the space a browser blends in. One
// group draw covers where the buffer lands with two triangles; the
// fragment shader maps each pixel back into the buffer through the
// inverse affine, samples it there bilinearly (one tap, or up to four
// each way for a buffer drawn smaller, so its edges do not alias), each
// texel scaled by the polygon clip's coverage where there is one,
// scales the sample by the opacity and the rounded clip's coverage at
// the pixel, and hands the premultiplied color to fixed-function "over".
// The decode is a pass over the whole box.

struct Group {
    // The target pixels drawn: x0, y0, x1, y1.
    bounds: vec4<f32>,
    // The part of the image that is the buffer: x, y, width, height in
    // its texels. Outside it the buffer is transparent.
    window: vec4<f32>,
    // The rounded clip on the surface: x, y, width, height.
    clip: vec4<f32>,
    // Its corner radii: top-left, top-right, bottom-right, bottom-left.
    radius: vec4<f32>,
    // The inverse affine's factors: ax, bx, ay, by.
    inverse_a: vec4<f32>,
    // Its constants: cx, cy. A surface point (px, py) maps to the
    // buffer pixel u = ax * px + bx * py + cx, v = ay * px + by * py + cy.
    inverse_c: vec2<f32>,
    // The target's width and height.
    extent: vec2<f32>,
    // Where the target's top-left pixel sits on the surface.
    origin: vec2<f32>,
    // Samples per pixel along each axis.
    taps: vec2<u32>,
    opacity: f32,
    has_clip: u32,
    has_mask: u32,
    pad: u32,
};

@group(0) @binding(0) var<uniform> group: Group;
@group(0) @binding(1) var image: texture_2d<f32>;
// The polygon clip's coverage over the buffer, one texel per buffer
// pixel, when `has_mask` is set.
@group(0) @binding(2) var mask: texture_2d<f32>;

struct Vertex {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> Vertex {
    // Two triangles over the bounds. Target y grows down, clip-space y up.
    var corner = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corner[i];
    let px = mix(group.bounds.x, group.bounds.z, c.x);
    let py = mix(group.bounds.y, group.bounds.w, c.y);
    var out: Vertex;
    out.position = vec4<f32>(
        px / group.extent.x * 2.0 - 1.0,
        1.0 - py / group.extent.y * 2.0,
        0.0,
        1.0,
    );
    return out;
}

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> Vertex {
    // One triangle covering the whole target.
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    var out: Vertex;
    out.position = vec4<f32>(x, y, 0.0, 1.0);
    return out;
}

// a + (b - a) * t, in the CPU painter's order of operations.
fn lerp(a: vec4<f32>, b: vec4<f32>, t: f32) -> vec4<f32> {
    return a + (b - a) * t;
}

// The buffer's pixel at integer coordinates, transparent outside it
// (Image::texel on the buffer), through the polygon clip's coverage
// (mask_polygon).
fn texel(x: i32, y: i32) -> vec4<f32> {
    if x < 0 || y < 0 || x >= i32(group.window.z) || y >= i32(group.window.w) {
        return vec4<f32>(0.0);
    }
    var t = textureLoad(image, vec2<i32>(x + i32(group.window.x), y + i32(group.window.y)), 0);
    if group.has_mask != 0u {
        let c = textureLoad(mask, vec2<i32>(x, y), 0).r;
        if c < 1.0 {
            t = t * c;
        }
    }
    return t;
}

// Bilinear sample with texel centers at half integers (Image::sample).
fn sample_buffer(u: f32, v: f32) -> vec4<f32> {
    let fx = u - 0.5;
    let fy = v - 0.5;
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

// Signed distance to the rounded clip at a surface point, negative
// inside, with the corner's own radius (distance).
fn clip_distance(px: f32, py: f32) -> f32 {
    let w = group.clip.z;
    let h = group.clip.w;
    if w <= 0.0 || h <= 0.0 {
        return 1.0;
    }
    let cx = px - group.clip.x - w / 2.0;
    let cy = py - group.clip.y - h / 2.0;
    // A radius never takes more than half the shorter side.
    let limit = min(w / 2.0, h / 2.0);
    var r: f32;
    if cx > 0.0 {
        if cy > 0.0 {
            r = group.radius.z;
        } else {
            r = group.radius.y;
        }
    } else {
        if cy > 0.0 {
            r = group.radius.w;
        } else {
            r = group.radius.x;
        }
    }
    r = clamp(r, 0.0, limit);
    let qx = abs(cx) - (w / 2.0 - r);
    let qy = abs(cy) - (h / 2.0 - r);
    let ox = max(qx, 0.0);
    let oy = max(qy, 0.0);
    let outside = sqrt(ox * ox + oy * oy);
    return outside + min(max(qx, qy), 0.0) - r;
}

// The clip's coverage at a pixel center, softened over one pixel so
// edges do not stair-step (coverage).
fn clip_coverage(px: f32, py: f32) -> f32 {
    return clamp(0.5 - clip_distance(px, py), 0.0, 1.0);
}

// The buffer at a surface point, transparent outside it.
fn sample_at(px: f32, py: f32) -> vec4<f32> {
    let u = group.inverse_a.x * px + group.inverse_a.y * py + group.inverse_c.x;
    let v = group.inverse_a.z * px + group.inverse_a.w * py + group.inverse_c.y;
    if u >= 0.0 && v >= 0.0 && u < group.window.z && v < group.window.w {
        return sample_buffer(u, v);
    }
    return vec4<f32>(0.0);
}

@fragment
fn fs_group(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // The fragment's position is the pixel's center; the pixel's
    // surface coordinates come from the target's origin.
    let dx = floor(pos.x) + group.origin.x;
    let dy = floor(pos.y) + group.origin.y;
    let nx = group.taps.x;
    let ny = group.taps.y;
    var color: vec4<f32>;
    if nx == 1u && ny == 1u {
        // A buffer drawn at its own size or larger: one tap a pixel.
        color = sample_at(dx + 0.5, dy + 0.5);
    } else {
        // Drawn smaller: several taps a pixel, averaged.
        var acc = vec4<f32>(0.0);
        for (var j = 0u; j < ny; j++) {
            let py = dy + (f32(j) + 0.5) / f32(ny);
            for (var i = 0u; i < nx; i++) {
                let px = dx + (f32(i) + 0.5) / f32(nx);
                acc += sample_at(px, py);
            }
        }
        color = acc * (1.0 / f32(nx * ny));
    }
    var a = group.opacity;
    if group.has_clip != 0u {
        a = a * clip_coverage(dx + 0.5, dy + 0.5);
    }
    return color * a;
}

// The sRGB curve to linear light, unclamped (srgb_to_linear).
fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

// The finished box's pixel in linear light (decode_pixel): each channel
// taken straight, decoded, and premultiplied again.
@fragment
fn fs_decode(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = textureLoad(image, vec2<i32>(i32(pos.x), i32(pos.y)), 0);
    if p.a <= 0.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(
        srgb_to_linear(p.r / p.a) * p.a,
        srgb_to_linear(p.g / p.a) * p.a,
        srgb_to_linear(p.b / p.a) * p.a,
        p.a,
    );
}
