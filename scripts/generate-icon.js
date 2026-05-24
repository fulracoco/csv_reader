// generate-icon.js — Converts build/icon.svg to build/icon.png (512x512)
// electron-builder uses this PNG to generate platform-specific icons (.ico, .icns)

const sharp = require('sharp');
const path = require('path');
const fs = require('fs');

const svgPath = path.join(__dirname, '..', 'build', 'icon.svg');
const pngPath = path.join(__dirname, '..', 'build', 'icon.png');

const svg = fs.readFileSync(svgPath, 'utf8');

sharp(Buffer.from(svg))
  .resize(512, 512)
  .png()
  .toFile(pngPath)
  .then((info) => {
    console.log('Icon generated:', pngPath, `(${info.width}x${info.height}, ${(info.size / 1024).toFixed(1)} KB)`);
  })
  .catch((err) => {
    console.error('Failed to generate icon:', err.message);
    process.exit(1);
  });
