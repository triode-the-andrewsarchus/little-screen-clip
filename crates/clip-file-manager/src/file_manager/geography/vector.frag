#version 330 core
in vec2 v_geo;
in float v_front;
in float v_edge;
in float v_along;
uniform float u_blend;
uniform vec4 u_color;
uniform bool u_dashed;
out vec4 frag;
void main() {
    if(abs(v_geo.x)>180.0 || (u_blend>0.2 && v_front < -0.02*(1.0-u_blend))) discard;
    if(u_dashed && mod(v_along,9.0)>5.0) discard;
    float coverage=1.0-smoothstep(1.0-max(fwidth(v_edge),0.001),1.0,abs(v_edge));
    float alpha=u_color.a*coverage;
    frag=vec4(u_color.rgb*alpha,alpha);
}
