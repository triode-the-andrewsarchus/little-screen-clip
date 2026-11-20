#version 330 core
layout(points) in;
// Seven grid cells can cross at most 28 vertical, horizontal and diagonal edges.
layout(triangle_strip, max_vertices=66) out;
in vec4 v_points[];
in vec2 v_other[];
flat in int v_chunk[];
out vec2 v_geo;
out float v_front;
out float v_edge;
out float v_along;
uniform vec4 u_camera;
uniform vec2 u_view;
uniform float u_width;
uniform sampler2DArray u_terrain_dem;
uniform usampler2D u_terrain_pages;
uniform int u_dem_level;
uniform float u_drape_step;
uniform float u_drape_relief;

const float CELLS_PER_CHUNK = 7.0;

float wrap(float lon) { return mod(lon + 180.0, 360.0) - 180.0; }

uvec4 terrain_page(uvec2 cell, int level) {
    uint key = ((uint(level) << 24u) | (cell.y << 12u) | cell.x) + 1u;
    uint index = (key * 2654435761u) >> 23u;
    for (int i = 0; i < 512; ++i) {
        uvec4 entry = texelFetch(u_terrain_pages, ivec2(int(index), 0), 0);
        if (entry.x == key || entry.x == 0u) return entry;
        index = (index + 1u) & 511u;
    }
    return uvec4(0u);
}

vec3 mesh_vertex(vec2 grid, vec2 origin, float step, vec2 uv_offset, float uv_step, uint page) {
    vec2 geo = origin + grid * vec2(step, -step);
    vec3 normal = sphere_normal(geo);
    if (page == 0u) return normal;
    vec2 uv = ((uv_offset + grid * uv_step) * 256.0 + 1.0) / 258.0;
    vec2 dem = textureLod(u_terrain_dem, vec3(uv, float(page - 1u)), 0.0).rg;
    return normal * (1.0 + (dem.g > .99 ? max(0.0, dem.r) : 0.0) / EARTH_RADIUS_M);
}

vec3 mesh_triangle(vec2 pos, uvec2 patch, int level, uvec4 entry) {
    float steps = float(entry.w);
    float side = 180.0 / float(1 << level);
    float step = side / steps;
    vec2 origin = vec2(-180.0, 90.0) + vec2(patch) * vec2(side, -side);
    uint scale = 1u << (uint(level) - entry.z);
    vec2 uv_offset = vec2(patch % scale) / float(scale);
    float uv_step = 1.0 / (steps * float(scale));
    vec2 p = pos * steps;
    vec2 grid = min(floor(p), vec2(steps - 1.0));
    vec2 f = p - grid;
    vec3 east = mesh_vertex(grid + vec2(1, 0), origin, step, uv_offset, uv_step, entry.y);
    vec3 south = mesh_vertex(grid + vec2(0, 1), origin, step, uv_offset, uv_step, entry.y);
    if (f.x + f.y <= 1.0) {
        vec3 corner = mesh_vertex(grid, origin, step, uv_offset, uv_step, entry.y);
        return corner + f.x * (east - corner) + f.y * (south - corner);
    }
    vec3 corner = mesh_vertex(grid + vec2(1, 1), origin, step, uv_offset, uv_step, entry.y);
    return corner + (1.0 - f.y) * (east - corner) + (1.0 - f.x) * (south - corner);
}

vec3 mesh_position(vec2 geo) {
    geo.x += u_camera.y;
    vec2 uv = vec2(fract((geo.x+180.0)/360.0), clamp((90.0-geo.y)/180.0, 0.0, .99999994));
    for (int level = u_dem_level; level >= 0; --level) {
        vec2 position = uv * vec2(2 << level, 1 << level);
        uvec2 patch = uvec2(position);
        uvec4 entry = terrain_page(patch, level);
        if (entry.x != 0u) return mesh_triangle(fract(position), patch, level, entry);
    }
    return sphere_normal(geo);
}

vec4 draped_clip(vec2 geo) {
    vec4 clip = surface_clip(mesh_position(geo));
    // Put the stroke two metres toward the eye without moving it across the
    // screen. The depth test still clips it against intervening terrain.
    if (clip.w > u_near) {
        clip.z = clip.w - 2.0 * u_near * clip.w / max(u_near, clip.w - 2.0 / EARTH_RADIUS_M);
    }
    return clip;
}

vec2 screen(vec4 clip) { return clip.xy * u_view * .5 / max(clip.w, u_near); }

void vertex_at(vec2 geo, vec4 clip, float edge, float along) {
    v_geo = geo;
    vec3 normal = sphere_normal(vec2(geo.x + u_camera.y, geo.y));
    v_front = dot(normal, normalize(u_eye - normal));
    v_edge = edge;
    v_along = along;
    gl_Position = clip;
    EmitVertex();
}

