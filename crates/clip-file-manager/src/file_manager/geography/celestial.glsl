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

// Distance to a cell wall in a 3-D slice of granulation. This layer only
// resolves on approach to the photosphere (one unit is one solar radius).
float solar_granules(vec3 p, float phase) {
    vec3 cell = floor(p), local = fract(p);
    float first = 5.0, second = 5.0;
    for (int z = -1; z <= 1; ++z)
    for (int y = -1; y <= 1; ++y)
    for (int x = -1; x <= 1; ++x) {
        vec3 offset = vec3(x,y,z);
        vec3 seed = cell+offset;
        vec3 center = vec3(solar_hash(seed),solar_hash(seed+19.3),solar_hash(seed-7.8));
        vec3 delta = offset + .16+.68*center + .11*sin(phase+6.28*center.yzx) - local;
        float dist = dot(delta,delta);
        second = min(second,max(first,dist));
        first = min(first,dist);
    }
    return smoothstep(.0,.49,sqrt(second)-sqrt(first)) + .12*exp(-6.0*first);
}

// A bundle of coronal strands sharing two magnetic footpoints. Measure their
// projected width in the ray's tangent plane, keeping the loops round even
// edge-on. The foreground photosphere hides the backs of the arches.
float solar_arcade(vec3 anchor, vec3 tangent, float span, float phase,
                   vec3 across, vec3 ray, float front, float pixel, float burst) {
    vec3 side = cross(anchor,tangent);
    float light = 0.0;
    float width = .0033 + pixel*.45;
    for (int strand = 0; strand < 4; ++strand) {
        float f = float(strand)/3.0;
        float spread = span*(.73+.27*f);
        float height = spread*(.74+.82*f+.11*sin(phase+3.0*f)+.8*burst*f);
        vec3 start = anchor-tangent*spread;
        float closest = 1e3, along = 0.0;
        for (int step = 1; step <= 14; ++step) {
            float u = float(step)/14.0;
            float arch = sin(3.141593*u);
            vec3 end = anchor*(1.0+height*arch) - tangent*(spread*cos(3.141593*u))
                     + side*arch*span*(.19*sin(phase+.9*f+u*2.0)+.63*f-.25+.07*sin(3.0*phase+u*9.0));
            vec3 p = start-across, d = end-start;
            vec3 dp = d-ray*dot(d,ray);
            float t = clamp(-dot(p,dp)/max(dot(dp,dp),1e-9),0.0,1.0);
            vec3 q = p+t*d;
            float depth = dot(q,ray);
            q -= ray*depth;
            float dist = dot(q,q);
            if (dist < closest && depth <= front) {
                closest = dist;
                along = (float(step-1)+t)/14.0;
            }
            start = end;
        }
        float surge = .42+.37*cos(phase*2.0 - along*7.0+1.8*f);
        surge += .24*exp(-14.0*min(along,1.0-along));
        float d2 = closest/(width*width);
        light += surge*(.80*exp(-d2) + .23*exp(-d2/11.0) + .07*exp(-d2/57.0));
    }
    return light*.0033/width;
}

