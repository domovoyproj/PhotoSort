"""Generate disposable, clearly named UI test images, never user photos."""
from pathlib import Path
from PIL import Image, ImageDraw

root = Path(__file__).resolve().parent.parent / 'build' / 'ui-fixtures'
root.mkdir(parents=True, exist_ok=True)
for index in range(12):
    image = Image.new('RGB', (1200, 800), (215 - index * 4, 220 - index * 3, 223 - index * 3))
    draw = ImageDraw.Draw(image)
    draw.ellipse((800 - index * 20, 100, 930 - index * 20, 230), fill=(245, 244, 230))
    draw.polygon([(0, 800), (0, 580), (350, 230 + index * 14), (680, 610), (960, 320), (1200, 550), (1200, 800)], fill=(99 + index * 4, 114 + index * 3, 119 + index * 3))
    draw.polygon([(0, 800), (0, 720), (600, 520 + index * 9), (1200, 760), (1200, 800)], fill=(55, 68, 72))
    exif = Image.Exif()
    exif[36867] = f'2026:10:01 10:00:{index:02d}'
    image.save(root / f'TEST_landscape_{index:02d}.jpg', exif=exif)
(root / 'TEST_duplicate.jpg').write_bytes((root / 'TEST_landscape_00.jpg').read_bytes())
print(root)
