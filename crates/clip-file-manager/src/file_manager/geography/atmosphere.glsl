// Blue daylight scattering gives way to a warm terminator and faint night airglow.
vec3 atmosphere_color(float sun_height) {
    float daylight = smoothstep(-.16, .30, sun_height);
    float twilight = smoothstep(-.24, -.025, sun_height) * (1.0-daylight);
    float night = 1.0 - daylight - twilight;
    return daylight * vec3(.24,.48,.91) + twilight * vec3(.98,.40,.17)
        + night * vec3(.014,.031,.075);
}
