// Eight moving-head beams and their pools on the floor, drawn from the show CHOP.
// Input 0 is that CHOP as a texture, one channel per row: fixture i's dimmer on row 2i,
// its pan on row 2i+1 (0..1, 0.5 straight down), and the haze level on row 16.
// Fosfora's two accents: orange beams and pools, blue floor and haze.
out vec4 fragColor;
const vec3 ORANGE = vec3(1.0, 0.376, 0.251);
const vec3 BLUE = vec3(0.314, 0.565, 0.878);

float ch(int row) { return texelFetch(sTD2DInputs[0], ivec2(0, row), 0).r; }

void main()
{
    vec2 res = uTDOutputInfo.res.zw;
    vec2 p = vUV.st * res;
    float floorY = res.y * 0.14;
    float haze = ch(16);
    vec3 col = vec3(0.0);
    for (int i = 0; i < 8; i++) {
        float dim = ch(2 * i);
        float ang = (ch(2 * i + 1) - 0.5) * 1.1;
        vec2 head = vec2(res.x * (0.12 + 0.76 * float(i) / 7.0), res.y * 0.93);
        vec2 dir = vec2(sin(ang), -cos(ang));
        vec2 d = p - head;
        float along = dot(d, dir);
        float across = abs(d.x * dir.y - d.y * dir.x);
        float width = 5.0 + along * 0.075;
        float reach = (head.y - floorY) / max(0.25, cos(ang));
        float beam = (along > 0.0 && along < reach) ? exp(-pow(across / width, 2.0)) : 0.0;
        beam *= 0.25 + 0.75 * exp(-along / (res.y * 0.9));
        vec2 q = (p - (head + dir * reach)) / vec2(width * 1.6, 10.0 + width * 0.25);
        float pool = exp(-dot(q, q));
        float lens = exp(-dot(d, d) / 90.0);
        col += ORANGE * dim * ((0.35 + 0.65 * haze) * 0.6 * beam + 0.9 * pool) + mix(ORANGE, vec3(1.0), 0.6) * (0.25 + dim) * lens;
    }
    col += BLUE * 0.12 * exp(-abs(p.y - floorY) / 2.0);  // the floor line
    col += BLUE * haze * 0.05 * smoothstep(res.y, 0.0, p.y);   // haze, thicker near the floor
    col = 1.0 - exp(-col * 1.5);                          // overlaps bloom instead of clipping
    fragColor = TDOutputSwizzle(vec4(col, 1.0));
}
