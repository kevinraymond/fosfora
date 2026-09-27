# Usage: python3 scripts/make_fosfora_image.py  (from the repo root; needs Pillow)
# Render assets/images/raster_fosfora.png: FOSFORA in Inter Bold, teal
# fading to amber left to right, with a glow and a scatter of sparks on
# black, matching the size and look of the old raster_phosphor.png.
import random
from PIL import Image, ImageDraw, ImageFilter, ImageFont, ImageChops

W, H = 2048, 1152
TEAL = (40, 235, 200)
AMBER = (255, 190, 70)
random.seed(7)

font = ImageFont.truetype("assets/fonts/Inter-Bold.ttf", 330)
mask = Image.new("L", (W, H), 0)
d = ImageDraw.Draw(mask)
text = "FOSFORA"
box = d.textbbox((0, 0), text, font=font)
tw, th = box[2] - box[0], box[3] - box[1]
x0 = (W - tw) // 2 - box[0]
y0 = (H - th) // 2 - box[1]
d.text((x0, y0), text, font=font, fill=255)

# The gradient across the word.
grad = Image.new("RGB", (W, H))
gp = grad.load()
left, right = x0 + box[0], x0 + box[0] + tw
for x in range(W):
    t = min(max((x - left) / max(right - left, 1), 0.0), 1.0)
    t = t * t * (3 - 2 * t)
    c = tuple(int(TEAL[i] * (1 - t) + AMBER[i] * t) for i in range(3))
    for y in range(H):
        gp[x, y] = c

# Letters with a speckled fill: the particle look of the old picture.
speck = Image.new("L", (W, H), 0)
sp = speck.load()
for _ in range(150_000):
    x, y = random.randrange(W), random.randrange(H)
    sp[x, y] = random.randint(120, 255)
speck = speck.filter(ImageFilter.MaxFilter(3))
fill = ImageChops.multiply(mask, ImageChops.lighter(speck, Image.new("L", (W, H), 45)))

# Bright outlines, as in the old picture.
edge = mask.filter(ImageFilter.FIND_EDGES).filter(ImageFilter.MaxFilter(5))
letters = ImageChops.lighter(fill, edge)

glow = letters.filter(ImageFilter.GaussianBlur(14)).point(lambda v: int(v * 0.6))
halo = mask.filter(ImageFilter.GaussianBlur(60)).point(lambda v: int(v * 0.35))
light = ImageChops.add(ImageChops.add(letters, glow), halo)

# Sparks thrown off around the word.
sparks = Image.new("L", (W, H), 0)
sd = ImageDraw.Draw(sparks)
for _ in range(2600):
    x = random.gauss(W / 2, tw / 2.2)
    y = random.gauss(H / 2, th * 0.9)
    r = random.choice([1, 1, 1, 2, 2, 3])
    sd.ellipse([x - r, y - r, x + r, y + r], fill=random.randint(90, 255))
sparks = ImageChops.add(sparks, sparks.filter(ImageFilter.GaussianBlur(3)))
light = ImageChops.add(light, sparks)

out = ImageChops.multiply(Image.merge("RGB", [light] * 3), grad)
out.save("assets/images/raster_fosfora.png", optimize=True)
print("wrote", out.size)
