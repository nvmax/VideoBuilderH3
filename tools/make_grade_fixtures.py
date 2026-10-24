"""Dev tool: golden pictures for ``h3-media::grade``, made by the pack's own torch code.

    python tools/make_grade_fixtures.py --pack <pack folder>

Writes ``fixtures/post/source.png``, ``fixtures/post/lut_*.cube``, one result PNG per case and ``fixtures/post/cases.json``.
The cases run the pack's ``apply_adjust_to_image`` and ``apply_lut_to_image`` on the CPU. Film grain is left out: it draws
torch's random numbers, which cannot be matched.
"""

import argparse
import importlib
import json
import math
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import pack_env  # noqa: E402

WIDTH, HEIGHT = 53, 41

ADJUST_CASES = {
    "adjust_warm_contrast": {"temperature": 40, "tint": -15, "contrast": 25, "saturation": 20, "exposure": 10},
    "adjust_lifts": {"highlights": -40, "shadows": 55, "whites": 30, "blacks": -25, "fade": 20},
    "adjust_detail": {"clarity": 60, "sharpen": 45},
    "adjust_negative_clarity": {"clarity": -70, "vignette": 55},
    "adjust_everything": {
        "temperature": -30, "tint": 20, "saturation": -35, "exposure": -12, "contrast": 18, "highlights": 20, "shadows": -10,
        "whites": 12, "blacks": 8, "sharpen": 20, "clarity": 25, "vignette": 30, "fade": 10,
    },
    "adjust_disabled": {"enabled": False, "contrast": 50},
    "adjust_defaults": {},
}


def make_source(path: Path) -> None:
    from PIL import Image

    rng = random.Random(1234)
    image = Image.new("RGB", (WIDTH, HEIGHT))
    pixels = image.load()
    for y in range(HEIGHT):
        for x in range(WIDTH):
            base = (x * 255 // (WIDTH - 1), y * 255 // (HEIGHT - 1), (x * 3 + y * 5) % 256)
            noise = [rng.randint(-24, 24) for _ in range(3)]
            block = 60 if (x // 9 + y // 9) % 2 else 0
            pixels[x, y] = tuple(max(0, min(255, base[i] + noise[i] - block)) for i in range(3))
    image.save(path)


def write_cube(path: Path, size: int, domain_max: float = 1.0) -> None:
    lines = ["TITLE \"golden\"", f"LUT_3D_SIZE {size}", "DOMAIN_MIN 0.0 0.0 0.0", f"DOMAIN_MAX {domain_max} {domain_max} {domain_max}"]
    for b in range(size):
        for g in range(size):
            for r in range(size):
                rn, gn, bn = r / (size - 1), g / (size - 1), b / (size - 1)
                red = min(1.0, rn * 0.9 + gn * 0.15 + 0.03)
                green = gn ** 0.8 * 0.95 + bn * 0.05
                blue = min(1.0, bn * 0.7 + rn * 0.2 + 0.1 * math.sin(gn * 3.0) ** 2)
                lines.append(f"{red:.6f} {green:.6f} {blue:.6f}")
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--pack", required=True, type=Path)
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "fixtures" / "post")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    pack_env.install(args.pack)
    tools = importlib.import_module("vrgdg_pack.post_process.lut_video_tools")
    luts = importlib.import_module("vrgdg_pack.post_process.luts")

    source = args.out / "source.png"
    make_source(source)
    cases = []

    for name, settings in ADJUST_CASES.items():
        target = args.out / f"{name}.png"
        result = tools.apply_adjust_to_image(str(source), output_path=str(target), settings=settings, device="cpu")
        cases.append({"kind": "adjust", "name": name, "settings": settings, "output": target.name, "normalized": result["settings"]})

    cube_a = args.out / "lut_mix.cube"
    cube_b = args.out / "lut_domain.cube"
    write_cube(cube_a, 17)
    write_cube(cube_b, 9, domain_max=0.8)
    luts_dir = args.out / "luts_for_pack"
    luts_dir.mkdir(exist_ok=True)
    for cube in (cube_a, cube_b):
        (luts_dir / cube.name).write_text(cube.read_text(encoding="utf-8"), encoding="utf-8")
    luts.LUTS_DIR = str(luts_dir)
    tools.LUTS_DIR = str(luts_dir)
    for cube, strength in ((cube_a, 10.0), (cube_a, 4.0), (cube_b, 10.0), (cube_b, 0.0)):
        target = args.out / f"lut_{cube.stem}_{int(strength)}.png"
        tools.apply_lut_to_image(str(source), cube.name, output_path=str(target), strength=strength, device="cpu")
        cases.append({"kind": "lut", "name": f"{cube.stem}_{int(strength)}", "cube": cube.name, "strength": strength, "output": target.name})

    (luts_dir / cube_a.name).unlink()
    (luts_dir / cube_b.name).unlink()
    luts_dir.rmdir()
    (args.out / "cases.json").write_text(json.dumps({"width": WIDTH, "height": HEIGHT, "cases": cases}, indent=1), encoding="utf-8")
    print("grade cases:", len(cases))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
