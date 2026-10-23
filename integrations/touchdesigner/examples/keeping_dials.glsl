// The Keeping time picture: three clock dials and an 8-second trace of each bar phase.
// Input 0: this frame's values, one channel per row (see the now Script CHOP):
//   0 a_phase, 1 s_phase, 2 b_phase, 3..5 beat in bar 0..3, 6 A+ holding, 9 beat glow, 10 time.
// Input 1: the last 8 seconds of the three bar phases (the hist Trail CHOP), one row each.
// Fosfora's two accents: blue for the bar and its trace, orange for the hand and the beat.
out vec4 fragColor;
const float TAU = 6.2831853;
const vec3 ORANGE = vec3(1.0, 0.376, 0.251);
const vec3 BLUE = vec3(0.314, 0.565, 0.878);

float now(int row) { return texelFetch(sTD2DInputs[0], ivec2(0, row), 0).r; }

float hist(int row, float x)
{
    int n = textureSize(sTD2DInputs[1], 0).x;
    int i = clamp(int(x * float(n - 1) + 0.5), 0, n - 1);
    return texelFetch(sTD2DInputs[1], ivec2(i, row), 0).r;
}

vec3 dial(vec2 p, vec2 c, float r, float phase, float beat, float hold, float glow, float t)
{
    vec2 d = p - c;
    float dist = length(d);
    float a01 = fract(atan(d.x, d.y) / TAU);              // 0 at twelve o'clock, clockwise
    float ring = exp(-pow((dist - r) / 3.5, 2.0));
    vec3 v = BLUE * (0.2 * ring + (a01 <= phase ? 0.9 * ring : 0.0));
    vec2 head = c + r * vec2(sin(phase * TAU), cos(phase * TAU));
    v += ORANGE * exp(-dot(p - head, p - head) / 70.0);   // the hand
    int cur = int(beat + 0.5);
    for (int k = 0; k < 4; k++) {                          // the four beats of the bar
        float ta = float(k) * 0.25 * TAU;
        vec2 tp = c + (r + 30.0) * vec2(sin(ta), cos(ta));
        bool on = k == cur;
        float rad = on ? 12.0 + 7.0 * glow : 8.0;
        float lit = on ? 1.0 : 0.25;
        bool waiting = hold > 0.5 && k == 3;
        if (waiting)
            lit = 0.55 + 0.45 * sin(t * 9.0);              // the held last beat, waiting
        v += (on || waiting ? ORANGE : BLUE) * lit * smoothstep(rad + 1.5, rad - 1.5, length(p - tp));
    }
    v += ORANGE * glow * 0.22 * smoothstep(r * 0.6, r * 0.55, dist);
    return v;
}

float trace(vec2 p, vec2 lo, vec2 hi, int row)
{
    if (p.x < lo.x || p.x > hi.x || p.y < lo.y - 3.0 || p.y > hi.y + 3.0)
        return 0.0;
    float w = hi.x - lo.x;
    float x = (p.x - lo.x) / w;
    float y = mix(lo.y, hi.y, hist(row, x));
    float yp = mix(lo.y, hi.y, hist(row, x - 1.0 / w));
    // the line, and a vertical stroke where the bar restarts, so each tooth is one bar
    float line = (p.y >= min(y, yp) - 1.2 && p.y <= max(y, yp) + 1.2) ? 1.0 : exp(-pow((p.y - y) / 1.4, 2.0));
    return 0.85 * line + 0.07 * step(abs(p.y - lo.y), 0.7);
}

void main()
{
    vec2 res = uTDOutputInfo.res.zw;
    vec2 p = vUV.st * res;
    float glow = now(9), t = now(10), r = 92.0;
    float cy = res.y * 0.63;
    vec3 v = dial(p, vec2(res.x * 0.2, cy), r, now(0), now(3), 0.0, glow, t)
            + dial(p, vec2(res.x * 0.5, cy), r, now(1), now(4), now(6), glow, t)
            + dial(p, vec2(res.x * 0.8, cy), r, now(2), now(5), 0.0, glow, t);
    for (int k = 0; k < 3; k++) {
        float top = res.y * (0.34 - 0.075 * float(k));
        v += BLUE * trace(p, vec2(res.x * 0.12, top - res.y * 0.05), vec2(res.x * 0.96, top), k);
    }
    fragColor = TDOutputSwizzle(vec4(1.0 - exp(-v * 1.3), 1.0));
}