// All terrain grids have power-of-two resolution. Their triangle edges lie
// on vertical, horizontal or descending diagonal lines in the finest grid.
vec3 grid_coordinates(vec2 geo) {
    vec2 local = vec2(geo.x + mod(u_camera.y, u_drape_step),
        u_camera.x - geo.y + mod(-u_camera.x, u_drape_step));
    return vec3(local, local.x + local.y) / u_drape_step;
}

vec3 first_crossings(vec3 origin, vec3 direction) {
    vec3 forward = floor(origin + 1e-4) + 1.0 - origin;
    vec3 backward = origin - (ceil(origin - 1e-4) - 1.0);
    return mix(forward, backward, lessThan(direction, vec3(0.0))) / max(abs(direction), vec3(1e-7));
}

// Test a conservative Earth-fixed sphere before any atlas reads. The
// margin covers the widest river stroke and the tile's displacement.
bool chunk_visible(vec2 a, vec2 b) {
    vec2 d = abs(b - a);
    float radius = radians(d.x + d.y) * .5 + u_drape_relief;
    vec2 midpoint = (a + b) * .5 + vec2(u_camera.y, 0.0);
    vec3 center = transpose(u_view_axes) * (sphere_normal(midpoint) - u_eye);
    float far_depth = -center.z + radius;
    vec2 frustum = u_half_fov * (1.0 + 52.0 / u_view);
    return far_depth >= u_near
        && all(lessThanEqual(abs(center.xy), far_depth * frustum + radius));
}

void line_strip(vec2 a, vec2 b) {
    vec2 span = abs(b - a);
    int chunks = max(1, int(ceil(max(span.x, span.y) / (CELLS_PER_CHUNK * u_drape_step))));
    int chunk = v_chunk[0];
    if (chunk >= chunks) return;
    vec2 start = mix(a, b, float(chunk) / float(chunks));
    vec2 end = mix(a, b, float(chunk+1) / float(chunks));
    if (!chunk_visible(start, end)) return;
    vec2 delta = end - start;
    vec3 direction = vec3(delta.x, -delta.y, delta.x - delta.y) / u_drape_step;
    vec3 crossings = first_crossings(grid_coordinates(start), direction);
    vec3 spacing = 1.0 / max(abs(direction), vec3(1e-7));
    vec2 point = start;
    vec4 clip = draped_clip(point);
    vec4 previous = chunk == 0 ? clip : draped_clip(start - delta * .05);
    float along = 0.0;
    float t = 0.0;
    for (int i = 0; i <= 32; ++i) {
        float next_t = min(1.0, min(crossings.x, min(crossings.y, crossings.z)));
        vec2 next = mix(start, end, next_t);
        vec4 following = t < 1.0 ? draped_clip(next)
            : (chunk+1 == chunks ? clip : draped_clip(end + delta * .05));
        vec2 direction_px = screen(following) - screen(previous);
        vec2 normal = vec2(-direction_px.y, direction_px.x) / max(length(direction_px), .001);
        float focal = u_view.y / (2.0 * u_half_fov.y);
        float width = u_width < 0.0
            ? clamp(v_other[0].x / EARTH_RADIUS_M * focal / max(clip.w, u_near), .8, 48.0)
            : u_width;
        vec2 offset = normal * (width + 1.2) * clip.w / u_view;
        vertex_at(point, clip - vec4(offset, 0, 0), -1.0, along);
        vertex_at(point, clip + vec4(offset, 0, 0),  1.0, along);
        if (t >= 1.0) break;
        crossings += mix(vec3(0.0), spacing, lessThanEqual(crossings, vec3(next_t + 1e-5)));
        along += length(screen(following) - screen(clip));
        previous = clip;
        clip = following;
        point = next;
        t = next_t;
    }
}

void main() {
    vec2 a = v_points[0].xy, b = v_points[0].zw;
    float dx = wrap(b.x - a.x);
    float start = wrap(a.x + dx * .5 - u_camera.y) - dx * .5;
    if (u_width != 0.0) {
        line_strip(vec2(start, a.y), vec2(start + dx, b.y));
    } else {
        vec2 c = vec2(start + wrap(v_other[0].x - a.x), v_other[0].y);
        a.x = start;
        b.x = start + dx;
        vertex_at(a, draped_clip(a), 0.0, 0.0);
        vertex_at(b, draped_clip(b), 0.0, 0.0);
        vertex_at(c, draped_clip(c), 0.0, 0.0);
    }
    EndPrimitive();
}
