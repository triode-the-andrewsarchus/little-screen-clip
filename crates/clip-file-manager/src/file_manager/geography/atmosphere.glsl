// A thin cartographic atmosphere. Blue scattering on the day side gives way
// to warmer light along the terminator and drops off into the night.
vec3 atmosphere_color(float sun_height) {
    float daylight = smoothstep(-.16, .30, sun_height);
    float twilight = smoothstep(-.24, -.025, sun_height) * (1.0-daylight);
    return daylight * vec3(.20,.43,.84) + twilight * vec3(.75,.29,.13);
}
