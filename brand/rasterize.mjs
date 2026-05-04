#!/usr/bin/env node
// Rasterize SVG avatars to PNG sets for upload to GitHub, npm, and favicons.
//
// Usage (from repo root):
//   node brand/rasterize.mjs
//
// Output:
//   brand/avatars/png/<name>-<size>.png
//
// Sizes:
//   500 — GitHub org/repo avatar, npm package avatar
//   256 — high-DPI npm display, generic web
//   128 — GitHub README inline mark
//    64 — small inline icon
//    32 — favicon
//    16 — favicon
//
// Sharp is sourced from dashboard/node_modules so we don't add a new dep at the repo root.

import { readdir, readFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "..");
const SVG_DIR = join(HERE, "avatars");
const PNG_DIR = join(HERE, "avatars", "png");
const SIZES = [500, 256, 128, 64, 32, 16];

const dashboardRequire = createRequire(join(REPO_ROOT, "dashboard", "package.json"));
const sharp = dashboardRequire("sharp");

async function rasterize() {
  const entries = await readdir(SVG_DIR);
  const svgs = entries.filter((name) => name.endsWith(".svg"));

  if (svgs.length === 0) {
    console.error(`No .svg files found in ${SVG_DIR}`);
    process.exit(1);
  }

  for (const file of svgs) {
    const base = file.replace(/\.svg$/, "");
    const buf = await readFile(join(SVG_DIR, file));

    for (const size of SIZES) {
      const out = join(PNG_DIR, `${base}-${size}.png`);
      await sharp(buf, { density: 400 })
        .resize(size, size, { fit: "contain", background: { r: 8, g: 8, b: 26, alpha: 1 } })
        .png({ compressionLevel: 9 })
        .toFile(out);
      console.log(`wrote ${out}`);
    }
  }
}

rasterize().catch((err) => {
  console.error(err);
  process.exit(1);
});
