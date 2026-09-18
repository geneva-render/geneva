// One box blur pass over a layer, along its rows or down its columns: a
// transcription of the CPU renderer's box_blur_rows and box_blur_columns
// (crates/geneva-render/src/blur.rs). Each texel becomes the mean of
// itself and its `radius` neighbours on either side, with texels beyond
// the layer counting as transparent, so a layer drawn over transparency
// fades out at its border. Three passes each way approximate a Gaussian.

struct Box {
    radius: i32,
    // 0 along the rows, 1 down the columns.
    vertical: u32,
};

@group(0) @binding(0) var<uniform> box_pass: Box;
@group(0) @binding(1) var src: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the whole target.
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(i32(pos.x), i32(pos.y));
    let d = vec2<i32>(textureDimensions(src));
    var step = vec2<i32>(1, 0);
    var extent = d.x;
    var along = p.x;
    if box_pass.vertical != 0u {
        step = vec2<i32>(0, 1);
        extent = d.y;
        along = p.y;
    }
    let r = box_pass.radius;
    var acc = vec4<f32>(0.0);
    for (var i = -r; i <= r; i++) {
        let q = along + i;
        if q >= 0 && q < extent {
            acc += textureLoad(src, p + step * i, 0);
        }
    }
    return acc * (1.0 / f32(2 * r + 1));
}
