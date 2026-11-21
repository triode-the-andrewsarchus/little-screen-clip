#version 330 core
in vec2 v_ndc;
out vec4 frag;
uniform mat3 u_inertial_from_fixed;
uniform mat3 u_camera_basis; // East, north, outward in Earth-fixed coordinates.
uniform vec2 u_fov;          // Half extents at unit focal depth.
uniform float u_camera_distance;
uniform bool u_sky_eye_override;
uniform vec3 u_sky_eye;
uniform bool u_moon_view;
uniform bool u_moon_only;
uniform vec3 u_moon_eye;
uniform sampler2D u_earth_west;
uniform sampler2D u_earth_east;
uniform sampler2D u_earth_night;
uniform ivec2 u_earth_tiles;
uniform bool u_light_earth_disk;
uniform bool u_earth_night_available;
uniform float u_opacity;
uniform float u_atmosphere;
uniform float u_star_trail; // Offset in right ascension during a clock transition.
uniform bool u_readable_sky;
uniform float u_readable_disk_zoom;
uniform sampler2D u_stars;
uniform vec3 u_sun;
uniform vec3 u_moon;
uniform vec3 u_planets[7];

#include <lunar>

const float PI = 3.141592653589793;
const float EARTH_RADIUS_KM = 6378.137;
const float READABLE_DISK_SCALE = 16.0;
const float planet_radius[7] = float[7](2439.7, 6051.8, 3389.5, 69911.0, 58232.0, 25362.0, 24622.0);
const vec3 planet_color[7] = vec3[7](
    vec3(.67,.62,.57), vec3(1,.88,.66), vec3(.92,.46,.31), vec3(.93,.77,.62),
    vec3(.93,.82,.54), vec3(.58,.83,.84), vec3(.44,.58,.89));
const float planet_emphasis[7] = float[7](.60, 1.0, .72, 1.0, .75, .40, .40);

// Leave objects at their physical size as they cross Earth's limb.
float limb_clearance(vec3 eye, vec3 toward_body) {
    return length(eye + toward_body * max(0.0, -dot(eye, toward_body))) - EARTH_RADIUS_KM;
}

float disk_enlargement(vec3 sun, vec3 moon) {
    vec3 a = normalize(sun), b = normalize(moon);
    float separation = atan(length(cross(a,b)), dot(a,b));
    float radii = SOLAR_RADIUS_KM / length(sun) + MOON_RADIUS_KM / length(moon);
    // Ease back to physical sizes before any of the enlarged disks could overlap.
    return smoothstep(READABLE_DISK_SCALE * radii, (READABLE_DISK_SCALE + 1.5) * radii, separation);
}

float disk(vec3 ray, vec3 offset, float radius, float pixel, out float distance) {
    distance = dot(ray, offset);
    float off_axis = length(cross(ray, offset)) / max(length(offset), 1.0);
    float angular = radius / max(length(offset), 1.0);
    float aperture = min(1.0, PI * (angular / pixel) * (angular / pixel));
    return distance > 0.0 ? aperture * (1.0 - smoothstep(max(0.0, angular - pixel*.5), angular + pixel*.5, off_axis)) : 0.0;
}

vec3 lunar_surface(vec3 ray, vec3 offset) {
    vec3 across = cross(ray, cross(ray, offset));
    float chord = sqrt(max(0.0, MOON_RADIUS_KM * MOON_RADIUS_KM - dot(across, across)));
    vec3 normal = normalize(across - ray * chord);
    return lunar_color(normal, normal, 1.0, 0.0);
}
vec3 earth_surface(vec3 ray, vec3 offset) {
    vec3 across = cross(ray, cross(ray, offset));
    float chord = sqrt(max(0.0, EARTH_RADIUS_KM * EARTH_RADIUS_KM - dot(across, across)));
    vec3 normal = normalize(across - ray * chord);
    vec2 uv = vec2(fract(.5 + atan(normal.y, normal.x) / (2.0 * PI)),
        .5 - asin(clamp(normal.z, -1.0, 1.0)) / PI);
    vec2 tile_uv = (vec2(fract(uv.x * 2.0), uv.y) * 256.0 + 1.0) / 258.0;
    vec3 ocean = vec3(.15, .32, .52);
    vec4 sample_rgb = vec4(ocean, 0.0);
    if (uv.x < .5 && u_earth_tiles.x != 0) sample_rgb = texture(u_earth_west, tile_uv);
    if (uv.x >= .5 && u_earth_tiles.y != 0) sample_rgb = texture(u_earth_east, tile_uv);
    vec3 surface = mix(ocean, sample_rgb.rgb, sample_rgb.a);
    if (!u_light_earth_disk) return surface;
    float sunlight = dot(normal, normalize(u_sun - normal * EARTH_RADIUS_KM));
    float day = smoothstep(-.10, .08, sunlight);
    vec3 color = mix(surface * vec3(.15, .19, .28), surface, day);
    if (u_earth_night_available) {
        vec3 observed = max(textureLod(u_earth_night, uv, 2.0).rgb - vec3(.10, .11, .15), 0.0);
        float lights = smoothstep(.08, .62, dot(observed, vec3(.5, .4, .1)));
        color = mix(color, vec3(1.0, .80, .46), lights * (1.0 - day));
    }
    float rim = 1.0 - smoothstep(0.0, .23, max(0.0, dot(normal, -ray)));
    return mix(color, atmosphere_color(sunlight), .38 * rim);
}

