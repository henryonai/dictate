// The icon of the app, held in place: the master drawing, the icons that `scripts/make-icons.sh` makes from it, the tray images and the
// README header that shows the icon. The files are read from the repository (as identity.test.ts does); the PNGs are decoded here so
// that a test can tell that an icon is the one drawn in the master and not an older one that was left behind.
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { inflateSync } from "node:zlib";
import { describe, expect, test } from "vitest";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../..");
const icons = "app/src-tauri/icons";
const readText = (file: string) => readFileSync(join(repoRoot, file), "utf8").replace(/\r\n/g, "\n");
const readBytes = (file: string) => readFileSync(join(repoRoot, file));

const GRAPHITE = [0x23, 0x23, 0x27];
const BLUE = [0x4c, 0x9a, 0xff];
const NEAR_WHITE = [0xe9, 0xe9, 0xee];

interface Png {
  width: number;
  height: number;
  /** Red, green, blue and alpha of each pixel, row by row. */
  rgba: Uint8Array;
}

/** A PNG of 8 bits a channel with alpha, not interlaced: what `tauri icon` writes. */
function decodePng(file: string): Png {
  const bytes = readBytes(file);
  expect(bytes.subarray(0, 8).toString("hex"), `${file} is a PNG`).toBe("89504e470d0a1a0a");
  let width = 0;
  let height = 0;
  const data: Buffer[] = [];
  for (let at = 8; at < bytes.length; ) {
    const length = bytes.readUInt32BE(at);
    const kind = bytes.toString("latin1", at + 4, at + 8);
    const body = bytes.subarray(at + 8, at + 8 + length);
    if (kind === "IHDR") {
      width = body.readUInt32BE(0);
      height = body.readUInt32BE(4);
      expect([body[8], body[9], body[12]], `${file}: 8 bits, RGBA, not interlaced`).toEqual([8, 6, 0]);
    } else if (kind === "IDAT") {
      data.push(body);
    }
    at += 12 + length;
  }
  const raw = inflateSync(Buffer.concat(data));
  const stride = width * 4;
  const rgba = new Uint8Array(stride * height);
  for (let y = 0; y < height; y++) {
    const filter = raw[y * (stride + 1)];
    for (let x = 0; x < stride; x++) {
      const value = raw[y * (stride + 1) + 1 + x];
      const left = x >= 4 ? rgba[y * stride + x - 4] : 0;
      const up = y > 0 ? rgba[(y - 1) * stride + x] : 0;
      const upLeft = x >= 4 && y > 0 ? rgba[(y - 1) * stride + x - 4] : 0;
      let predicted = 0;
      if (filter === 1) predicted = left;
      else if (filter === 2) predicted = up;
      else if (filter === 3) predicted = (left + up) >> 1;
      else if (filter === 4) {
        const p = left + up - upLeft;
        const [pa, pb, pc] = [Math.abs(p - left), Math.abs(p - up), Math.abs(p - upLeft)];
        predicted = pa <= pb && pa <= pc ? left : pb <= pc ? up : upLeft;
      }
      rgba[y * stride + x] = (value + predicted) & 0xff;
    }
  }
  return { width, height, rgba };
}

/** How many opaque pixels are within `tolerance` of the color on every channel. */
function near(png: Png, color: number[], tolerance = 12): number {
  let count = 0;
  for (let i = 0; i < png.rgba.length; i += 4) {
    if (png.rgba[i + 3] === 255 && color.every((c, k) => Math.abs(png.rgba[i + k] - c) <= tolerance)) count++;
  }
  return count;
}

