#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const nodeVersion = '24.15.0';
const nodeDistributions = {
  'darwin-arm64': ['tar.gz', '372331b969779ab5d15b949884fc6eaf88d5afe87bde8ba881d6400b9100ffc4'],
  'darwin-x64': ['tar.gz', 'ffd5ee293467927f3ee731a553eb88fd1f48cf74eebc2d74a6babe4af228673b'],
  'linux-arm64': ['tar.xz', 'f3d5a797b5d210ce8e2cb265544c8e482eaedcb8aa409a8b46da7e8595d0dda0'],
  'linux-x64': ['tar.xz', '472655581fb851559730c48763e0c9d3bc25975c59d518003fc0849d3e4ba0f6'],
};
const nodePlatform = `${process.platform}-${process.arch}`;
const nodeDistribution = nodeDistributions[nodePlatform];
if (!nodeDistribution) throw new Error(`No locked Node distribution for ${nodePlatform}.`);

const source = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const destination = args.shift();
if (!destination || (args.length && (args.length !== 2 || args[0] !== '--plugins'))) {
  throw new Error('Usage: node install.mjs ABSOLUTE_INSTALL_DIRECTORY [--plugins subagents,background,context7,fff,mcporter]');
}
if (!path.isAbsolute(destination)) throw new Error('Install directory must be absolute.');
const selected = args.length ? args[1].split(',').filter(Boolean) : [];
const catalog = {
  subagents: { extension_paths: ['node_modules/pi-subagents/index.ts'], skill_paths: ['node_modules/pi-subagents/skills'] },
  background: { extension_paths: ['node_modules/pi-background-bash/extensions/background-bash.ts'], skill_paths: ['node_modules/pi-background-bash/skills'] },
  context7: { extension_paths: ['node_modules/@upstash/context7-pi/extensions/context7.ts'], skill_paths: ['node_modules/@upstash/context7-pi/skills'] },
  fff: { extension_paths: ['node_modules/@ff-labs/pi-fff/src/index.ts'], skill_paths: [] },
  mcporter: { extension_paths: [], skill_paths: [], executable_paths: ['node_modules/mcporter/dist/cli.js'] },
};
if (new Set(selected).size !== selected.length || selected.some(name => !Object.hasOwn(catalog, name))) throw new Error('Unknown or duplicate plugin selection.');
if (fs.existsSync(destination) && fs.readdirSync(destination).length) throw new Error('Install directory must be empty; retain the previous runtime and install a new immutable candidate.');
fs.mkdirSync(destination, { recursive: true });
for (const name of ['.cache', '.tmp', '.install-home']) fs.mkdirSync(path.join(destination, name), { recursive: true });
const env = { ...process.env, HOME: path.join(destination, '.install-home'), TMPDIR: path.join(destination, '.tmp'), npm_config_cache: path.join(destination, '.cache') };
const runtimeFiles = ['package.json', 'package-lock.json', 'pi', ...fs.readdirSync(source).filter(name => name.endsWith('.mjs') && name !== 'install.mjs')];
for (const name of runtimeFiles) fs.copyFileSync(path.join(source, name), path.join(destination, name));
for (const name of Object.keys(catalog)) {
  const directory = path.join(destination, 'plugins', name);
  fs.mkdirSync(directory, { recursive: true });
  fs.copyFileSync(path.join(source, 'plugins', name, 'package.json'), path.join(directory, 'package.json'));
}
fs.chmodSync(path.join(destination, 'pi'), 0o755);
fs.mkdirSync(path.join(destination, 'bin'));
const sha = filename => crypto.createHash('sha256').update(fs.readFileSync(filename)).digest('hex');
// Host Node may link Homebrew libraries; deliver the official standalone binary instead.
const nodeDirectory = `node-v${nodeVersion}-${nodePlatform}`;
const nodeArchive = `${nodeDirectory}.${nodeDistribution[0]}`;
const nodeUrl = `https://nodejs.org/dist/v${nodeVersion}/${nodeArchive}`;
const archivePath = path.join(destination, '.cache', nodeArchive);
execFileSync('curl', ['--fail', '--location', '--silent', '--show-error', '--connect-timeout', '30', '--max-time', '300', '--output', archivePath, nodeUrl], { env, stdio: 'inherit' });
if (sha(archivePath) !== nodeDistribution[1]) throw new Error(`Node distribution SHA256 mismatch: ${nodeArchive}.`);
execFileSync('tar', ['-xf', archivePath, '-C', path.join(destination, '.tmp'), `${nodeDirectory}/bin/node`, `${nodeDirectory}/LICENSE`], { env, stdio: 'inherit' });
fs.copyFileSync(path.join(destination, '.tmp', nodeDirectory, 'bin/node'), path.join(destination, 'bin/node'));
fs.copyFileSync(path.join(destination, '.tmp', nodeDirectory, 'LICENSE'), path.join(destination, 'bin/node-LICENSE'));
fs.chmodSync(path.join(destination, 'bin/node'), 0o755);
const installedNodeVersion = execFileSync(path.join(destination, 'bin/node'), ['--version'], { env, encoding: 'utf8' }).trim();
if (installedNodeVersion !== `v${nodeVersion}`) throw new Error(`Installed Node did not report v${nodeVersion}: ${installedNodeVersion}.`);
const workspaceArgs = selected.length
  ? ['--include-workspace-root', ...selected.map(name => `--workspace=plugins/${name}`)]
  : ['--workspaces=false'];
