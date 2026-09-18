// The pack of a rendered frame into an encoder's planes: a transcription
// of frame_to_planes_into (crates/geneva-media/src/convert.rs). One pass
// per plane, a fullscreen triangle over a texture of the plane's size:
// the luma plane encodes each pixel, a chroma plane averages the block
// of pixels it stands for (fewer at a right or bottom edge), and an
// RGBA plane writes straight-alpha sRGB bytes. Integer targets, so the
// codes are exactly what the shader computes.

struct Pack {
    // The range: luma scale and offset, chroma scale and offset.
    yuv: vec4<f32>,
    // The matrix: kr, kb, kg, and whether m0..m2 apply.
    coef: vec4<f32>,
    // Rows of the matrix taking working-space light into the output's
    // primaries.
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
    // The largest code.
    max: f32,
    // The peak of an HDR output, in units of reference white.
    hdr_peak: f32,
    // Whether the table is the HDR one.
    hdr: u32,
    // 0 luma, 1 Cb, 2 Cr, 3 RGBA.
    mode: u32,
    // The chroma block: pixels per sample across and down.
    block: vec2<u32>,
    // The frame's size.
    size: vec2<u32>,
};

@group(0) @binding(0) var<uniform> pack: Pack;
@group(0) @binding(1) var frame: texture_2d<f32>;
// The output's transfer function, linear light to the non-linear value,
// one entry per 16-bit step.
@group(0) @binding(2) var<storage, read> encode: array<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

// The index into a 16-bit table for a value nominally in [0, 1]
// (lut_index).
fn lut_index(v: f32) -> i32 {
    let i = i32(v * 65535.0 + 0.5);
    return clamp(i, 0, 65535);
}

// Light to the output's non-linear value (enc): through the SDR table,
// or the HDR table by the fourth root of the fraction of the peak.
fn enc(v: f32) -> f32 {
    if pack.hdr != 0u {
        return encode[lut_index(sqrt(sqrt(max(v, 0.0) / pack.hdr_peak)))];
    }
    return encode[lut_index(v)];
}

// One pixel's Y', Cb, Cr before scaling (to_ycc). Premultiplied over
// opaque black is the premultiplied value itself.
fn to_ycc(p: vec4<f32>) -> vec3<f32> {
    var rgb = p.rgb;
    if pack.coef.w != 0.0 {
        rgb = vec3<f32>(
            pack.m0.x * rgb.x + pack.m0.y * rgb.y + pack.m0.z * rgb.z,
            pack.m1.x * rgb.x + pack.m1.y * rgb.y + pack.m1.z * rgb.z,
            pack.m2.x * rgb.x + pack.m2.y * rgb.y + pack.m2.z * rgb.z,
        );
    }
    let r = enc(rgb.x);
    let g = enc(rgb.y);
    let b = enc(rgb.z);
    let kr = pack.coef.x;
    let kb = pack.coef.y;
    let kg = pack.coef.z;
    let y = kr * r + kg * g + kb * b;
    let cb = (b - y) / (2.0 * (1.0 - kb));
    let cr = (r - y) / (2.0 * (1.0 - kr));
    return vec3<f32>(y, cb, cr);
}

// A code from a normalized value (code): rounded, clamped, truncated.
fn code(v: f32, scale: f32, off: f32) -> u32 {
    return u32(clamp(v * scale + off + 0.5, 0.0, pack.max));
}

@fragment
fn fs_plane(@builtin(position) pos: vec4<f32>) -> @location(0) u32 {
    let p = vec2<i32>(i32(pos.x), i32(pos.y));
    if pack.mode == 0u {
        let ycc = to_ycc(textureLoad(frame, p, 0));
        return code(ycc.x, pack.yuv.x, pack.yuv.y);
    }
    // A chroma sample stands for a block of pixels, row by row, with
    // fewer at a right or bottom edge.
    let w = i32(pack.size.x);
    let h = i32(pack.size.y);
    let bx = i32(pack.block.x);
    let by = i32(pack.block.y);
    var sum = 0.0;
    var count = 0.0;
    for (var dy = 0; dy < by; dy++) {
        for (var dx = 0; dx < bx; dx++) {
            let x = p.x * bx + dx;
            let y = p.y * by + dy;
            if x < w && y < h {
                let ycc = to_ycc(textureLoad(frame, vec2<i32>(x, y), 0));
                if pack.mode == 1u {
                    sum += ycc.y;
                } else {
                    sum += ycc.z;
                }
                count += 1.0;
            }
        }
    }
    return code(sum / count, pack.yuv.z, pack.yuv.w);
}

// The 8-bit sRGB code of a straight linear value (srgb8_reference).
fn srgb8(v: f32) -> u32 {
    let c = clamp(v, 0.0, 1.0);
    var s: f32;
    if c <= 0.0031308 {
        s = 12.92 * c;
    } else {
        s = 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    }
    return u32(clamp(s, 0.0, 1.0) * 255.0 + 0.5);
}

@fragment
fn fs_rgba(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<u32> {
    let p = vec2<i32>(i32(pos.x), i32(pos.y));
    let px = textureLoad(frame, p, 0);
    // Straight alpha (to_srgb8): a transparent pixel is all zero.
    if px.a <= 0.0 {
        return vec4<u32>(0u);
    }
    let a = u32(min(px.a, 1.0) * 255.0 + 0.5);
    return vec4<u32>(srgb8(px.r / px.a), srgb8(px.g / px.a), srgb8(px.b / px.a), a);
}
