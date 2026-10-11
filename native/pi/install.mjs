#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

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
for (const name of ['package.json', 'package-lock.json', 'pi', 'pi.mjs', 'managed.mjs', 'process-owner.mjs']) fs.copyFileSync(path.join(source, name), path.join(destination, name));
fs.chmodSync(path.join(destination, 'pi'), 0o755);
fs.mkdirSync(path.join(destination, 'bin'));
fs.copyFileSync(process.execPath, path.join(destination, 'bin/node'));
fs.chmodSync(path.join(destination, 'bin/node'), 0o755);
execFileSync('npm', ['ci', '--ignore-scripts', '--no-audit', '--no-fund'], { cwd: destination, env, stdio: 'inherit' });
const sha = filename => crypto.createHash('sha256').update(fs.readFileSync(filename)).digest('hex');
const patches = JSON.parse(fs.readFileSync(path.join(source, 'patches.json'), 'utf8'));
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
for (const name of ['package.json', 'package-lock.json', 'pi', 'pi.mjs', 'managed.mjs', 'process-owner.mjs']) files[name] = sha(path.join(destination, name));
const member = relative => ({ path: relative, sha256: files[relative] });
const manifest = {
  schema_version: 1,
  pi_version: '1.0.2',
  capabilities: ['retry_intent', 'settle_extensions', 'owned_inference', 'managed_execution', 'owned_stop'],
  entrypoint: member('pi'),
  managed_module: member('managed.mjs'),
  process_owner: member('process-owner.mjs'),
  node: member('bin/node'),
  plugins: selected.map(name => ({ name, executable_paths: [], ...catalog[name] })),
  npm_lock_sha256: files['package-lock.json'],
  packages: Object.fromEntries(Object.keys(JSON.parse(fs.readFileSync(path.join(source, 'package.json'))).dependencies).map(name => [name, JSON.parse(fs.readFileSync(path.join(destination, 'node_modules', name, 'package.json'))).version])),
  patches: patches.map(({ package: name, path: relative }) => ({ package: name, path: relative, sha256: sha(path.join(source, relative)) })),
  files,
};
fs.writeFileSync(path.join(destination, 'runtime-manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`, { flag: 'wx' });
console.log(JSON.stringify({ manifest: path.join(destination, 'runtime-manifest.json'), entrypoint: path.join(destination, 'pi'), plugins: selected, files: Object.keys(files).length }));
