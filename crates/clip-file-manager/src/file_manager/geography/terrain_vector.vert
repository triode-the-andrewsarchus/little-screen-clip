#version 330 core
layout(location=0) in vec4 a_points;
layout(location=1) in vec2 a_other;
out vec4 v_points;
out vec2 v_other;
flat out int v_chunk;

void main() {
    gl_Position = vec4(0.0, 0.0, 0.0, 1.0);
    v_points = a_points;
    v_other = a_other;
    v_chunk = gl_VertexID;
}
