uniform bool u_perspective;
uniform vec3 u_eye;
uniform mat3 u_view_axes;
uniform vec2 u_half_fov;
uniform float u_near;

const float EARTH_RADIUS_M = 6371000.0;

vec3 sphere_normal(vec2 geo) {
    vec2 angle = radians(geo);
    return vec3(cos(angle.y) * vec2(cos(angle.x), sin(angle.x)), sin(angle.y));
}

vec4 terrain_clip(vec3 normal, float height) {
    vec3 p = transpose(u_view_axes) * (normal * (1.0 + height / EARTH_RADIUS_M) - u_eye);
    float depth = -p.z;
    return vec4(p.xy / u_half_fov, depth - 2.0 * u_near, depth);
}
