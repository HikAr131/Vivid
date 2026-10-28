import { createRequire } from 'node:module';
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { createHash } from 'node:crypto';

const root = fileURLToPath(new URL('../', import.meta.url));
const args = process.argv.slice(2);
const moduleIndex = args.indexOf('--module-dir');
const moduleDirectory = moduleIndex >= 0 ? args[moduleIndex + 1] : undefined;
const require = createRequire(moduleDirectory ? path.resolve(moduleDirectory, '__vivid__.cjs') : import.meta.url);
const sharp = require('sharp');
const check = args.includes('--check');
const hash = value => createHash('sha256').update(value).digest('hex');
const folder = path.join(root, 'crates/vivid-gui/assets/social-icons');
const manifest = { renderer: `sharp ${sharp.versions.sharp} / librsvg ${sharp.versions.rsvg}`, size: 96, icons: [] };

for (const name of ['douyin', 'bilibili', 'xiaohongshu']) {
  const source = await readFile(path.join(folder, `${name}.svg`));
  const svg = source.toString('utf8');
  if (!svg.includes('fill="currentColor"')) throw new Error(`${name}: missing currentColor`);
  // Every SVG is rendered in its own document, so repeated masks cannot collide.
  const white = svg.replace('<svg ', '<svg color="#ffffff" ');
  const png = await sharp(Buffer.from(white), { density: 288 })
    .resize(96, 96).png({ compressionLevel: 9, adaptiveFiltering: false, palette: false }).toBuffer();
  const destination = path.join(folder, `${name}.png`);
  if (check) {
    if (!(await readFile(destination)).equals(png)) throw new Error(`${name}: stale PNG`);
  } else {
    await writeFile(destination, png);
  }
  manifest.icons.push({ name, svg_sha256: hash(source), png_sha256: hash(png) });
}
const bytes = Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`);
const manifestPath = path.join(folder, 'render-manifest.json');
if (check) {
  if (!(await readFile(manifestPath)).equals(bytes)) throw new Error('Stale icon manifest');
} else {
  await writeFile(manifestPath, bytes);
}
console.log(`${check ? 'Checked' : 'Rendered'} 3 isolated monochrome SVG icons`);
