#version 330 core
uniform float u_opacity;
out vec4 frag;
void main() {
    float alpha = .55 * u_opacity;
    frag = vec4(vec3(.44,.68,.89) * alpha, alpha);
}
