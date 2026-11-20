#version 330 core
in vec2 v_ndc;
out vec4 frag;
uniform mat3 u_inertial_from_fixed;
uniform mat3 u_camera_basis; // East, north, outward in Earth-fixed coordinates.
uniform vec2 u_fov;          // Half extents at unit focal depth.
uniform float u_camera_distance;
uniform float u_opacity;
uniform float u_atmosphere;
uniform sampler2D u_stars;
uniform vec3 u_sun;
uniform vec3 u_moon;
uniform vec3 u_planets[7];
uniform sampler2D u_lunar_albedo;
uniform bool u_has_lunar_albedo;
uniform vec3 u_lunar_sun;
uniform vec3 u_lunar_earth;

const float PI = 3.141592653589793;
const float planet_radius[7] = float[7](2439.7, 6051.8, 3389.5, 69911.0, 58232.0, 25362.0, 24622.0);
const vec3 planet_color[7] = vec3[7](
    vec3(.67,.62,.57), vec3(1,.88,.66), vec3(.92,.46,.31), vec3(.93,.77,.62),
    vec3(.93,.82,.54), vec3(.58,.83,.84), vec3(.44,.58,.89));

float disk(vec3 ray, vec3 offset, float radius, float pixel, out float distance) {
    distance = dot(ray, offset);
    float off_axis = length(cross(ray, offset)) / max(length(offset), 1.0);
    float angular = radius / max(length(offset), 1.0);
    float aperture = min(1.0, PI * (angular / pixel) * (angular / pixel));
    return distance > 0.0 ? aperture * (1.0 - smoothstep(max(0.0, angular - pixel*.5), angular + pixel*.5, off_axis)) : 0.0;
}

vec3 lunar_surface(vec3 ray, vec3 offset) {
    const float radius = 1737.4;
    // The closest point to the sphere center is stable even when the body
    // spans only a few pixels. Find the near intersection for its phase.
    vec3 across = cross(ray, cross(ray, offset));
    float chord = sqrt(max(0.0, radius*radius - dot(across, across)));
    vec3 normal = normalize(across - ray * chord);
    vec3 point = radius * normal;
    vec3 sunlight = u_lunar_sun - point;
    vec3 earth = u_lunar_earth - point;
    vec3 light = normalize(sunlight);
    float diffuse = max(0.0, dot(normal, light));
    float earth_phase = .5 * (1.0 - dot(normalize(earth), light));
    float earthshine = .045 * earth_phase * earth_phase * max(0.0, dot(normal, normalize(earth)));
    Shadow eclipse = shadow(sunlight, earth, 6371.0);
    float direct = (1.0 - .32 * pow(eclipse.covered, 4.0)) *
        (1.0 - smoothstep(.92, 1.0, eclipse.covered));
    vec3 copper = vec3(.25,.063,.024) * exp(-2.3*eclipse.depth)
        * smoothstep(.93, 1.0, eclipse.covered);
    // Synchronous lunar longitude with a mean IAU lunar pole. The apparent
    // Sun vector controls the crescent on this same textured sphere.
    vec3 pole_icrf = vec3(-.0000355, -.39813, .91733);
    vec3 north = normalize(transpose(u_inertial_from_fixed) * pole_icrf);
    vec3 front = normalize(-u_moon - north * dot(-u_moon, north));
    vec3 east = cross(north, front);
    vec2 uv = vec2(.5 + atan(dot(normal,east),dot(normal,front))/(2.0*PI),
        .5 - asin(clamp(dot(normal,north),-1.0,1.0))/PI);
    vec3 albedo = u_has_lunar_albedo ? texture(u_lunar_albedo, uv).rgb : vec3(.6,.59,.57);
    vec3 lighting = vec3(earthshine) + (vec3(direct) + copper) * pow(diffuse,.67);
    return albedo * min(lighting * 1.55, vec3(1.3));
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
        rgb = mix(rgb, lunar_surface(ray, u_moon-eye), moon);
    }
    if (u_atmosphere > 0.0) {
        vec3 tangent = cross(ray, cross(eye, ray));
        float height = length(tangent) - 6378.137;
        if (height >= 0.0 && height < 220.0) {
            float sunlight = dot(normalize(tangent), normalize(u_sun - tangent));
            float optical_depth = exp(-height / 26.0) * (1.0 - smoothstep(115.0, 220.0, height));
            rgb += u_atmosphere * optical_depth * atmosphere_color(sunlight) * .67;
        }
    }
    frag = vec4(rgb * u_opacity, u_opacity);
}
