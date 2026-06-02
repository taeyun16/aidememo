#!/usr/bin/env node

import fs from 'node:fs';
import path from 'node:path';

const napiDir = process.argv[2];
const nextVersion = process.argv[3] || '';

if (!napiDir) {
  console.error('usage: aidememo_napi_version.mjs <crates/aidememo-napi> [semver]');
  process.exit(1);
}

const rootPath = path.join(napiDir, 'package.json');
const npmDir = path.join(napiDir, 'npm');

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function writeJson(file, value) {
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

function packageDirs() {
  return fs
    .readdirSync(npmDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => path.join(npmDir, entry.name))
    .sort();
}

function assert(condition, message) {
  if (!condition) {
    throw new Error(message);
  }
}

const root = readJson(rootPath);
const dirs = packageDirs();
const platformPackages = dirs.map((dir) => {
  const file = path.join(dir, 'package.json');
  return { dir, file, pkg: readJson(file) };
});

if (nextVersion) {
  root.version = nextVersion;
  root.optionalDependencies ||= {};
  for (const { pkg } of platformPackages) {
    root.optionalDependencies[pkg.name] = nextVersion;
  }
  writeJson(rootPath, root);

  for (const { file, pkg } of platformPackages) {
    pkg.version = nextVersion;
    writeJson(file, pkg);
  }
}

const currentRoot = readJson(rootPath);
const currentPlatforms = packageDirs().map((dir) => {
  const file = path.join(dir, 'package.json');
  return { dir, file, pkg: readJson(file) };
});

const optionalDeps = currentRoot.optionalDependencies || {};
const platformNames = currentPlatforms.map(({ pkg }) => pkg.name).sort();
const optionalNames = Object.keys(optionalDeps).sort();

assert(
  JSON.stringify(platformNames) === JSON.stringify(optionalNames),
  `optionalDependencies mismatch: expected ${platformNames.join(', ')} got ${optionalNames.join(', ')}`,
);

for (const { dir, pkg } of currentPlatforms) {
  assert(
    pkg.version === currentRoot.version,
    `${pkg.name} version ${pkg.version} does not match root ${currentRoot.version}`,
  );
  assert(
    optionalDeps[pkg.name] === currentRoot.version,
    `root optionalDependencies.${pkg.name}=${optionalDeps[pkg.name]} does not match ${currentRoot.version}`,
  );
  assert(
    pkg.publishConfig?.access === 'public',
    `${pkg.name} must set publishConfig.access=public`,
  );
  assert(
    Array.isArray(pkg.files) && pkg.files.length === 1 && pkg.files[0].endsWith('.node'),
    `${pkg.name} must publish exactly one .node file`,
  );
  assert(
    pkg.main === pkg.files[0],
    `${pkg.name} main must match files[0] (${pkg.files[0]})`,
  );
  const expectedDirectory = `crates/aidememo-napi/npm/${pkg.name}`;
  assert(
    pkg.repository?.directory === expectedDirectory,
    `${pkg.name} repository.directory must be ${expectedDirectory}`,
  );
}

assert(
  Array.isArray(currentRoot.files) &&
    currentRoot.files.includes('index.js') &&
    currentRoot.files.includes('index.d.ts') &&
    !currentRoot.files.some((item) => item.endsWith('.node') || item === '*.node'),
  'root package files must include JS/types and exclude .node binaries',
);
assert(
  currentRoot.publishConfig?.access === 'public',
  'root package must set publishConfig.access=public',
);

console.log(
  `OK: aidememo-napi versions pinned at ${currentRoot.version} across ${currentPlatforms.length} platform packages`,
);
