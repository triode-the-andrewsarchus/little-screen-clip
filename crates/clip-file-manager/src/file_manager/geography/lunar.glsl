uniform sampler2D u_lunar_albedo;
uniform bool u_has_lunar_albedo;
uniform vec3 u_lunar_sun;
uniform vec3 u_lunar_earth;
uniform mat3 u_lunar_basis;
uniform float u_moon_phase;
uniform float u_timeline_light = 1.0;

const float LUNAR_PI = 3.141592653589793;
const float MOON_RADIUS_KM = 1737.4;

vec3 lunar_color(vec3 normal, vec3 relief_normal, float phase, float minimum_light) {
    vec3 point = MOON_RADIUS_KM * normal;
    vec3 sunlight = u_lunar_sun - point;
    vec3 earth = u_lunar_earth - point;
    vec3 light = normalize(sunlight);
    float diffuse = max(0.0, dot(relief_normal, light));
    float earth_phase = .5 * (1.0 - dot(normalize(earth), light));
    float earthshine = .045 * earth_phase * earth_phase * max(0.0, dot(normal, normalize(earth)));
    Shadow eclipse = shadow(sunlight, earth, 6371.0);
    float direct = (1.0 - .32 * pow(eclipse.covered, 4.0)) *
        (1.0 - smoothstep(.92, 1.0, eclipse.covered));
    vec3 copper = vec3(.25,.063,.024) * exp(-2.3*eclipse.depth)
        * smoothstep(.93, 1.0, eclipse.covered);
    vec3 local = transpose(u_lunar_basis) * normal;
    vec2 uv = vec2(.5 + atan(local.y, local.x) / (2.0 * LUNAR_PI),
        .5 - asin(clamp(local.z, -1.0, 1.0)) / LUNAR_PI);
    vec3 albedo = u_has_lunar_albedo ? texture(u_lunar_albedo, uv).rgb : vec3(.6,.59,.57);
    vec3 lighting = vec3(earthshine) + (vec3(direct) + copper) * pow(diffuse,.67);
    vec3 daylight = mix(vec3(.66), clamp(lighting * 1.55, minimum_light, 1.3), u_timeline_light);
    return albedo * mix(vec3(1.0), daylight, phase);
}
