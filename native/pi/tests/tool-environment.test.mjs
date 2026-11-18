import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { preserveToolConfig } from '../tool-environment.mjs';

for (const [name, environment, expected] of [
  ['explicit gh directory wins', { GH_CONFIG_DIR: '/operator/gh', XDG_CONFIG_HOME: '/operator/xdg' }, '/operator/gh'],
  ['original XDG path survives execution isolation', { XDG_CONFIG_HOME: '/operator/xdg' }, '/operator/xdg/gh'],
  ['default home path survives execution isolation', {}, '/operator/home/.config/gh'],
]) {
  test(name, () => {
    preserveToolConfig(environment, '/operator/home');
    environment.XDG_CONFIG_HOME = '/execution/private/config';
    assert.equal(environment.GH_CONFIG_DIR, expected);
    // The same explicit locator reaches a git credential-helper subprocess.
    const actual = execFileSync(process.execPath, ['-e', 'process.stdout.write(process.env.GH_CONFIG_DIR)'], {
      env: { ...process.env, ...environment }, encoding: 'utf8',
    });
    assert.equal(actual, expected);
    assert.equal(environment.XDG_CONFIG_HOME, '/execution/private/config');
  });
}