// Composite inspired by Inouye photospheric granulation and SDO's gold
// 171-angstrom images. Fine convection and loop flows use accelerated time.
vec4 solar_disk(vec3 ray, vec3 center, float angular_radius, vec2 clock) {
    float angular_pixel = max(length(dFdx(ray)),length(dFdy(ray)));
    float forward = dot(ray,center);
    if (forward <= 0.0 || angular_radius < 1e-9) return vec4(0);
    vec3 across = cross(ray,cross(ray,center))/angular_radius;
    float r = length(across);
    if (r > 1.65) return vec4(0);
    float pixel = clamp(angular_pixel/angular_radius,.0001,.75);
    float coverage = 1.0-smoothstep(1.0-pixel*.55,1.0+pixel*.55,r);
    float mu = sqrt(max(0.0,1.0-r*r));

    // Rotate both the surface and the overlying field together in object space.
    // Harmonic phases keep the animation continuous at the clock's 4096 s wrap.
    float rotation = 6.2831853*fract(clock.x/25.1);
    mat2 spin = mat2(cos(rotation),sin(rotation),-sin(rotation),cos(rotation));
    across.xy = spin*across.xy;
    ray.xy = spin*ray.xy;
    vec3 normal = across-ray*mu;
    float cycle = clock.y*(6.2831853/4096.0);
    vec3 color = vec3(0);
    if (coverage > 0.0) {
        vec3 warp = vec3(solar_noise(normal*4.6),solar_noise(normal*4.6+13.7),
                         solar_noise(normal*4.6+39.1));
        vec3 drift = .014*sin(cycle*vec3(321,245,278) + 10.0*warp);
        vec3 field = normal+.14*(warp-.5)+drift;
        float network = solar_noise(field*12.0);
        float ridge = exp(-22.0*abs(network-.49));
        float footprint = pixel*(forward-angular_radius*mu)/max(mu,.04);
        float fine = 1.0-smoothstep(.002,.024,footprint);
        float fibers = solar_noise(field*56.0 + 2.2*vec3(network,warp.z,network));
        float knots = solar_noise(field*25.0+warp.yzx);
        float grit = solar_noise(normal*174.0+vec3(fibers,network,knots)*1.8);
        float grain = 1.0-smoothstep(.002,.009,footprint);
        float emission = .53 + .21*(warp.y-.5) + .23*(knots-.5) + .065*ridge
                       + fine*.23*(fibers-.5) + grain*.20*(grit-.5);
        float micro = 1.0-smoothstep(.00045,.0022,footprint);
        if (micro > 0.0) {
            // Small lateral excursions continually deform the granular lanes.
            vec3 convection = normal*640.0 + .34*sin(cycle*327.0+field.zxy*84.0);
            convection += .46*vec3(solar_noise(convection*.76),solar_noise(convection*.81+13.2),
                                    solar_noise(convection*.72+31.1));
            emission += micro*(.19*(solar_granules(convection,cycle*267.0)-.45)
                               + .08*(solar_noise(convection*2.4)-.5));
        }
        // Foreshortening and limb darkening ground the bright atmosphere above it.
        emission *= .73+.27*mu;
        color = mix(vec3(.46,.080,.019),vec3(.97,.40,.070),smoothstep(.22,.54,emission));
        color += vec3(.15,.45,.43)*smoothstep(.46,.74,emission);
    }

    float height = max(0.0,r-1.0);
    vec3 limb = across/max(r,.001);
    // Plumes spread and bend with height while fine spicules hug the limb.
    limb += height*vec3(.22,-.14,.19)*sin(limb.yzx*6.0+cycle*91.0);
    float plume = solar_noise(limb*9.0+vec3(0,0,.24*sin(293.0*cycle)));
    float threads = solar_noise(limb*39.0+vec3(0,.37*sin(371.0*cycle+plume*4.0),0));
    float streamer = smoothstep(.29,.80,.65*plume+.35*threads);
    float corona = exp(-height*(17.0-11.0*streamer))*(.10+.22*streamer);
    float spicule = exp(-height*(46.0+50.0*threads))*(.25+.45*threads);
    float halo = (1.0-coverage)*(corona+spicule)*(1.0-smoothstep(.48,.65,height));
    vec3 outer = vec3(1.0,.35,.065)*corona + vec3(1.0,.55,.11)*spicule;

    // Emergent bipolar groups occupy the Sun's active latitude belts. Anchors
    // persist between frames; bright faculae, spot pairs and arches share them.
    float arches = 0.0;
    for (int i = 0; i < 9; ++i) {
        float id = float(i);
        float z = .43*sin(id*1.83+.7);
        // Slower rotation away from the equator, measured relative to spin.
        float lag = 6.2831853*fract(clock.x*(1.0/25.1-1.0/(25.1+4.8*z*z)));
        float longitude = id*2.39996 + .28*sin(id*4.1) + lag;
        vec3 anchor = vec3(cos(longitude)*sqrt(1.0-z*z),sin(longitude)*sqrt(1.0-z*z),z);
        vec3 east = vec3(-sin(longitude),cos(longitude),0);
        vec3 tangent = normalize(east + cross(anchor,east)*(.35+.64*sin(id*3.8)));
        vec3 delta = anchor-across;
        vec3 projected = delta-ray*dot(delta,ray);
        float span = .060+.084*(.5+.5*sin(id*4.6+1.1));
        if (length(projected) > span*2.85+pixel || dot(anchor,ray) > .73) continue;
        float phase = cycle*(213.0+19.0*id)+id*2.9;
        if (coverage > 0.0) {
            vec3 local = normal-anchor;
            vec2 p = vec2(dot(local,tangent),dot(local,cross(anchor,tangent)))/span;
            float disturbance = solar_noise(normal*55.0);
            float d = min(length(p-vec2(-.72,.12)),length((p-vec2(.77,-.05))*1.4))
                     * (.83+.35*disturbance);
            float spots = 1.0-smoothstep(.11,.42,d);
            color *= 1.0-.83*spots;
            // Projected field lines fan away from the two opposite poles.
            float field_angle = atan(p.y+.04,p.x-.73)-atan(p.y-.10,p.x+.72);
            float fibril = .5+.5*cos(20.0*field_angle + 9.0*disturbance+2.2*solar_noise(normal*29.0)+.55*sin(phase));
            vec2 region = p*vec2(.65,1.05);
            float field_light = exp(-dot(region,region))*(1.0-spots);
            float facula = field_light*(.20+.68*fibril+.35*disturbance);
            color = mix(color,vec3(1.0,.91,.52),min(facula,1.0));
        }
        // A few heavier outbursts pass through otherwise continuously lit arcades.
        float burst = pow(.5+.5*sin(cycle*79.0+id*2.8),22.0);
        arches += solar_arcade(anchor,tangent,span,phase,across,ray,
                               r<1.0 ? -mu : 2.0,pixel,burst) * (.77+.48*burst);
    }
    float line = 1.0-exp(-1.55*arches);
    vec3 base = min(color,vec3(1.0))*coverage + outer*(1.0-coverage)
              * (1.0-smoothstep(.48,.65,height));
    return vec4(mix(base,vec3(1.0,.92,.58),line), mix(min(1.0,coverage+halo),1.0,line));
}
