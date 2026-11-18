#version 330 core
layout(location=0) in vec4 a_points;
layout(location=1) in vec2 a_other;
uniform vec4 u_camera;
uniform vec2 u_view;
uniform float u_width;
uniform float u_shift;
out vec2 v_geo;
out float v_front;
out float v_edge;
out float v_along;
const float RAD=0.017453292519943295;
vec3 project(vec2 geo) {
    float phi=geo.y*RAD, phi0=u_camera.x*RAD, dl=geo.x*RAD;
    vec2 globe=vec2(cos(phi)*sin(dl),-(cos(phi0)*sin(phi)-sin(phi0)*cos(phi)*cos(dl)));
    vec2 planar=vec2(dl*max(abs(cos(phi0)),0.25),-(geo.y-u_camera.x)*RAD);
    return vec3(mix(planar,globe,u_camera.w)*u_camera.z,sin(phi0)*sin(phi)+cos(phi0)*cos(phi)*cos(dl));
}
float wrap(float lon) { return mod(lon+180.0,360.0)-180.0; }
void main() {
    vec2 a=a_points.xy, b=a_points.zw;
    float dx=wrap(b.x-a.x);
    float middle=a.x+dx*0.5;
    float start=wrap(middle-u_camera.y)-dx*0.5+u_shift;
    a.x=start; b.x=start+dx;
    vec2 geo;
    vec2 screen;
    if(u_width==0.0) {
        vec2 c=a_other;
        c.x=start+wrap(c.x-a_points.x);
        geo=gl_VertexID==0?a:(gl_VertexID==1?b:c);
        screen=project(geo).xy;
        v_edge=0.0; v_along=0.0;
    } else {
        vec2 pa=project(a).xy, pb=project(b).xy;
        vec2 direction=pb-pa;
        float len=max(length(direction),0.001);
        vec2 normal=vec2(-direction.y,direction.x)/len;
        bool end_point=gl_VertexID==1 || gl_VertexID==2 || gl_VertexID==4;
        float side=(gl_VertexID==0 || gl_VertexID==1 || gl_VertexID==5)?-1.0:1.0;
        geo=end_point?b:a;
        float width=u_width<0.0?clamp(a_other.x/6371000.0*u_camera.z/max(abs(cos(radians(geo.y))),0.15),0.8,48.0):u_width;
        screen=(end_point?pb:pa)+normal*side*(width*0.5+0.6);
        v_edge=side; v_along=end_point?len:0.0;
    }
    v_geo=geo;
    v_front=project(geo).z;
    gl_Position=vec4(screen.x*2.0/u_view.x,-screen.y*2.0/u_view.y,0.0,1.0);
}
