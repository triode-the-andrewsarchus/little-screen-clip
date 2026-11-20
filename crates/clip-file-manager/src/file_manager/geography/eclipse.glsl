// Angular disks at the receiving surface point. Their overlap is the fraction
// of direct sunlight blocked. The planar angular-area approximation is accurate
// for the small apparent disks involved here.
struct Shadow {
    float covered;
    float depth;
};
Shadow shadow(vec3 sun, vec3 blocker, float blocker_radius) {
    const float solar_radius = 695700.0;
    float a = asin(solar_radius / length(sun));
    float b = asin(blocker_radius / length(blocker));
    float d = atan(length(cross(normalize(sun), normalize(blocker))), dot(normalize(sun), normalize(blocker)));
    if (d >= a + b) return Shadow(0.0, 0.0);
    if (d <= abs(a - b)) {
        return b >= a ? Shadow(1.0, max(0.0, (b - a - d) / a))
                      : Shadow((b / a) * (b / a), 0.0);
    }
    // Normalize by the solar radius for numerical stability near grazing contact.
    b /= a;
    d /= a;
    float x = clamp((d*d + 1.0 - b*b) / (2.0*d), -1.0, 1.0);
    float y = clamp((d*d + b*b - 1.0) / (2.0*d*b), -1.0, 1.0);
    float overlap = acos(x) - x * sqrt(max(0.0, 1.0-x*x))
        + b*b*(acos(y) - y * sqrt(max(0.0, 1.0-y*y)));
    return Shadow(clamp(overlap / 3.141592653589793, 0.0, 1.0), 0.0);
}
