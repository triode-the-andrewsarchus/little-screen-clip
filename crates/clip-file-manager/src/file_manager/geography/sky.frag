#version 330 core
in vec2 v_ndc;
out vec4 frag;
uniform mat3 u_inertial_from_fixed;
uniform mat3 u_camera_basis; // East, north, outward in Earth-fixed coordinates.
uniform vec2 u_fov;          // Half extents at unit focal depth.
uniform float u_camera_distance;
uniform float u_opacity;
uniform sampler2D u_stars;
uniform vec3 u_sun;
uniform vec3 u_moon;
uniform vec3 u_planets[7];

const float PI = 3.141592653589793;
const float planet_radius[7] = float[7](2439.7, 6051.8, 3389.5, 69911.0, 58232.0, 25362.0, 24622.0);
const vec3 planet_color[7] = vec3[7](
    vec3(.67,.62,.57), vec3(1,.88,.66), vec3(.92,.46,.31), vec3(.93,.77,.62),
    vec3(.93,.82,.54), vec3(.58,.83,.84), vec3(.44,.58,.89));

float disk(vec3 ray, vec3 offset, float radius, float pixel, out float distance) {
    distance = dot(ray, offset);
    float off_axis = length(cross(ray, offset)) / max(length(offset), 1.0);
    float angular = radius / max(length(offset), 1.0);
    return distance > 0.0 ? 1.0 - smoothstep(max(0.0, angular - pixel*.5), angular + pixel*.5, off_axis) : 0.0;
}

void main() {
    vec3 eye = u_camera_basis[2] * u_camera_distance;
    vec3 ray = u_camera_basis * normalize(vec3(v_ndc * u_fov, -1.0));
    vec3 icrf = u_inertial_from_fixed * ray;
    vec2 uv = vec2(0.5 - atan(icrf.y, icrf.x) / (2.0 * PI), 0.5 - asin(clamp(icrf.z,-1.0,1.0)) / PI);
    vec3 rgb = texture(u_stars, uv).rgb + vec3(.018,.023,.034);
    float pixel = max(length(dFdx(ray)), length(dFdy(ray)));
    float nearest = 1e20;
    float distance;
    // A point marker provides a way to see a planet below pixel resolution.
    // The optical disk keeps the physical angular radius.
    for (int i=0; i<7; ++i) {
        if (u_planets[i] == vec3(0.0)) continue;
        vec3 offset = u_planets[i] - eye;
        float coverage = disk(ray, offset, planet_radius[i], pixel, distance);
        if (distance <= 0.0 || distance >= nearest) continue;
        if (planet_radius[i] / length(offset) < .65 * pixel) {
            float marker = 1.0 - smoothstep(.55, 1.7, length(cross(ray, normalize(offset))) / pixel);
            rgb = mix(rgb, planet_color[i] * .87, .72 * marker);
        }
        if (coverage > 0.0) {
            rgb = mix(rgb, planet_color[i], coverage);
            nearest = distance;
        }
    }
    float sun = disk(ray, u_sun-eye, 695700.0, pixel, distance);
    if (sun > 0.0 && distance < nearest) {
        rgb = mix(rgb, vec3(1.0,.94,.81), sun);
        nearest = distance;
    }
    float moon = disk(ray, u_moon-eye, 1737.4, pixel, distance);
    if (moon > 0.0 && distance < nearest) {
        rgb = mix(rgb, vec3(.58,.57,.55), moon);
    }
    frag = vec4(rgb * u_opacity, u_opacity);
}
