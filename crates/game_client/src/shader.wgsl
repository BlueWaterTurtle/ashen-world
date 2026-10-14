struct Globals {
    view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    sun_dir: vec4<f32>,   // unit vector pointing toward the sun
    fog: vec4<f32>,       // rgb = fog colour (linear), w = density
};
@group(0) @binding(0) var<uniform> g: Globals;

struct Material {
    base_color: vec4<f32>,
};
@group(1) @binding(0) var base_color_texture: texture_2d<f32>;
@group(1) @binding(1) var base_color_sampler: sampler;
@group(1) @binding(2) var<uniform> material: Material;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tint: vec3<f32>,
    @location(3) uv: vec2<f32>,
};

fn srgb(c: vec3<f32>) -> vec3<f32> {
    return pow(c, vec3<f32>(2.2));
}

fn light(base: vec3<f32>, n_in: vec3<f32>, world: vec3<f32>) -> vec3<f32> {
    let n = normalize(n_in);
    let ndl = max(dot(n, normalize(g.sun_dir.xyz)), 0.0);
    let sky = vec3<f32>(0.10, 0.12, 0.17);
    let ground = vec3<f32>(0.04, 0.03, 0.025);
    let ambient = mix(ground, sky, n.y * 0.5 + 0.5);
    let sun = vec3<f32>(1.0, 0.80, 0.55);
    let lit = base * (ambient + sun * ndl * 1.25);
    let d = length(world - g.cam_pos.xyz);
    let f = clamp(1.0 - exp(-pow(d * g.fog.w, 2.0)), 0.0, 1.0);
    return mix(lit, g.fog.xyz, f);
}

@vertex
fn vs_terrain(@location(0) pos: vec3<f32>, @location(1) normal: vec3<f32>) -> VsOut {
    var o: VsOut;
    o.clip = g.view_proj * vec4<f32>(pos, 1.0);
    o.world = pos;
    o.normal = normal;
    o.tint = vec3<f32>(1.0);
    o.uv = vec2<f32>(0.0);
    return o;
}

@fragment
fn fs_terrain(i: VsOut) -> @location(0) vec4<f32> {
    let t = clamp(i.world.y / 40.0, 0.0, 1.0);
    let macro_noise = textureSample(
        base_color_texture,
        base_color_sampler,
        i.world.xz * 0.008,
    ).r;
    let fine_noise = textureSample(
        base_color_texture,
        base_color_sampler,
        i.world.xz * 0.18,
    ).r;
    let ash = srgb(vec3<f32>(0.27, 0.24, 0.21));
    let dust = srgb(vec3<f32>(0.48, 0.38, 0.27));
    let rock = srgb(vec3<f32>(0.56, 0.53, 0.48));
    let dust_patch = smoothstep(0.24, 0.76, macro_noise);
    var base = mix(ash, dust, 0.55 + dust_patch * 0.40);
    base = mix(base, rock, smoothstep(0.30, 0.85, t));
    let n = normalize(i.normal);
    let steep = clamp((1.0 - n.y) * 6.0, 0.0, 1.0);
    base = mix(base, srgb(vec3<f32>(0.40, 0.38, 0.36)), steep * 0.6);
    let fine_grain = 0.94 + fine_noise * 0.12;
    base = base * fine_grain;
    return vec4<f32>(light(base, n, i.world), 1.0);
}

@vertex
fn vs_agent(
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) inst_pos: vec4<f32>,
    @location(4) inst_color: vec4<f32>,
) -> VsOut {
    var o: VsOut;
    let world = pos + inst_pos.xyz;
    o.clip = g.view_proj * vec4<f32>(world, 1.0);
    o.world = world;
    o.normal = normal;
    o.tint = inst_color.rgb;
    o.uv = uv;
    return o;
}

@fragment
fn fs_agent(i: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(base_color_texture, base_color_sampler, i.uv);
    let base = texel.rgb * material.base_color.rgb * srgb(i.tint);
    return vec4<f32>(light(base, i.normal, i.world), texel.a * material.base_color.a);
}
