#version 330 core
layout(location=0) in vec2 a_uv;
layout(location=1) in float a_skirt;
uniform vec4 u_camera; // latitude, longitude, scale in logical pixels, globe blend
uniform vec2 u_view;
uniform vec4 u_tile; // west relative to camera, north, angular side, unused
uniform vec3 u_patch; // tile-relative minimum UV and patch span
uniform sampler2D u_height;
uniform bool u_signed_elevation;
out vec2 v_uv;
out vec2 v_geo;
out float v_front;
const float RAD=0.017453292519943295;
void main() {
    vec2 uv = u_perspective ? u_patch.xy + a_uv * u_patch.z : a_uv;
    float dl=u_tile.x+uv.x*u_tile.z;
    float lat=u_tile.y-uv.y*u_tile.z;
    v_uv=(uv*256.0+1.0)/258.0;
    v_geo=vec2(dl,lat);
    if (u_perspective) {
        vec3 normal = sphere_normal(vec2(u_camera.y+dl, lat));
        vec2 height = textureLod(u_height, v_uv, 0.0).rg;
        float elevation = height.g > .99 ? (u_signed_elevation ? height.r : max(0.0, height.r)) : 0.0;
        // Skirts close openings between meshes using different source levels.
        float edge = a_skirt * clamp(u_tile.z * u_patch.z * 380.0, 45.0, 600.0);
        vec3 surface = normal * (1.0 + (elevation-edge) / u_body_radius_m);
        v_front = dot(normal, normalize(u_eye - surface));
        gl_Position = terrain_clip(normal, elevation-edge);
        return;
    }
    float phi=lat*RAD, phi0=u_camera.x*RAD, lambda=dl*RAD;
    vec2 globe=vec2(cos(phi)*sin(lambda), -(cos(phi0)*sin(phi)-sin(phi0)*cos(phi)*cos(lambda)));
    vec2 planar=vec2(lambda*max(abs(cos(phi0)),0.25), -(lat-u_camera.x)*RAD);
    vec2 screen=mix(planar,globe,u_camera.w)*u_camera.z;
    gl_Position=vec4(screen.x*2.0/u_view.x,-screen.y*2.0/u_view.y,0.0,1.0);
    v_front=sin(phi0)*sin(phi)+cos(phi0)*cos(phi)*cos(lambda);
}
