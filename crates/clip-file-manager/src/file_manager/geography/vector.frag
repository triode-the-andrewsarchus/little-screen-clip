#version 330 core
in vec2 v_geo;
in float v_front;
in float v_edge;
in float v_along;
uniform float u_blend;
uniform bool u_perspective;
uniform vec4 u_color;
uniform bool u_dashed;
uniform bool u_fallback_road;
uniform bool u_detailed_road;
uniform float u_night_dimming;
uniform float u_earth_lighting;
uniform float u_road_night;
uniform int u_covered_count;
uniform ivec2 u_covered[64];
uniform float u_detail_span;
uniform float u_longitude;
uniform sampler2D u_night;
uniform vec3 u_sun;
out vec4 frag;
void main() {
    if(u_perspective ? v_front < -.12 :
       ((u_blend>0.0 && abs(v_geo.x)>180.0) || (u_blend>0.2 && v_front < -0.02*(1.0-u_blend)))) discard;
    float lon = mod(v_geo.x + u_longitude + 180.0, 360.0);
    if (u_fallback_road && u_covered_count > 0) {
        ivec2 cell = ivec2(floor(vec2(lon, 90.0-v_geo.y) / u_detail_span));
        for (int i=0; i<u_covered_count; ++i) {
            if (cell == u_covered[i]) discard;
        }
    }
    if(u_dashed && mod(v_along,9.0)>5.0) discard;
    float coverage=1.0-smoothstep(1.0-max(fwidth(v_edge),0.001),1.0,abs(v_edge));
    float alpha=u_color.a*coverage;
    vec3 color = u_color.rgb;
    if ((u_night_dimming > 0.0 && u_earth_lighting > 0.0) || (u_detailed_road && u_road_night > 0.0)) {
        vec2 angle = radians(vec2(lon-180.0, v_geo.y));
        vec3 normal = vec3(cos(angle.y) * vec2(cos(angle.x),sin(angle.x)),sin(angle.y));
        float elevation = dot(normal,normalize(u_sun));
        if (u_night_dimming > 0.0) {
            float darkness = 1.0 - smoothstep(-.105, .0349, elevation);
            alpha *= 1.0 - u_night_dimming * darkness * u_earth_lighting;
        } else {
            float night = 1.0 - smoothstep(-.105, 0.0, elevation);
            vec3 observed = max(vec3(0.0), texture(u_night,vec2(lon/360.0, .5-v_geo.y/180.0)).rgb-vec3(.10,.11,.15));
            float intensity = smoothstep(.02,.45,dot(observed,vec3(.5,.4,.1))) * night * u_road_night;
            color = mix(color, vec3(1.0,.78,.45), .34 * intensity);
            alpha = min(coverage,alpha*(1.0+.08*intensity));
        }
    }
    frag=vec4(color*alpha,alpha);
}
