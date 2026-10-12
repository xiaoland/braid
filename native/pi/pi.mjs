#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const directory = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const informational = args.includes('--version') || args.includes('-v') || args.includes('--help') || args.includes('-h');
if (informational) {
  const pi = await import('@earendil-works/pi-coding-agent');
  if (args.includes('--version') || args.includes('-v')) process.stdout.write(`${pi.VERSION}\n`);
  else process.stdout.write(`Pi coding agent\n\nUsage: pi [options] [messages...]\n\n  --mode <text|json|rpc>  Select output mode\n  -p, --print             Run one non-interactive prompt\n  -c, --continue          Continue the most recent session\n  -r, --resume            Resume the most recent session\n  --session <path|id>     Open a saved session\n  --session-dir <dir>     Set session storage\n  --model <model>         Select a model\n  --provider <provider>   Select a provider\n  --thinking <level>      Set reasoning level\n  -e, --extension <path>  Load an extension\n  --skill <path>          Load a skill\n  --no-extensions         Disable extension discovery\n  --no-skills             Disable skill discovery\n  -h, --help              Show this help\n  -v, --version           Show the Pi version\n`);
} else {
  // Native tools consume the configured environment, not ambient shell startup files.
  delete process.env.BASH_ENV;
  delete process.env.ENV;
  const manifest = JSON.parse(fs.readFileSync(path.join(directory, 'runtime-manifest.json'), 'utf8'));
  for (const name of ['BRAID_NATIVE_EXECUTION_DIR', 'BRAID_NATIVE_EXECUTION_ID', 'BRAID_NATIVE_START_ID']) {
    if (!process.env[name]) throw new Error(`${name} is required: launch this Pi runtime through Braid's native execution owner.`);
  }
  const nativeHome = process.env.PI_CODING_AGENT_DIR;
  if (!nativeHome || !path.isAbsolute(nativeHome)) throw new Error('PI_CODING_AGENT_DIR must be an absolute, persistent Braid native home.');
  for (const [name, relative] of Object.entries({ TMPDIR: 'tmp', XDG_CACHE_HOME: 'cache', XDG_DATA_HOME: 'data', XDG_CONFIG_HOME: 'config' })) {
    const value = path.join(process.env.BRAID_NATIVE_EXECUTION_DIR, relative);
    fs.mkdirSync(value, { recursive: true, mode: 0o700 });
    process.env[name] = value;
  }
  process.env.PBB_ROOT = path.join(nativeHome, 'pbb');
  process.env.PI_LANE_ROOT = path.join(nativeHome, 'pi-lane');
  process.env.BRAID_AGENT_RUNTIME = '1';
  process.env.BRAID_NATIVE_RUNTIME_MODULE = pathToFileURL(path.join(directory, manifest.managed_module.path)).href;
  process.env.PI_SUBAGENT_PI_BINARY = path.join(directory, 'pi');
  process.env.PI_CODING_AGENT_PACKAGE_ROOT = path.join(directory, 'node_modules/@earendil-works/pi-coding-agent');
  process.env.PATH = `${path.join(directory, 'bin')}${path.delimiter}${path.join(directory, 'node_modules/.bin')}${path.delimiter}${process.env.PATH ?? ''}`;
  process.env.BRAID_SUBAGENT_CATALOG = manifest.plugins.some(plugin => plugin.name === 'subagents') ? '1' : '0';
  process.env.BRAID_NATIVE_SUBAGENTS_ENABLED = manifest.plugins.some(plugin => plugin.name === 'subagents') ? '1' : '0';
  process.env.BRAID_NATIVE_MCPORTER_ENABLED = manifest.plugins.some(plugin => plugin.name === 'mcporter') ? '1' : '0';
  await import(process.env.BRAID_NATIVE_RUNTIME_MODULE);
  const selected = manifest.plugins.flatMap(plugin => plugin.extension_paths.flatMap(relative => ['-e', path.join(directory, relative)]));
  const skills = manifest.plugins.flatMap(plugin => plugin.skill_paths.flatMap(relative => ['--skill', path.join(directory, relative)]));
  process.argv.splice(2, 0, '--no-extensions', ...selected, ...skills);
  const sdk = await import('./sdk-host.mjs');
  await sdk.main();
}