void main() {
    vec3 eye = u_sky_eye_override ? u_sky_eye : u_camera_basis[2] * u_camera_distance;
    vec3 ray = u_camera_basis * normalize(vec3(v_ndc * u_fov, -1.0));
    if (u_moon_only) {
        vec3 across = cross(ray, cross(u_moon_eye, ray));
        float margin = 1.0 - dot(across, across);
        float feather = max(fwidth(margin), .000001);
        float coverage = smoothstep(-feather * .55, feather * .55, margin);
        if (dot(u_moon_eye, ray) >= 0.0 || coverage == 0.0) discard;
        vec3 normal = normalize(across - ray * sqrt(max(0.0, margin)));
        frag = vec4(lunar_color(normal, normal, u_moon_phase, .22) * coverage, coverage);
        return;
    }
    vec3 icrf = u_inertial_from_fixed * ray;
    vec2 uv = vec2(0.5 - atan(icrf.y, icrf.x) / (2.0 * PI), 0.5 - asin(clamp(icrf.z,-1.0,1.0)) / PI);
    vec3 rgb = texture(u_stars, uv).rgb + vec3(.018,.023,.034);
    if (abs(u_star_trail) > .0001) {
        for (int i = 1; i <= 3; ++i) {
            float t = float(i) / 3.0;
            vec3 bright = max(texture(u_stars, uv + vec2(u_star_trail * t, 0.0)).rgb - .35, vec3(0.0));
            rgb = max(rgb, bright * mix(.34, .12, t));
        }
    }
    float pixel = max(length(dFdx(ray)), length(dFdy(ray)));
    float nearest = 1e20;
    float distance;
    // A point marker provides a way to see a planet below pixel resolution.
    for (int i=0; i<7; ++i) {
        if (u_planets[i] == vec3(0.0)) continue;
        vec3 offset = u_planets[i] - eye;
        vec3 direction = normalize(offset);
        float marker_growth = u_readable_sky ? smoothstep(0.0, 250.0, limb_clearance(eye, direction)) : 0.0;
        float radius = planet_radius[i] * mix(1.0, 4.0, marker_growth);
        float coverage = disk(ray, offset, radius, pixel, distance);
        if (distance <= 0.0 || distance >= nearest) continue;
        if (planet_radius[i] / length(offset) < .65 * pixel) {
            float point = length(cross(ray, direction)) / pixel;
            float emphasis = planet_emphasis[i] * marker_growth;
            point /= mix(1.0, 8.0, marker_growth);
            float marker = 1.0 - smoothstep(.55 + .45 * emphasis, 1.7 + 1.4 * emphasis, point);
            float halo = 1.0 - smoothstep(1.1, 5.5, point);
            rgb += planet_color[i] * (.22 * emphasis * halo);
            rgb = mix(rgb, planet_color[i] * (.87 + .13 * emphasis), (.72 + .23 * emphasis) * marker);
        }
        if (coverage > 0.0) {
            rgb = mix(rgb, planet_color[i], coverage);
            nearest = distance;
        }
    }
    vec3 sun_offset = u_sun - eye, moon_offset = u_moon - eye;
    vec3 sun_direction = normalize(sun_offset), moon_direction = normalize(moon_offset);
    float enlargement = u_readable_sky ? (u_moon_view ? 1.0 : disk_enlargement(sun_offset, moon_offset)) : 0.0;
    float sun_growth = enlargement * smoothstep(0.0, 600.0, limb_clearance(eye, sun_direction));
    float moon_growth = enlargement * smoothstep(0.0, 600.0, limb_clearance(eye, moon_direction));
    float sun_scale = mix(1.0, READABLE_DISK_SCALE, sun_growth * u_readable_disk_zoom);
    float moon_scale = mix(1.0, READABLE_DISK_SCALE, moon_growth * u_readable_disk_zoom);
    float sun = disk(ray, sun_offset, SOLAR_RADIUS_KM * sun_scale, pixel, distance);
    if (distance > 0.0 && sun_scale > 1.0) {
        float disk_radii = length(cross(ray, sun_direction)) / (SOLAR_RADIUS_KM * sun_scale / length(sun_offset));
        float glow = 1.0 - smoothstep(1.0, 2.8, disk_radii);
        rgb += vec3(1.0,.83,.55) * (.16 * sun_growth * glow);
    }
    if (sun > 0.0 && distance < nearest) {
        rgb = mix(rgb, vec3(1.0,.94,.81), sun);
        nearest = distance;
    }
    float moon = disk(ray, moon_offset, MOON_RADIUS_KM * moon_scale, pixel, distance);
    if (!u_moon_view && moon > 0.0 && distance < nearest) {
        vec3 surface_ray = normalize(mix(moon_direction, ray, 1.0 / moon_scale));
        rgb = mix(rgb, lunar_surface(surface_ray, moon_offset), moon);
    }
    if (u_moon_view) {
        float earth = disk(ray, -eye, EARTH_RADIUS_KM, pixel, distance);
        if (earth > 0.0 && distance < nearest) {
            rgb = mix(rgb, earth_surface(ray, -eye), earth);
        }
    }
    if (u_atmosphere > 0.0) {
        vec3 tangent = cross(ray, cross(eye, ray));
        float height = length(tangent) - EARTH_RADIUS_KM;
        if (height >= 0.0 && height < 260.0) {
            float sunlight = dot(normalize(tangent), normalize(u_sun - tangent));
            // A soft falloff keeps the rim legible at globe scale.
            float optical_depth = exp(-height / 64.0) * (1.0 - smoothstep(125.0, 260.0, height));
            vec3 scatter = atmosphere_color(sunlight);
            float glow = u_atmosphere * optical_depth;
            rgb = mix(rgb, scatter, glow * max(scatter.r, scatter.b) * .55) + glow * scatter * .72;
        }
    }
    frag = vec4(rgb * u_opacity, u_opacity);
}
