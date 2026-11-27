import test from 'node:test';
import assert from 'node:assert/strict';
import { preserveToolConfig } from '../tool-environment.mjs';

for (const [name, environment, expected] of [
  ['explicit gh directory takes precedence over XDG', { GH_CONFIG_DIR: '/operator/gh', XDG_CONFIG_HOME: '/operator/xdg' }, '/operator/gh'],
  ['original XDG directory selects gh configuration', { XDG_CONFIG_HOME: '/operator/xdg' }, '/operator/xdg/gh'],
  ['absent GH and XDG use the caller home', {}, '/operator/home/.config/gh'],
]) {
  test(name, () => {
    preserveToolConfig(environment, '/operator/home');
    assert.equal(environment.GH_CONFIG_DIR, expected);
  });
}
