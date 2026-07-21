const fs = require('fs');
const path = require('path');
const { spawnSync } = require('child_process');

const root = path.join(__dirname, '..');
const packagePath = path.join(root, 'package.json');
const packageLockPath = path.join(root, 'package-lock.json');
const cargoManifestPath = path.join(root, 'src-tauri', 'Cargo.toml');
const cargoLockPath = path.join(root, 'src-tauri', 'Cargo.lock');
const checkOnly = process.argv.includes('--check');

const packageJson = readJson(packagePath);
const version = packageJson.version;
const semverPattern = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;

if (!semverPattern.test(version)) {
  fail(`Invalid semantic version in package.json: ${version}`);
}

if (checkOnly) {
  verifyVersions(version);
  console.log(`Version metadata is consistent: ${version}`);
  process.exit(0);
}

const packageLock = readJson(packageLockPath);
packageLock.version = version;
packageLock.packages[''].version = version;
fs.writeFileSync(packageLockPath, JSON.stringify(packageLock, null, 2) + '\n');

const cargoManifest = fs.readFileSync(cargoManifestPath, 'utf8');
const updatedManifest = replacePackageVersion(cargoManifest, version, cargoManifestPath);
fs.writeFileSync(cargoManifestPath, updatedManifest);

const cargoResult = spawnSync(
  'cargo',
  ['metadata', '--manifest-path', cargoManifestPath, '--no-deps', '--format-version', '1'],
  { cwd: root, encoding: 'utf8' },
);
if (cargoResult.status !== 0) {
  process.stderr.write(cargoResult.stderr || 'Cargo metadata update failed.\n');
  process.exit(cargoResult.status || 1);
}

verifyVersions(version);
console.log(`Synchronized version metadata to ${version}`);

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, 'utf8'));
}

function replacePackageVersion(content, nextVersion, filePath) {
  const packageSection = /(^\[package\][\s\S]*?^version\s*=\s*")[^"]+("\s*$)/m;
  if (!packageSection.test(content)) {
    fail(`Package version not found in ${filePath}`);
  }
  return content.replace(packageSection, `$1${nextVersion}$2`);
}

function readCargoVersion(content, filePath, packageBlockPattern) {
  const match = content.match(packageBlockPattern);
  if (!match) {
    fail(`Package version not found in ${filePath}`);
  }
  return match[1];
}

function verifyVersions(expectedVersion) {
  const packageLock = readJson(packageLockPath);
  const cargoManifest = fs.readFileSync(cargoManifestPath, 'utf8');
  const cargoLock = fs.readFileSync(cargoLockPath, 'utf8');
  const cargoManifestVersion = readCargoVersion(
    cargoManifest,
    cargoManifestPath,
    /^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"\s*$/m,
  );
  const cargoLockVersion = readCargoVersion(
    cargoLock,
    cargoLockPath,
    /^\[\[package\]\]\s*\r?\nname\s*=\s*"csv-reader"\s*\r?\nversion\s*=\s*"([^"]+)"/m,
  );

  const versions = [
    ['package-lock.json', packageLock.version],
    ['package-lock.json root package', packageLock.packages[''].version],
    ['src-tauri/Cargo.toml', cargoManifestVersion],
    ['src-tauri/Cargo.lock', cargoLockVersion],
  ];
  const mismatches = versions.filter(([, current]) => current !== expectedVersion);
  if (mismatches.length > 0) {
    const details = mismatches
      .map(([file, current]) => `${file}: ${current}`)
      .join(', ');
    fail(`Version mismatch; expected ${expectedVersion}. Run npm run version:sync. ${details}`);
  }
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