describe("the icon of the app", () => {
  test("the_master_drawing_is_the_loah_icon_in_graphite_blue_and_near_white", () => {
    const master = readText(`${icons}/source/loah.svg`);
    expect(master).toContain('viewBox="0 0 1024 1024"');
    for (const color of ["#232327", "#4c9aff", "#e9e9ee"]) expect(master.toLowerCase(), color).toContain(color);
  });

  test("the_tray_glyph_is_drawn_in_one_color_that_the_system_chooses", () => {
    const glyph = readText(`${icons}/source/loah-tray.svg`);
    expect(glyph).toContain("currentColor");
    // No color of its own anywhere: a template image is a shape, and the menu bar paints it.
    expect(glyph).not.toMatch(/#[0-9a-f]{3,8}\b/i);
  });

  test("the_script_that_makes_the_icons_names_the_drawings_and_the_tauri_command", () => {
    const script = readText("scripts/make-icons.sh");
    expect(script).toContain("tauri icon");
    expect(script).toContain("icons/source/loah.svg");
    expect(script).toContain("icons/source/loah-tray.svg");
  });

  test("the_check_command_of_the_script_names_the_drawing_from_the_folder_that_pnpm_dir_app_runs_in", () => {
    // `pnpm --dir app` runs the command with `app/` as its working folder: a path that starts with `app/` is not there.
    const script = readText("scripts/make-icons.sh").replace(/\n#\s*/g, " ");
    expect(script).toContain("pnpm --dir app tauri icon src-tauri/icons/source/loah.svg");
    expect(script).not.toContain("pnpm --dir app tauri icon app/");
  });

  test("every_icon_the_bundle_lists_is_there_in_its_size", () => {
    const config = JSON.parse(readText("app/src-tauri/tauri.conf.json"));
    const listed: string[] = config.bundle.icon;
    expect(listed.length).toBeGreaterThan(0);
    for (const icon of listed) expect(existsSync(join(repoRoot, "app/src-tauri", icon)), icon).toBe(true);
    const size = (name: string) => decodePng(`${icons}/${name}`).width;
    expect(size("32x32.png")).toBe(32);
    expect(size("128x128.png")).toBe(128);
    expect(size("128x128@2x.png")).toBe(256);
    expect(size("icon.png")).toBe(512);
    expect(readBytes(`${icons}/icon.icns`).toString("latin1", 0, 4)).toBe("icns");
    expect(readBytes(`${icons}/icon.ico`).subarray(0, 4).toString("hex")).toBe("00000100");
  });

  test("the_icons_are_drawn_from_the_master_and_not_from_the_icon_before_it", () => {
    for (const name of ["128x128.png", "128x128@2x.png", "icon.png"]) {
      const png = decodePng(`${icons}/${name}`);
      const pixels = png.width * png.height;
      // Graphite is the tile, blue the sound bars, near-white the lines: the one before it was navy and orange.
      expect(near(png, GRAPHITE), `${name}: the tile`).toBeGreaterThan(pixels * 0.4);
      expect(near(png, BLUE), `${name}: the sound bars`).toBeGreaterThan(pixels * 0.01);
      expect(near(png, NEAR_WHITE), `${name}: the lines`).toBeGreaterThan(pixels * 0.01);
      // The tile leaves a clear margin all round, as the icons of macOS do: the corner pixel is transparent.
      expect(png.rgba[3], `${name}: the margin`).toBe(0);
    }
  });

  test("the_tray_images_come_in_two_sizes_each", () => {
    const sizes = {
      "tray-template.png": 18,
      "tray-template@2x.png": 36,
      "tray.png": 16,
      "tray@2x.png": 32,
    };
    for (const [name, size] of Object.entries(sizes)) {
      const png = decodePng(`${icons}/tray/${name}`);
      expect([png.width, png.height], name).toEqual([size, size]);
    }
  });

  test("the_readme_headers_show_the_generated_128_pixel_icon", () => {
    for (const readme of ["README.md", "README.vi.md"]) {
      const header = readText(readme).split("\n")[0];
      expect(header, readme).toContain(`${icons}/128x128.png`);
    }
    expect(decodePng(`${icons}/128x128.png`).width).toBe(128);
  });
});
