uniform float feather;
flat in vec3 thread_rgb;
flat in float segment_length;
in vec2 along_across;
out vec4 pixel;

void main() {
    vec2 closest = vec2(clamp(along_across.x, 0.0, segment_length), 0.0);
    float distance_to_stitch = distance(along_across, closest);
    // Premultiplied coverage lets overlapping stitches retain their thread color.
    float coverage = clamp((0.65 - distance_to_stitch) / feather + 0.5, 0.0, 1.0);
    pixel = vec4(thread_rgb * coverage, coverage);
}
