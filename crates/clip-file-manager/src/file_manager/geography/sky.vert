#version 330 core
out vec2 v_ndc;
void main() {
    v_ndc = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2) * 2.0 - 1.0;
    gl_Position = vec4(v_ndc, 0.0, 1.0);
}
