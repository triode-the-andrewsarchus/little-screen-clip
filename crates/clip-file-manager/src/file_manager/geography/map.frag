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
uniform float u_zoom;
uniform float u_side;
uniform float u_contour;
uniform float u_earth_lighting;
uniform bool u_black_marble;
uniform bool u_solar_possible;
uniform float u_longitude;
uniform float u_atmosphere;
uniform vec3 u_sun;
uniform vec3 u_moon;
uniform sampler2D u_night;
const vec3 LUMA = vec3(.299,.587,.114);
const float NATURAL_LAND_EXPOSURE = .84;
float night_signal(vec3 sample_rgb) {
    // Black Marble includes a blue background in the distributed JPEG.
    return dot(max(sample_rgb - vec3(.10,.11,.15), vec3(0.0)), vec3(.5,.4,.1));
}
vec3 light_surface(vec3 rgb, vec2 geo) {
    vec2 angles = radians(geo + vec2(u_longitude, 0.0));
    vec3 normal = vec3(cos(angles.y) * vec2(cos(angles.x), sin(angles.x)), sin(angles.y));
    const float e2 = 0.00669437999014;
    float n = 6378.137 * inversesqrt(1.0 - e2 * normal.z * normal.z);
    vec3 point = n * normal * vec3(1.0, 1.0, 1.0-e2);
    vec3 sunlight = u_sun - point;
    float elevation = dot(normal, normalize(sunlight));
    // Build the daylight grade from a faint twilight contribution and direct
    // sun that rises from zero as the upper edge of the Sun clears the horizon.
    // The Moon removes direct light without leaving a band of unshadowed day
    // between the night side and an eclipse at sunrise or sunset.
    float solar_radius = SOLAR_RADIUS_KM / length(sunlight);
    float twilight = .18 * smoothstep(-.105, .0349, elevation);
    float direct = .82 * smoothstep(-solar_radius, .0349, elevation);
    if (u_solar_possible && direct > 0.0) {
        direct *= 1.0 - shadow(sunlight, u_moon - point, 1737.4).covered;
    }
    // Grade the same terrain into night so the relief and coastline line up
    // through the terminator. Keep the brighter grade for navigating up close.
    vec3 night_surface = rgb * vec3(.36,.40,.49);
    if (u_style == 0) {
        float overview = u_blend * (1.0 - smoothstep(1.35, 3.75, u_zoom));
        float luma = dot(rgb, LUMA);
        vec3 dark_surface = rgb * vec3(.06,.075,.10) + luma * luma * vec3(.055,.080,.15);
        night_surface = mix(night_surface, dark_surface, overview);
    }
    rgb = mix(night_surface, rgb, twilight + direct);
    if (u_black_marble) {
        vec2 uv = vec2(angles.x / 6.28318530718 + .5, .5 - angles.y / 3.14159265359);
        vec2 dx = dFdx(uv), dy = dFdy(uv);
        vec2 footprint = abs(dx) + abs(dy);
        vec2 texture_size = vec2(textureSize(u_night, 0));
        vec2 texels_per_pixel = footprint * texture_size;
        // Fade the raster as source texels enlarge from 4.5 to 12 screen pixels.
        // Detailed roads sample the same light data separately for their styling.
        float detail = smoothstep(1.0 / 12.0, 1.0 / 4.5, min(texels_per_pixel.x, texels_per_pixel.y));
        if (elevation < 0.0 && detail > 0.0) {
            // Use coarser mip levels for nearby light around the detailed core.
            vec2 glow_extent = max(2.0 * footprint, 4.0 / texture_size);
            float local = night_signal(textureGrad(u_night, uv, dx, dy).rgb);
            float nearby = night_signal(textureGrad(u_night, uv,
                vec2(glow_extent.x, 0.0), vec2(0.0, glow_extent.y)).rgb);
            float night = detail * (1.0 - smoothstep(-.105, 0.0, elevation));
            vec3 warm = vec3(1.0, .76, .36);
            vec3 core_color = mix(warm, vec3(1.0, .98, .92), smoothstep(.24, .74, local));
            rgb = mix(rgb, warm, .14 * smoothstep(.10, .40, nearby) * night);
            rgb = mix(rgb, core_color, .97 * smoothstep(.10, .80, local) * night);
        }
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
    if(u_style==2) {
        float phase=fract(h/500.0);
        return hsl(mod(24.0+h/1850.0*360.0,360.0),0.86,0.14+0.76*(1.0-abs(2.0*phase-1.0)));
    }
    const float stops[7]=float[7](0.0,250.0,700.0,1400.0,2400.0,3800.0,5200.0);
    const vec3 colors[7]=vec3[7](vec3(96,146,88),vec3(148,174,108),vec3(204,194,132),vec3(194,146,96),vec3(168,114,88),vec3(196,184,178),vec3(240,244,248));
    for(int i=1;i<7;i++) if(h<=stops[i]) return mix(colors[i-1],colors[i],clamp((h-stops[i-1])/(stops[i]-stops[i-1]),0.0,1.0))/255.0;
    return colors[6]/255.0;
}
float edge_coverage(float value, float border, float min_width) {
    float width = clamp(.5 * fwidth(value), min_width, .5);
    return smoothstep(border - width, border + width, value);
}
float land_coverage(vec2 uv) {
    ivec2 last = textureSize(u_height, 0) - 1;
    vec2 grid = uv * vec2(last + 1) - .5;
    ivec2 p = ivec2(floor(grid));
    vec2 t = fract(grid);
    // Interpolate the shoreline between samples. The actual elevations can
    // sit at exactly zero for an entire bay and jump abruptly on land.
    float nw = float(texelFetch(u_height, clamp(p, ivec2(0), last), 0).r > 0.0);
    float ne = float(texelFetch(u_height, clamp(p + ivec2(1,0), ivec2(0), last), 0).r > 0.0);
    float sw = float(texelFetch(u_height, clamp(p + ivec2(0,1), ivec2(0), last), 0).r > 0.0);
    float se = float(texelFetch(u_height, clamp(p + ivec2(1), ivec2(0), last), 0).r > 0.0);
    return edge_coverage(mix(mix(nw, ne, t.x), mix(sw, se, t.x), t.y), .5, .04);
}
// Continuous tints derived from the imagery. The channel gaps distinguish
// green vegetation, yellow-green open ground, and warmer bare soil and sand.
vec3 biome_color(vec3 rgb, float h, float lat) {
    float luma = dot(rgb, LUMA);
    float warm = rgb.r - rgb.g;
    float green = smoothstep(.005, .072, -warm) * smoothstep(.025, .10, rgb.g - rgb.b);
    float bare = smoothstep(.008, .080, warm) * smoothstep(.045, .125, rgb.r - rgb.b);
    vec3 open = mix(vec3(140,151,110), vec3(167,154,113), smoothstep(-.035, .047, warm));
    vec3 vegetation = mix(vec3(69,105,74), vec3(134,161,100), smoothstep(.30, .63, luma));
    vec3 earth = mix(vec3(151,117,89), vec3(217,184,139), smoothstep(.43, .86, luma));
    vec3 ground = mix(mix(open, earth, bare), vegetation, green) / 255.0;
    float snow = smoothstep(.72, .87, luma) * max(smoothstep(2400.0, 4000.0, h),
                                                     smoothstep(57.0, 68.0, abs(lat)));
    return mix(mix(ground, rgb, .24), vec3(236,242,248) / 255.0, snow);
}

vec3 natural_land(vec3 source, float h, float lat, float shade, float detail) {
    vec3 overview = source * NATURAL_LAND_EXPOSURE;
    if (detail == 0.0) return overview;
    vec3 palette = biome_color(source, h, lat);
    float source_luma = dot(overview, LUMA);
    float palette_luma = dot(palette, LUMA);
    // Match the imagery's brightness as the palette comes in. Mixing towards
    // black or white keeps the matched color inside the display gamut.
    vec3 matched = source_luma < palette_luma
        ? palette * (source_luma / palette_luma)
        : mix(palette, vec3(1.0), (source_luma - palette_luma) / (1.0 - palette_luma));
    // Flat terrain is neutral. Finer elevation adds relief around that level.
    const float flat_shade = .70710678;
    return mix(overview, matched * (1.0 + .30 * (shade - flat_shade)), detail);
}

float contour_coverage(float height, float interval, float footprint) {
    float phase = fract(height / interval);
    float distance_m = min(phase, 1.0 - phase) * interval;
    return 1.0 - smoothstep(0.0, footprint, distance_m);
}

float contour_visibility(float spacing) {
    return smoothstep(2.0, 4.0, spacing) * min(1.0, spacing / 10.0);
}

float contour_ink(float height) {
    float footprint = max(fwidth(height), .25);
    float minor = contour_coverage(height, u_contour, footprint);
    if (u_style != 0) {
        return minor * (mod(round(height / u_contour), 5.0) == 0.0 ? .58 : .30);
    }
    // Fade densely spaced contours before antialiasing turns the whole slope
    // gray. Index contours have their own spacing and stay visible longer.
    float index_interval = 5.0 * u_contour;
    float index = contour_coverage(height, index_interval, footprint);
    return max(.30 * minor * contour_visibility(u_contour / footprint),
               .58 * index * contour_visibility(index_interval / footprint));
}

void main() {
    if((u_blend>0.0 && abs(v_geo.x)>180.0) || (u_blend>0.2 && v_front < -0.02*(1.0-u_blend))) discard;
    vec4 base=texture(u_color,v_uv);
    vec2 dem=texture(u_height,v_uv).rg;
    float h=dem.r, shade=1.0;
    float detail = smoothstep(5.0, 9.0, u_zoom);
    float land = u_style != 0 || detail > 0.0 ? land_coverage(v_uv) : float(h > 0.0);
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
    if (dem.g > .99) {
        const vec3 water_blue = vec3(.149,.40,.675);
        vec3 water;
        if (u_style == 0) {
            water = mix(rgb, water_blue, detail);
            rgb = natural_land(rgb, h, v_geo.y, shade, detail);
        } else if (u_style == 3) {
            water = vec3(.11,.20,.32);
            rgb = vec3((48.0+170.0*shade)/255.0);
        } else {
            water = u_style == 2 ? vec3(0.0) : vec3(.094,.227,.40);
            rgb = elevation_color(max(h,0.0));
            rgb *= u_style == 2 ? .66+.34*shade : .64+.36*shade;
        }
        rgb = mix(water, rgb, land);
    } else if (u_style == 0 && base.a >= .5) {
        // Approximate land from the imagery while elevations are unavailable.
        float water_hint = smoothstep(.02, .10, rgb.b - rgb.g);
        rgb *= mix(NATURAL_LAND_EXPOSURE, 1.0, water_hint);
    }
    if (u_contour > 0.0) {
        float ink = contour_ink(h);
        if (dem.g > .99 && h > 0.0) rgb = mix(rgb, vec3(.10,.09,.08), ink * land);
    }
    if (u_show_water) {
        float water = edge_coverage(texture(u_water,v_uv).r, .75, .04);
        rgb = mix(rgb, u_style == 2 ? vec3(0.0) : vec3(.149,.40,.675), water * (u_style == 2 ? 1.0 : .94));
    }
    if (u_earth_lighting > 0.0) rgb = mix(rgb, light_surface(rgb, v_geo), u_earth_lighting);
    frag=vec4(rgb,1.0);
}
