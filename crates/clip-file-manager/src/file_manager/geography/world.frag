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

float grain(vec2 cell) {
    vec2 seed = fract(cell * vec2(.1031,.11369));
    seed += dot(seed, seed.yx + 19.19);
    return fract((seed.x + seed.y) * seed.x);
}

float ring_dust(vec2 p) {
    vec2 cell = floor(p);
    vec2 f = fract(p);
    f = f*f*(3.0-2.0*f);
    return mix(mix(grain(cell),grain(cell+vec2(1,0)),f.x),
               mix(grain(cell+vec2(0,1)),grain(cell+1.0),f.x),f.y);
}

vec2 cylinder_span(vec2 eye, vec2 ray, float radius) {
    float speed = dot(ray,ray);
    if (speed < 1e-8)
        return dot(eye,eye) < radius*radius ? vec2(-1e9,1e9) : vec2(1,-1);
    float mid = -dot(eye,ray) / speed;
    vec2 offset = eye + mid*ray;
    float delta = (radius*radius - dot(offset,offset))/speed;
    if (delta < 0.0) return vec2(1,-1);
    return mid + sqrt(delta)*vec2(-1,1);
}

vec4 rings(vec3 ray, float surface_depth) {
    if (u_band_count == 0) return vec4(0);
    // A thin disk with finite vertical depth stays visible from its equator.
    // All distances are measured in planetary radii.
    float outer = u_bands[u_band_count-1].y;
    float height = clamp(outer*.0032,.004,.011);
    vec2 slab = vec2(0,surface_depth);
    if (abs(ray.z) < 1e-6) {
        if (abs(u_eye.z) >= height) return vec4(0);
    } else {
        vec2 crossings = (vec2(-height,height)-u_eye.z)/ray.z;
        slab = vec2(max(0.0,min(crossings.x,crossings.y)),
                    min(surface_depth,max(crossings.x,crossings.y)));
    }
    if (slab.y <= slab.x) return vec4(0);

    float pixel_angle = max(u_fov.x,u_fov.y) * fwidth(v_ndc.x);
    float thickness = 0.0;
    float shaded = 0.0;
    for (int i = 0; i < u_band_count; ++i) {
        vec3 band = u_bands[i];
        float feather = clamp(pixel_angle*max(slab.x,.05)*.55,.00006,.05);
        float width = band.y-band.x;
        float edge = feather*.65;
        vec2 span = cylinder_span(u_eye.xy,ray.xy,band.y+edge);
        span = vec2(max(span.x,slab.x),min(span.y,slab.y));
        if (span.y <= span.x) continue;
        vec2 hole = cylinder_span(u_eye.xy,ray.xy,max(0.0,band.x-edge));
        vec2 near_span = vec2(span.x, min(span.y, hole.x));
        vec2 far_span = vec2(max(span.x, hole.y),span.y);
        if (hole.y < hole.x) {
            near_span = span;
            far_span = vec2(0);
        }
        // Integrate the two sides of the annulus after clipping to the globe.
        for (int side = 0; side < 2; ++side) {
            vec2 section = side == 0 ? near_span : far_span;
            float length_in_band = section.y-section.x;
            if (length_in_band <= 0.0) continue;
            float t = (section.x+section.y)*.5;
            vec3 point = u_eye+t*ray;
            float r = length(point.xy);
            float coverage = smoothstep(band.x-edge,band.x+edge,r)
                           * (1.0-smoothstep(band.y-edge,band.y+edge,r));
            // For bands smaller than a pixel, preserve their integrated opacity.
            coverage = max(coverage, min(.50,width/(width+2.0*edge)));
            float footprint = pixel_angle*max(t,.01);
            float noise_blend = 1.0-smoothstep(.001,.007,footprint+length_in_band*.06);
            float dust = mix(.56, .19 + .93*ring_dust(point.xy*410.0), noise_blend);
            float profile = mix(1.0,.80+.20*sin(r*132.0),
                                1.0-smoothstep(.004,.023,footprint));
            float depth = band.z * coverage * dust * profile
                        * min(length_in_band/(2.0*height),4.2);
            float behind = -dot(SUN,point);
            float shadow = behind > 0.0 ? smoothstep(.94,1.04,length(point+behind*SUN)) : 1.0;
            thickness += depth;
            shaded += depth * mix(.24,1.0,shadow);
        }
    }
    float alpha = 1.0-exp(-thickness);
    vec3 color = mix(vec3(.74,.75,.77),u_color,.43)*shaded/max(thickness,1e-6);
    return vec4(color*alpha,alpha);
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
