layout(location = 0) in vec4 endpoints;
layout(location = 1) in vec4 color;
uniform vec2 to_clip;
uniform float feather;

flat out vec3 thread_rgb;
flat out float segment_length;
out vec2 along_across;

void main() {
    vec2 delta = endpoints.zw - endpoints.xy;
    segment_length = length(delta);
    vec2 tangent = segment_length > 0.0 ? delta / segment_length : vec2(1.0, 0.0);
    vec2 normal = vec2(-tangent.y, tangent.x);

    // Enclose the round caps and the one-pixel antialiasing fringe.
    float pad = 0.65 + 0.5 * feather;
    along_across = vec2(
        (gl_VertexID & 1) == 0 ? -pad : segment_length + pad,
        (gl_VertexID & 2) == 0 ? -pad : pad
    );
    vec2 point = endpoints.xy + tangent * along_across.x + normal * along_across.y;
    gl_Position = vec4(point * to_clip + vec2(-1.0, 1.0), 0.0, 1.0);
    thread_rgb = color.rgb;
}
