#version 330 core
in vec2 v_uv;
in vec2 v_geo;
in float v_front;
out vec4 frag;
uniform sampler2D u_color;
uniform sampler2D u_height;
uniform sampler2D u_water;
uniform int u_style;
uniform bool u_show_water;
uniform float u_blend;
uniform float u_side;
uniform float u_contour;
uniform vec2 u_range;
uniform float u_natural_detail;
uniform bool u_earth_lighting;
uniform bool u_black_marble;
uniform bool u_solar_possible;
uniform float u_longitude;
uniform float u_atmosphere;
uniform vec3 u_sun;
uniform vec3 u_moon;
uniform sampler2D u_night;
vec3 light_surface(vec3 rgb, vec2 geo) {
    vec2 angles = radians(geo + vec2(u_longitude, 0.0));
    vec3 normal = vec3(cos(angles.y) * vec2(cos(angles.x), sin(angles.x)), sin(angles.y));
    const float e2 = 0.00669437999014;
    float n = 6378.137 * inversesqrt(1.0 - e2 * normal.z * normal.z);
    vec3 point = n * normal * vec3(1.0, 1.0, 1.0-e2);
    vec3 sunlight = u_sun - point;
    float elevation = dot(normal, normalize(sunlight));
    float day = smoothstep(-0.105, 0.0349, elevation);
    float shade = 0.0;
    if (u_solar_possible && elevation > 0.0) {
        shade = shadow(sunlight, u_moon-point, 1737.4).covered;
    }
    // Apply daylight to the composited cartography. Leave the existing relief
    // normals and shading intact and retain contrast on the night side.
    rgb *= mix(vec3(.36,.40,.49), vec3(1.0), day * (1.0 - .82 * pow(shade, 1.35)));
    if (u_black_marble && elevation < 0.0) {
        vec2 uv = vec2(angles.x / 6.28318530718 + .5, .5 - angles.y / 3.14159265359);
        vec3 observed = max(vec3(0.0), texture(u_night, uv).rgb - vec3(.10,.11,.15));
        rgb += vec3(1.0,.72,.38) * min(.36, dot(observed, vec3(.5,.4,.1))) * (1.0-day);
    }
    float rim = (1.0 - smoothstep(0.0, .23, max(0.0, v_front))) * u_atmosphere;
    rgb = mix(rgb, atmosphere_color(elevation), .38 * rim);
    return rgb;
}
vec3 hsl(float hue,float s,float l) {
    vec3 rgb=clamp(abs(mod(hue/60.0+vec3(0.0,4.0,2.0),6.0)-3.0)-1.0,0.0,1.0);
    return l+(rgb-0.5)*(1.0-abs(2.0*l-1.0))*s;
}
vec3 elevation_color(float h) {
    if(h<=0.0) return u_style==2?vec3(0.0):vec3(0.094,0.227,0.40);
    if(u_style==2) {
        float phase=fract(h/500.0);
        return hsl(mod(24.0+h/1850.0*360.0,360.0),0.86,0.14+0.76*(1.0-abs(2.0*phase-1.0)));
    }
    const float stops[7]=float[7](0.0,250.0,700.0,1400.0,2400.0,3800.0,5200.0);
    const vec3 colors[7]=vec3[7](vec3(96,146,88),vec3(148,174,108),vec3(204,194,132),vec3(194,146,96),vec3(168,114,88),vec3(196,184,178),vec3(240,244,248));
    for(int i=1;i<7;i++) if(h<=stops[i]) return mix(colors[i-1],colors[i],clamp((h-stops[i-1])/(stops[i]-stops[i-1]),0.0,1.0))/255.0;
    return colors[6]/255.0;
}
// The original close-zoom Natural palette, evaluated on the GPU. Classifying
// the source colours removes enlarged shading and water smudges baked into NE2.
vec3 biome_color(vec3 rgb, float h, float lat) {
    float luma=dot(rgb,vec3(0.299,0.587,0.114));
    rgb*=255.0;
    if((h>2400.0 && luma>0.78) || h>4200.0 || (abs(lat)>62.0 && luma>0.78)) return vec3(236,242,248)/255.0;
    if(rgb.r>rgb.g+14.0 && rgb.r>rgb.b+25.0) return vec3(186,166,134)/255.0;
    if(rgb.g>=rgb.r && rgb.g>rgb.b+10.0) return (luma<0.52?vec3(54,98,56):vec3(124,152,86))/255.0;
    if(rgb.r>=rgb.g && rgb.g>rgb.b) return vec3(158,144,98)/255.0;
    return vec3(124,152,86)/255.0;
}
void main() {
    if((u_blend>0.0 && abs(v_geo.x)>180.0) || (u_blend>0.2 && v_front < -0.02*(1.0-u_blend))) discard;
    vec4 base=texture(u_color,v_uv);
    vec2 dem=texture(u_height,v_uv).rg;
    float h=dem.r, shade=1.0;
    if(dem.g>0.99) {
        vec2 step_uv=vec2(1.0/258.0,0.0);
        float west=texture(u_height,v_uv-step_uv).r, east=texture(u_height,v_uv+step_uv).r;
        float south=texture(u_height,v_uv-step_uv.yx).r, north=texture(u_height,v_uv+step_uv.yx).r;
        float dy=u_side/256.0*111320.0;
        vec3 normal=normalize(vec3(-(east-west)/(2.0*dy*max(abs(cos(radians(v_geo.y))),0.15))*2.4,(north-south)/(2.0*dy)*2.4,1.0));
        shade=clamp(dot(normal,vec3(-0.5,0.5,0.70710678)),0.12,1.0);
    }
    vec3 rgb=base.rgb;
    if(base.a<0.5) {
        float checker=mod(floor(v_geo.x/15.0)+floor(v_geo.y/15.0),2.0);
        rgb=mix(vec3(0.18,0.24,0.29),vec3(0.22,0.28,0.33),checker);
    }
    if(u_style!=0 && dem.g>0.99) {
        rgb=elevation_color(h);
        if(u_style==3) rgb=h<=0.0?vec3(0.11,0.20,0.32):vec3((48.0+170.0*shade)/255.0);
        else if(h>0.0) rgb*=u_style==2?0.66+0.34*shade:0.64+0.36*shade;
    } else if(dem.g>0.99 && h>0.0) {
        rgb=mix(rgb,biome_color(rgb,h,v_geo.y),u_natural_detail);
        float local_norm=clamp((h-u_range.x)/max(u_range.y-u_range.x,1.0),0.0,1.0);
        float gradient=1.0-0.14*u_natural_detail+0.24*u_natural_detail*local_norm;
        rgb*=(0.76+0.36*(shade-0.5))*gradient;
    }
    if(u_contour>0.0 && dem.g>0.99 && h>0.0) {
        float phase=fract(h/u_contour);
        float distance_m=min(phase,1.0-phase)*u_contour;
        float coverage=1.0-smoothstep(0.0,max(fwidth(h),0.25),distance_m);
        float major=mod(round(h/u_contour),5.0)==0.0?0.58:0.30;
        rgb=mix(rgb,vec3(0.10,0.09,0.08),coverage*major);
    }
    if(u_show_water && texture(u_water,v_uv).r>0.75) rgb=u_style==2?vec3(0.0):mix(rgb,vec3(0.149,0.40,0.675),0.94);
    if (u_earth_lighting) rgb = light_surface(rgb, v_geo);
    frag=vec4(rgb,1.0);
}
