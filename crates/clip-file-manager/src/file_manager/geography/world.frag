#version 330 core
in vec2 v_ndc;
out vec4 frag;
uniform mat3 u_axes;
uniform vec2 u_fov;
uniform vec3 u_eye;
uniform vec3 u_color;
uniform float u_zoom;
uniform sampler2D u_albedo;
uniform bool u_has_albedo;
uniform bool u_gas;
uniform int u_band_count;
uniform vec3 u_bands[10];

const float PI = 3.141592653589793;
const vec3 SUN = vec3(.811,.324,.487);

float fringe(float a, float b, float r, float aa) {
    return smoothstep(a-aa,a+aa,r) * (1.0-smoothstep(b-aa,b+aa,r));
}

float grain(vec2 cell) {
    vec2 seed = fract(cell * vec2(.1031,.11369));
    seed += dot(seed, seed.yx + 19.19);
    return fract((seed.x + seed.y) * seed.x);
}

vec4 rings(vec3 ray, float surface_depth) {
    if (u_band_count == 0 || abs(ray.z) < 1e-6) return vec4(0.0);
    float t = -u_eye.z / ray.z;
    if (t <= 0.0 || t >= surface_depth) return vec4(0.0);
    vec2 position = u_eye.xy + t * ray.xy;
    float r = length(position);
    float width = clamp(fwidth(r)*.7, .00006, .09);
    float opacity = 0.0;
    for (int i = 0; i < u_band_count; ++i) {
        vec3 band = u_bands[i];
        float middle = (band.x+band.y)*.5;
        float radius = max((band.y-band.x)*.5, width*.55);
        opacity += band.z * fringe(middle-radius,middle+radius,r,width);
    }
    if (opacity < .001) return vec4(0.0);
    float layer = .76 + .14*sin(r*139.0) + .10*sin(r*311.0);
    vec2 cell = floor(position * 930.0);
    float dust = grain(cell);
    float particles = mix(dust > .70 ? .27 + dust * .73 : .18 + dust * .24,
        .55, smoothstep(.0004,.0018,width));
    vec3 point = vec3(position,0.0);
    float behind = -dot(SUN, point);
    float shade = behind > 0.0 && length(point + behind * SUN) < 1.0 ? .27 : 1.0;
    float alpha = clamp(layer * opacity * (particles * 1.42) / max(.50, abs(ray.z)), 0.0, .88);
    return vec4(mix(vec3(.74,.75,.77),u_color,.43)*shade * alpha,alpha);
}

void main() {
    vec3 ray = u_axes * normalize(vec3(v_ndc * u_fov, -1.0));
    float line = -dot(ray,u_eye);
    vec3 across = u_eye + ray * line;
    float margin = 1.0-dot(across,across);
    float aa = max(fwidth(margin), .000001);
    float body_coverage = line > 0.0 ? smoothstep(-aa*.56, aa*.56, margin) : 0.0;
    vec4 body = vec4(0.0);
    float depth = line - sqrt(max(0.0,margin));
    if (body_coverage > 0.0 && line > 0.0) {
        vec3 normal = normalize(across - ray*sqrt(max(0.0,margin)));
        float lat = asin(clamp(normal.z,-1.0,1.0));
        float lon = atan(normal.y,normal.x);
        vec2 uv = vec2(.5 + lon / (2.0*PI),.5 - lat / PI);
        vec3 albedo = u_color;
        if (u_has_albedo) {
            vec2 dx = dFdx(uv);
            vec2 dy = dFdy(uv);
            dx.x -= round(dx.x);
            dy.x -= round(dy.x);
            albedo = textureGrad(u_albedo,uv,dx,dy).rgb;
        } else if (u_gas) {
            albedo *= .93 + .10*sin(lat*27.0) + .035*sin(lat*73.5);
        }
        float phase = 1.0-smoothstep(2.5,30.0,u_zoom);
        float light = .25 + .75*pow(max(dot(normal,SUN),0.0),.54);
        body = vec4(albedo*mix(1.0,light,phase)*body_coverage,body_coverage);
    }
    vec4 front = rings(ray,body_coverage > .5 && depth > 0.0 ? depth : 1e19);
    frag = front + body*(1.0-front.a);
}