execFileSync('npm', ['ci', '--ignore-scripts', '--no-audit', '--no-fund', ...workspaceArgs], { cwd: destination, env, stdio: 'inherit' });
// Workspace wrappers select dependencies at installation; they are not runtime packages.
for (const name of Object.keys(catalog)) {
  const link = path.join(destination, 'node_modules', '@braid', `pi-runtime-${name}`);
  if (fs.existsSync(link)) fs.unlinkSync(link);
}
const patches = JSON.parse(fs.readFileSync(path.join(source, 'patches.json'), 'utf8'))
  .filter(item => !item.plugin || selected.includes(item.plugin));
for (const item of patches) {
  execFileSync('patch', ['--batch', '--fuzz=0', '-p1', '-d', path.join(destination, 'node_modules', item.package), '-i', path.join(source, item.path)], { env, stdio: 'inherit' });
}
const files = {};
function record(directory, relative = '') {
  for (const name of fs.readdirSync(directory).sort()) {
    const filename = path.join(directory, name);
    const entry = path.join(relative, name);
    const stat = fs.lstatSync(filename);
    if (stat.isDirectory()) record(filename, entry);
    else if (stat.isSymbolicLink()) {
      const canonical = fs.realpathSync(filename);
      const within = path.relative(fs.realpathSync(destination), canonical);
      if (within === '..' || within.startsWith(`..${path.sep}`) || path.isAbsolute(within)) throw new Error(`Runtime symlink leaves installation: ${entry}`);
      if (!fs.statSync(canonical).isFile()) throw new Error(`Runtime directory symlink is unsupported: ${entry}`);
      files[entry] = sha(canonical);
    }
    else if (stat.isFile()) files[entry] = sha(filename);
  }
}
record(path.join(destination, 'node_modules'), 'node_modules');
record(path.join(destination, 'bin'), 'bin');
for (const name of runtimeFiles) files[name] = sha(path.join(destination, name));
record(path.join(destination, 'plugins'), 'plugins');
const member = relative => ({ path: relative, sha256: files[relative] });
const manifest = {
  schema_version: 1,
  pi_version: JSON.parse(fs.readFileSync(path.join(destination, 'node_modules/@earendil-works/pi-coding-agent/package.json'))).version,
  capabilities: ['retry_intent', 'settle_extensions', 'owned_inference', 'managed_execution', 'owned_stop'],
  entrypoint: member('pi'),
  managed_module: member('managed.mjs'),
  process_owner: member('process-owner.mjs'),
  node: member('bin/node'),
  node_distribution: { version: nodeVersion, platform: process.platform, arch: process.arch, url: nodeUrl, archive_sha256: nodeDistribution[1] },
  plugins: selected.map(name => ({ name, executable_paths: [], ...catalog[name] })),
  npm_lock_sha256: files['package-lock.json'],
  packages: Object.fromEntries([
    ...Object.keys(JSON.parse(fs.readFileSync(path.join(source, 'package.json'))).dependencies),
    ...selected.flatMap(name => Object.keys(JSON.parse(fs.readFileSync(path.join(source, 'plugins', name, 'package.json'))).dependencies)),
  ].map(name => [name, JSON.parse(fs.readFileSync(path.join(destination, 'node_modules', name, 'package.json'))).version])),
  patches: patches.map(({ package: name, path: relative }) => ({ package: name, path: relative, sha256: sha(path.join(source, relative)) })),
  files,
};
fs.writeFileSync(path.join(destination, 'runtime-manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`, { flag: 'wx' });
console.log(JSON.stringify({ manifest: path.join(destination, 'runtime-manifest.json'), entrypoint: path.join(destination, 'pi'), plugins: selected, files: Object.keys(files).length }));
