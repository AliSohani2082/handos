// One instance per line segment or joint: a quad around the segment, shaded by distance to it.
// A joint is a segment whose two ends are the same point.

struct Globals {
    size: vec2<f32>, // surface size in pixels
};

@group(0) @binding(0) var<uniform> g: Globals;

struct Inst {
    @location(0) a: vec2<f32>,
    @location(1) b: vec2<f32>,
    @location(2) radius: f32,
    @location(3) color: vec4<f32>, // straight alpha
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) px: vec2<f32>,
    @location(1) a: vec2<f32>,
    @location(2) b: vec2<f32>,
    @location(3) radius: f32,
    @location(4) color: vec4<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32, inst: Inst) -> VsOut {
    let corner = vec2<f32>(f32(vi & 1u), f32((vi >> 1u) & 1u)); // triangle strip: 0,0 1,0 0,1 1,1
    let pad = inst.radius + 1.0;
    let lo = min(inst.a, inst.b) - vec2<f32>(pad);
    let hi = max(inst.a, inst.b) + vec2<f32>(pad);
    let px = mix(lo, hi, corner);
    var out: VsOut;
    out.pos = vec4<f32>(px.x / g.size.x * 2.0 - 1.0, 1.0 - px.y / g.size.y * 2.0, 0.0, 1.0);
    out.px = px;
    out.a = inst.a;
    out.b = inst.b;
    out.radius = inst.radius;
    out.color = inst.color;
    return out;
}

@fragment
fn fs(v: VsOut) -> @location(0) vec4<f32> {
    let ab = v.b - v.a;
    let len2 = dot(ab, ab);
    var h = 0.0;
    if (len2 > 0.0) {
        h = clamp(dot(v.px - v.a, ab) / len2, 0.0, 1.0);
    }
    let d = length(v.px - (v.a + ab * h));
    let cover = clamp(v.radius - d + 0.5, 0.0, 1.0);
    if (cover <= 0.0) {
        discard;
    }
    let a = v.color.a * cover;
    return vec4<f32>(v.color.rgb * a, a); // premultiplied
}
