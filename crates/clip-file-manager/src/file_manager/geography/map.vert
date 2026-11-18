#version 330 core
layout(location=0) in vec2 a_uv;
uniform vec4 u_camera; // latitude, longitude, scale in logical pixels, globe blend
uniform vec2 u_view;
uniform vec4 u_tile; // west relative to camera, north, angular side, unused
out vec2 v_uv;
out vec2 v_geo;
out float v_front;
const float RAD=0.017453292519943295;
void main() {
    float dl=u_tile.x+a_uv.x*u_tile.z;
    float lat=u_tile.y-a_uv.y*u_tile.z;
    float phi=lat*RAD, phi0=u_camera.x*RAD, lambda=dl*RAD;
    vec2 globe=vec2(cos(phi)*sin(lambda), -(cos(phi0)*sin(phi)-sin(phi0)*cos(phi)*cos(lambda)));
    vec2 planar=vec2(lambda*max(abs(cos(phi0)),0.25), -(lat-u_camera.x)*RAD);
    vec2 screen=mix(planar,globe,u_camera.w)*u_camera.z;
    gl_Position=vec4(screen.x*2.0/u_view.x,-screen.y*2.0/u_view.y,0.0,1.0);
    v_front=sin(phi0)*sin(phi)+cos(phi0)*cos(phi)*cos(lambda);
    v_uv=(a_uv*256.0+1.0)/258.0;
    v_geo=vec2(dl,lat);
}
