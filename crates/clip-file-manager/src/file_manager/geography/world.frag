#version 330 core
in vec2 v_ndc;
out vec4 frag;
uniform mat3 u_axes;
uniform vec2 u_fov;
uniform vec4 u_ray_region = vec4(1.0,1.0,0.0,0.0);
uniform vec3 u_eye;
uniform vec3 u_color;
uniform float u_zoom;
uniform sampler2D u_albedo;
uniform bool u_has_albedo;
uniform bool u_gas;
uniform vec4 u_air_optics;
uniform vec3 u_air_tint;
uniform bool u_venus;
uniform bool u_uranus;
uniform bool u_neptune;
uniform int u_band_count;
uniform float u_ring_emphasis;
uniform vec3 u_bands[10];

const float PI = 3.141592653589793;
uniform vec3 u_sun_direction;
uniform float u_sun_radius;
uniform vec2 u_solar_clock;
uniform bool u_emissive;
uniform bool u_background_only;
uniform float u_sky_brightness;
uniform bool u_has_stars;
uniform sampler2D u_stars;

#include <celestial>

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

vec4 rings(vec3 ray, float surface_depth, float pixel_angle) {
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
            float behind = -dot(u_sun_direction,point);
            float shadow = behind > 0.0 ? smoothstep(.94,1.04,length(point+behind*u_sun_direction)) : 1.0;
            thickness += depth;
            shaded += depth * mix(.24,1.0,shadow);
        }
    }
    float alpha = 1.0-exp(-thickness*max(1.0,u_ring_emphasis));
    vec3 color = mix(vec3(.74,.75,.77),u_color,.43)*shaded/max(thickness,1e-6);
    return vec4(color*alpha,alpha);
}

vec4 atmospheric_light(vec3 ray, vec3 across, float line, float body_depth) {
    float height = u_air_optics.x;
    if (height <= 0.0 || line < 0.0) return vec4(0);
    float shell = 1.0 + 6.0*height;
    float radial2 = dot(across,across);
    if (radial2 >= shell*shell) return vec4(0);
    float half_chord = sqrt(shell*shell-radial2);
    // Integrate relative to closest approach to avoid cancellation when the
    // observer is far away in units of the body's own radius.
    float near = max(-line,-half_chord);
    float far = min(body_depth,half_chord);
    float step_size = (far-near)/6.0;
    if (step_size <= 0.0) return vec4(0);
    float column = 0.0, lit = 0.0;
    if (radial2 < .88 && body_depth < 0.0) {
        // Away from the tangent the layer is locally plane parallel.
        column = 1.0;
        step_size = height / sqrt(1.0-radial2);
        vec3 surface = across+ray*body_depth;
        lit = smoothstep(-.035,.022,dot(surface,u_sun_direction));
    } else {
        for (int i = 0; i < 6; ++i) {
            vec3 p = across + ray*(near+(float(i)+.5)*step_size);
            float r2 = dot(p,p);
            float density = exp(-max(0.0,sqrt(r2)-1.0)/height);
            float sun_height = dot(p,u_sun_direction);
            float daylight = smoothstep(-.025,.018,sun_height+sqrt(max(0.0,r2-1.0)));
            column += density;
            lit += density*daylight;
        }
    }
    float alpha = 1.0-exp(-column*step_size*u_air_optics.y/height);
    float angle = dot(ray,u_sun_direction);
    float g = u_air_optics.z;
    float forward = pow((1.0-g)*(1.0-g)/(1.0+g*g-2.0*g*angle), 1.5);
    vec3 color = mix(u_air_tint,vec3(.46,.67,1.0),u_air_optics.w*forward);
    float scatter = .28 + .19*angle*angle + 1.25*forward;
    float exposure = mix(1.0,.42,smoothstep(9.0,32.0,u_zoom));
    return vec4(color*alpha*scatter*(lit/max(column,1e-5)),alpha) * exposure;
}

void main() {
    vec2 ndc = v_ndc*u_ray_region.xy+u_ray_region.zw;
    float pixel_angle = max(u_fov.x*fwidth(ndc.x),u_fov.y*fwidth(ndc.y));
    vec3 ray = u_axes * normalize(vec3(ndc * u_fov, -1.0));
    if (u_background_only) {
        // Ecliptic coordinates -> J2000 equatorial coordinates.
        vec3 equatorial = vec3(ray.x,.917482*ray.y-.397777*ray.z,
                                      .397777*ray.y+.917482*ray.z);
        vec3 stars = u_has_stars ? star_color(u_stars,equatorial) : vec3(0);
        vec3 sky = vec3(.012,.017,.028) + u_sky_brightness*stars;
        vec4 sun = solar_disk(ray,u_sun_direction,u_sun_radius,u_solar_clock);
        frag = u_has_stars ? vec4(sun.rgb + sky*(1.0-sun.a),1.0) : sun;
        return;
    }
    if (u_emissive) {
        frag = solar_disk(ray,normalize(-u_eye),1.0/length(u_eye),u_solar_clock);
        return;
    }
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
        if (u_venus) {
            // The installed radar map describes the ground below the opaque
            // clouds. Only a low-contrast suggestion of cloud structure is
            // visible from orbit in reflected visible light.
            float clouds = .5 + .16*sin(lat*24.0+sin(lon*2.0+lat*5.0));
            albedo = vec3(.92,.85,.70) * (.91+.13*clouds);
        }
        if (u_uranus && !u_has_albedo)
            albedo = vec3(.66,.80,.80) * (1.0+.055*sin(lat*13.0));
        if (u_neptune && u_has_albedo) {
            float luminance = dot(albedo,vec3(.25,.55,.20));
            albedo = mix(albedo,vec3(luminance)*vec3(.87,1.02,1.10),.62);
        }
        float phase = 1.0-smoothstep(2.5,30.0,u_zoom);
        float light = .25 + .75*pow(max(dot(normal,u_sun_direction),0.0),.54);
        body = vec4(albedo*mix(1.0,light,phase)*body_coverage,body_coverage);
    }
    bool opaque = body_coverage > .5 && depth > 0.0;
    vec4 air = atmospheric_light(ray,across,line,opaque ? -sqrt(max(0.0,margin)) : 1e19);
    vec4 globe = air + body*(1.0-air.a);
    vec4 front = rings(ray,opaque ? depth : 1e19,pixel_angle);
    frag = front + globe*(1.0-front.a);
}
