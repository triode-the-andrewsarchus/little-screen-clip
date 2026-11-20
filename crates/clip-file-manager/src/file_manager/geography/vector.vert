#version 330 core
layout(location=0) in vec4 a_points;
layout(location=1) in vec2 a_other;
uniform vec4 u_camera;
uniform vec2 u_view;
uniform float u_width;
uniform float u_shift;
uniform sampler2DArray u_terrain_dem;
uniform usampler2D u_terrain_pages;
uniform int u_dem_level;
out vec2 v_geo;
out float v_front;
out float v_edge;
out float v_along;
const float RAD=0.017453292519943295;
vec3 project(vec2 geo) {
    float phi=geo.y*RAD, phi0=u_camera.x*RAD, dl=geo.x*RAD;
    vec2 globe=vec2(cos(phi)*sin(dl),-(cos(phi0)*sin(phi)-sin(phi0)*cos(phi)*cos(dl)));
    vec2 planar=vec2(dl*max(abs(cos(phi0)),0.25),-(geo.y-u_camera.x)*RAD);
    return vec3(mix(planar,globe,u_camera.w)*u_camera.z,sin(phi0)*sin(phi)+cos(phi0)*cos(phi)*cos(dl));
}
float wrap(float lon) { return mod(lon+180.0,360.0)-180.0; }
float drape_height(vec2 geo) {
    vec2 uv = vec2(fract((geo.x+180.0)/360.0), clamp((90.0-geo.y)/180.0, 0.0, .99999994));
    for (int level = u_dem_level; level >= 0; --level) {
        vec2 position = uv * vec2(2 << level, 1 << level);
        uvec2 cell = uvec2(position);
        uint key = ((uint(level) << 24u) | (cell.y << 12u) | cell.x) + 1u;
        uint index = (key * 2654435761u) >> 23u;
        for (int probe = 0; probe < 512; ++probe) {
            uvec2 entry = texelFetch(u_terrain_pages, ivec2(int(index), 0), 0).rg;
            if (entry.x == 0u) break;
            if (entry.x == key) {
                vec2 sample_uv = (fract(position) * 256.0 + 1.0) / 258.0;
                vec2 dem = textureLod(u_terrain_dem, vec3(sample_uv, float(entry.y)), 0.0).rg;
                return max(0.0, dem.r * float(dem.g > .99));
            }
            index = (index + 1u) & 511u;
        }
    }
    return 0.0;
}

vec4 terrain_vector(vec2 geo) {
    geo.x += u_camera.y;
    return terrain_clip(sphere_normal(geo), drape_height(geo) + 3.0);
}

void draped(vec2 a, vec2 b, float start) {
    vec2 geo;
    vec4 clip;
    if (u_width == 0.0) {
        vec2 c = vec2(start+wrap(a_other.x-a_points.x), a_other.y);
        geo = gl_VertexID == 0 ? a : (gl_VertexID == 1 ? b : c);
        clip = terrain_vector(geo);
        v_edge = 0.0; v_along = 0.0;
    } else {
        vec4 ca = terrain_vector(a), cb = terrain_vector(b);
        vec2 direction = (cb.xy / max(cb.w,u_near) - ca.xy / max(ca.w,u_near)) * u_view * .5;
        float len = max(length(direction), .001);
        vec2 normal = vec2(-direction.y,direction.x)/len;
        bool last = gl_VertexID==1 || gl_VertexID==2 || gl_VertexID==4;
        float side = (gl_VertexID==0 || gl_VertexID==1 || gl_VertexID==5) ? -1.0 : 1.0;
        geo = last ? b : a;
        clip = last ? cb : ca;
        float focal = u_view.y / (2.0 * u_half_fov.y);
        float width = u_width < 0.0
            ? clamp(a_other.x / EARTH_RADIUS_M * focal / max(clip.w, u_near), .8, 48.0)
            : u_width;
        clip.xy += normal * side * (width + 1.2) * clip.w / u_view;
        v_edge = side; v_along = last ? len : 0.0;
    }
    vec3 normal = sphere_normal(vec2(geo.x + u_camera.y, geo.y));
    v_geo = geo;
    v_front = dot(normal, normalize(u_eye - normal));
    gl_Position = clip;
}

void main() {
    vec2 a=a_points.xy, b=a_points.zw;
    float dx=wrap(b.x-a.x);
    float middle=a.x+dx*0.5;
    float start=wrap(middle-u_camera.y)-dx*0.5+u_shift;
    a.x=start; b.x=start+dx;
    if (u_perspective) {
        draped(a, b, start);
        return;
    }
    vec2 geo;
    vec2 screen;
    if(u_width==0.0) {
        vec2 c=a_other;
        c.x=start+wrap(c.x-a_points.x);
        geo=gl_VertexID==0?a:(gl_VertexID==1?b:c);
        screen=project(geo).xy;
        v_edge=0.0; v_along=0.0;
    } else {
        vec2 pa=project(a).xy, pb=project(b).xy;
        vec2 direction=pb-pa;
        float len=max(length(direction),0.001);
        vec2 normal=vec2(-direction.y,direction.x)/len;
        bool end_point=gl_VertexID==1 || gl_VertexID==2 || gl_VertexID==4;
        float side=(gl_VertexID==0 || gl_VertexID==1 || gl_VertexID==5)?-1.0:1.0;
        geo=end_point?b:a;
        float width=u_width<0.0?clamp(a_other.x/6371000.0*u_camera.z/max(abs(cos(radians(geo.y))),0.15),0.8,48.0):u_width;
        screen=(end_point?pb:pa)+normal*side*(width*0.5+0.6);
        v_edge=side; v_along=end_point?len:0.0;
    }
    v_geo=geo;
    v_front=project(geo).z;
    gl_Position=vec4(screen.x*2.0/u_view.x,-screen.y*2.0/u_view.y,0.0,1.0);
}
