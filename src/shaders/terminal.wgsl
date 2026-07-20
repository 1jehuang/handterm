struct Uniforms {
    screen_size: vec2<f32>,
    cell_size: vec2<f32>,
    atlas_size: vec2<f32>,
    grid_offset: vec2<f32>,
};

struct CellInstance {
    @location(0) pos: vec2<f32>,
    @location(1) size: vec2<f32>,
    @location(2) uv_offset: vec2<f32>,
    @location(3) uv_size: vec2<f32>,
    @location(4) fg: vec4<f32>,
    @location(5) bg: vec4<f32>,
    @location(6) deco: vec4<f32>,
    @location(7) flags: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) fg: vec4<f32>,
    @location(2) bg: vec4<f32>,
    @location(3) deco: vec4<f32>,
    @location(4) flags: u32,
    @location(5) local_pos: vec2<f32>,
    @location(6) cell_size: vec2<f32>,
};

@group(0) @binding(0) var<uniform> uniforms: Uniforms;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, instance: CellInstance) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vi];

    let pixel_pos = uniforms.grid_offset + instance.pos + corner * instance.size;
    let ndc = vec2<f32>(
        pixel_pos.x / uniforms.screen_size.x * 2.0 - 1.0,
        1.0 - pixel_pos.y / uniforms.screen_size.y * 2.0,
    );

    var out: VertexOutput;
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = (instance.uv_offset + corner * instance.uv_size) / uniforms.atlas_size;
    out.fg = instance.fg;
    out.bg = instance.bg;
    out.deco = instance.deco;
    out.flags = instance.flags;
    out.local_pos = corner * instance.size;
    out.cell_size = instance.size;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = in.bg;

    if (in.flags & 1u) != 0u {
        let glyph = textureSample(atlas_tex, atlas_sampler, in.uv);
        if (in.flags & 128u) != 0u {
            color = glyph;
        } else {
            color = vec4<f32>(in.fg.rgb, glyph.a * in.fg.a);
        }
    }

    let y = in.local_pos.y;
    let x = in.local_pos.x;
    let h = in.cell_size.y;
    let w = in.cell_size.x;

    if (in.flags & 2u) != 0u {
        let ul_y = h - 2.0;
        if y >= ul_y && y < ul_y + 1.0 {
            color = vec4<f32>(in.deco.rgb, 1.0);
        }
    }
    if (in.flags & 8u) != 0u {
        let ul_y = h - 2.0;
        let phase = x / w * 6.28318530718;
        let wave = sin(phase) * 2.0;
        if abs(y - (ul_y + wave)) < 1.5 {
            color = vec4<f32>(in.deco.rgb, 1.0);
        }
    }
    if (in.flags & 16u) != 0u {
        let ul_y1 = h - 2.0;
        let ul_y2 = h - 4.0;
        if (y >= ul_y1 && y < ul_y1 + 1.0) || (y >= ul_y2 && y < ul_y2 + 1.0) {
            color = vec4<f32>(in.deco.rgb, 1.0);
        }
    }
    if (in.flags & 32u) != 0u {
        let ul_y = h - 2.0;
        if y >= ul_y && y < ul_y + 1.0 && u32(x) % 3u == 0u {
            color = vec4<f32>(in.deco.rgb, 1.0);
        }
    }
    if (in.flags & 64u) != 0u {
        let ul_y = h - 2.0;
        let dash = u32(w) / 3u;
        let offset = u32(x);
        if y >= ul_y && y < ul_y + 1.0 && (offset < dash || (offset >= dash * 2u && offset < dash * 3u)) {
            color = vec4<f32>(in.deco.rgb, 1.0);
        }
    }

    if (in.flags & 4u) != 0u {
        let mid_y = h / 2.0;
        if y >= mid_y && y < mid_y + 1.0 {
            color = vec4<f32>(in.fg.rgb, 1.0);
        }
    }

    if (in.flags & 256u) != 0u && x < min(2.0, w) {
        color = vec4<f32>(in.fg.rgb, 1.0);
    }
    if (in.flags & 512u) != 0u {
        let cursor_y = h - min(2.0, h);
        if y >= cursor_y {
            color = vec4<f32>(in.fg.rgb, 1.0);
        }
    }

    return color;
}
