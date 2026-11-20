#version 330 core
layout(location=0) in vec3 a_inertial;
uniform mat3 u_camera_inertial;
uniform vec2 u_fov;
void main() {
    // Invert the sky's ray camera. Directional chords project to the same
    // lines as great-circle arcs, and the frustum clips them at the viewport.
    vec3 camera = transpose(u_camera_inertial) * a_inertial;
    gl_Position = vec4(camera.xy / u_fov, 0.0, -camera.z);
}
