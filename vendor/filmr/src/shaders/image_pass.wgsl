struct Dimensions { width: u32, height: u32 }
@group(0) @binding(0) var<storage, read> source: array<f32>;
@group(0) @binding(1) var<storage, read_write> output: array<f32>;
@group(0) @binding(2) var<storage, read> params: array<f32>;
@group(0) @binding(3) var<uniform> dims: Dimensions;

fn read_rgb(index: u32) -> vec3<f32> {
    return vec3<f32>(source[index], source[index + 1u], source[index + 2u]);
}
fn write_rgb(index: u32, color: vec3<f32>) {
    output[index] = color.r; output[index + 1u] = color.g; output[index + 2u] = color.b;
}
fn coefficients(index: u32) -> vec3<f32> {
    return vec3<f32>(params[index], params[index + 1u], params[index + 2u]);
}

// The full spectral propagation is linear in incident RGB. Its integrated
// response to each RGB basis is computed through all 81 bins on the CPU once.
@compute @workgroup_size(16, 16, 1)
fn exposure(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= dims.width || id.y >= dims.height) { return; }
    let index = (id.y * dims.width + id.x) * 3u;
    let rgb = read_rgb(index);
    write_rgb(index, vec3<f32>(dot(coefficients(0u), rgb), dot(coefficients(3u), rgb), dot(coefficients(6u), rgb)));
}

fn round_away(value: f32) -> f32 { return sign(value) * floor(abs(value) + 0.5); }
@compute @workgroup_size(16, 16, 1)
fn motion(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= dims.width || id.y >= dims.height) { return; }
    let index = (id.y * dims.width + id.x) * 3u;
    let original = read_rgb(index);
    var color = vec3<f32>(0.0);
    for (var i = 0u; i < arrayLength(&params); i += 3u) {
        let sx = i32(round_away(f32(id.x) - params[i]));
        let sy = i32(round_away(f32(id.y) - params[i + 1u]));
        var sample = original;
        if (sx >= 0 && sx < i32(dims.width) && sy >= 0 && sy < i32(dims.height)) {
            sample = read_rgb((u32(sy) * dims.width + u32(sx)) * 3u);
        }
        color += params[i + 2u] * sample;
    }
    write_rgb(index, color);
}

// Identical 81-bin dye transmission and D65/CIE integration to CPU output.
@compute @workgroup_size(16, 16, 1)
fn spectral_output(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= dims.width || id.y >= dims.height) { return; }
    let index = (id.y * dims.width + id.x) * 3u;
    let net = max(read_rgb(index) - coefficients(0u), vec3<f32>(0.0));
    var xyz = vec3<f32>(0.0);
    for (var i = 0u; i < 81u; i++) {
        let offset = 16u + i * 6u;
        let density = dot(net, coefficients(offset));
        xyz += pow(10.0, -density) * coefficients(offset + 3u);
    }
    xyz *= params[6u];
    var rgb = vec3<f32>(dot(coefficients(7u), xyz), dot(coefficients(10u), xyz), dot(coefficients(13u), xyz));
    if (params[3u] > 0.0) { rgb = vec3<f32>(1.0) - rgb; }
    if (params[4u] != 1.0) {
        let lum = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        rgb = vec3<f32>(lum) + (rgb - vec3<f32>(lum)) * params[4u];
    }
    if (params[5u] > 0.0) {
        let dx = (f32(id.x) + 0.5) / f32(dims.width) - 0.5;
        let dy = (f32(id.y) + 0.5) / f32(dims.height) - 0.5;
        let denominator = 1.0 + (dx * dx + dy * dy) / 0.2;
        let cos4 = 1.0 / (denominator * denominator);
        rgb *= 1.0 - params[5u] * (1.0 - cos4);
    }
    write_rgb(index, rgb);
}
