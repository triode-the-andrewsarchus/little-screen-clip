// NASA SVS star map, sampled in its inertial (J2000 equatorial) frame.
vec3 star_color(sampler2D stars, vec3 direction) {
    const float pi = 3.141592653589793;
    vec2 uv = vec2(.5-atan(direction.y,direction.x)/(2.0*pi),
                   .5-asin(clamp(direction.z,-1.0,1.0))/pi);
    // Longitude wraps at the date line.
    vec2 dx = dFdx(uv), dy = dFdy(uv);
    dx.x -= round(dx.x);
    dy.x -= round(dy.x);
    return textureGrad(stars,uv,dx,dy).rgb;
}

float solar_hash(vec3 p) {
    p = fract(p * .3183099 + vec3(.13,.29,.43)) * 17.0;
    return fract(p.x*p.y*p.z*(p.x+p.y+p.z));
}

float solar_noise(vec3 p) {
    vec3 i = floor(p);
    vec3 f = fract(p);
    f = f*f*(3.0-2.0*f);
    vec4 a = vec4(solar_hash(i),solar_hash(i+vec3(1,0,0)),
                  solar_hash(i+vec3(0,1,0)),solar_hash(i+vec3(1,1,0)));
    i.z += 1.0;
    vec4 b = vec4(solar_hash(i),solar_hash(i+vec3(1,0,0)),
                  solar_hash(i+vec3(0,1,0)),solar_hash(i+vec3(1,1,0)));
    vec4 v = mix(a,b,f.z);
    return mix(mix(v.x,v.y,f.x),mix(v.z,v.w,f.x),f.y);
}

vec3 solar_palette(float power) {
    vec3 core = mix(vec3(.19,.006,.001),vec3(.98,.24,.012),smoothstep(.04,.65,power));
    return mix(core,vec3(1.0,.94,.45),smoothstep(.51,1.0,power));
}

// Premultiplied emissive disk and limb. The same angular projection also
// handles a camera near the star and a disk only a few pixels across.
vec4 solar_disk(vec3 ray, vec3 center, float angular_radius, vec2 clock) {
    if (dot(ray,center) <= 0.0 || angular_radius < 1e-9) return vec4(0);
    vec3 radial = (ray - center*dot(ray,center))/angular_radius;
    float r = length(radial);
    if (r > 1.65) return vec4(0);
    float footprint = clamp(length(dFdx(ray))/angular_radius,.0003,.75);
    vec3 normal = radial-center*sqrt(max(0.0,1.0-r*r));
    // Differential rotation, approximately 25 days at the equator and
    // slower toward the poles. clock.x is days since J2000, clock.y seconds.
    float latitude = normal.z*normal.z;
    float rotation = 6.28318531*fract(clock.x/25.1)
                   - .65*latitude*sin(6.28318531*clock.x/107.0);
    float s = sin(rotation), c = cos(rotation);
    vec3 surface = vec3(mat2(c,s,-s,c)*normal.xy,normal.z);
    vec3 flow = vec3(solar_noise(surface*5.0),
                     solar_noise(surface*5.0+17.0),
                     solar_noise(surface*5.0+29.0));
    vec3 warped = surface + .13*(flow-.5);
    float mottling = solar_noise(warped*7.5);
    float plasma = solar_noise(warped*22.0+vec3(0,0,clock.y*.016));
    float detail = 1.0-smoothstep(.007,.085,footprint);
    float granules = mix(.5,solar_noise(surface*84.0+vec3(clock.y*.036,0,0)),detail);
    float knots = solar_noise(warped*9.5+1.3*solar_noise(surface*3.0));
    float spots = smoothstep(.66,.82,knots) * (1.0-smoothstep(.47,.74,abs(normal.z)));
    float hot_regions = smoothstep(.67,.88,mottling);
    float temperature = .26 + .36*mottling + .27*plasma + .15*(granules-.5)
                      + .25*hot_regions - .39*spots;
    temperature += smoothstep(.85,1.0,r)*.12;
    vec3 disk = solar_palette(temperature);
    float coverage = 1.0-smoothstep(1.0-footprint,1.0+footprint,r);

    vec3 limb = normalize(radial+vec3(1e-7));
    float filament = solar_noise(limb*12.0+vec3(0,0,clock.y*.018));
    float height = max(0.0,r-1.0);
    float corona = exp(-height*(9.5+18.0*filament))* (.18+.26*filament);
    float loop = smoothstep(.67,.86,filament) * exp(-pow((height-.045)/.029,2.0));
    float cycle = mod(clock.y,37.0);
    float flare_time = smoothstep(17.0,18.0,cycle)*(1.0-smoothstep(18.5,22.0,cycle));
    vec3 event = normalize(vec3(sin(floor(clock.y/37.0)*2.4),.36,cos(floor(clock.y/37.0)*2.4)));
    event = normalize(event-center*dot(center,event) + vec3(1e-6));
    float flare = flare_time*exp(-height*9.0)*pow(max(dot(limb,event),0.0),75.0);
    float halo = clamp(corona+loop*.42+flare*.68,0.0,1.0)*(1.0-coverage);
    vec3 atmosphere = solar_palette(.58 + .36*filament + .3*flare);
    return vec4(disk*coverage + atmosphere*halo,coverage+halo);
}
