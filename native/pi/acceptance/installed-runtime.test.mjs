import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { once } from 'node:events';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import readline from 'node:readline';
import test from 'node:test';

// Install once with --plugins background, then run against that candidate.
// This is a distribution test; the regular source tests need no npm install.
test('installed background-only Pi opens a fresh session, serves tools and closes the execution', async t => {
  const candidate = process.env.BRAID_TEST_PI_INSTALL;
  assert.ok(candidate && path.isAbsolute(candidate), 'set BRAID_TEST_PI_INSTALL to an absolute background-only installation');
  const install = fs.realpathSync(candidate);
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), 'braid-pi-smoke-')));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const agentDir = path.join(root, 'pi-home');
  fs.mkdirSync(agentDir);
  fs.writeFileSync(path.join(agentDir, 'models.json'), JSON.stringify({
    providers: { fixture: { baseUrl: 'http://127.0.0.1:1/v1', api: 'openai-completions', models: [{ id: 'offline' }] } },
  }));
  const id = randomUUID();
  const start = randomUUID();
  const directory = path.join(root, 'execution');
  const session = path.join(root, 'fresh.jsonl');
  const env = {
    PATH: '/usr/bin:/bin', HOME: root, PI_OFFLINE: '1', PI_CODING_AGENT_DIR: agentDir,
    XDG_CONFIG_HOME: path.join(root, 'operator-config'),
    BRAID_NATIVE_EXECUTION_DIR: directory, BRAID_NATIVE_EXECUTION_ID: id, BRAID_NATIVE_LAUNCH_START_ID: start,
  };
  const pi = path.join(install, 'pi');
  const node = path.join(install, 'bin/node');
  assert.equal(execFileSync(pi, ['--version'], { cwd: root, env, encoding: 'utf8', timeout: 30000 }).trim(), '1.1.0');
  const child = spawn(node, [
    path.join(install, 'process-owner.mjs'), 'launch', '--start-id', start, '--kind', 'root', '--',
    pi, '--mode', 'rpc', '--session', session, '--provider', 'fixture', '--model', 'offline',
  ], { cwd: root, env, detached: true, timeout: 30000 });
  const closed = once(child, 'close');
  let stderr = '';
  child.stderr.setEncoding('utf8').on('data', text => { stderr += text; });
  const responses = readline.createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  const request = async (type, fields = {}) => {
    child.stdin.write(JSON.stringify({ id: type, type, ...fields }) + '\n');
    for (;;) {
      const line = await responses.next();
      assert.equal(line.done, false, `Pi exited before ${type}: ${stderr}`);
      const reply = JSON.parse(line.value);
      if (reply.type !== 'response') continue;
      assert.equal(reply.id, type);
      assert.equal(reply.command, type);
      assert.equal(reply.success, true, JSON.stringify(reply));
      return reply.data;
    }
  };
  try {
    const state = await request('get_state');
    assert.equal(state.sessionFile, session);
    assert.equal(state.model.provider, 'fixture');
    assert.equal(state.model.id, 'offline');
    assert.equal(state.managed_state.execution_id, id);
    assert.equal(state.managed_state.status, 'quiescent');
    assert.equal(state.braid_native_runtime.schema_version, 1);
    assert.equal(state.braid_native_runtime.pi_version, '1.1.0');
    for (const required of ['retry_intent', 'settle_extensions', 'owned_inference', 'managed_execution', 'owned_stop']) {
      assert.ok(state.braid_native_runtime.capabilities.includes(required), `missing RPC capability ${required}`);
    }
    assert.equal(state.braid_native_runtime.execution_id, id);
    assert.deepEqual(state.braid_native_runtime.extension_errors, []);
    assert.deepEqual(state.braid_native_runtime.extensions, [
      path.join(install, 'node_modules/pi-background-bash/extensions/background-bash.ts'),
    ]);
    const bash = await request('bash', { command: 'printf "%s\n%s" "$GH_CONFIG_DIR" "$XDG_CONFIG_HOME"', excludeFromContext: true });
    assert.equal(bash.exitCode, 0, JSON.stringify(bash));
    assert.equal(bash.output, [path.join(root, 'operator-config/gh'), path.join(directory, 'config')].join('\n'));
    const handoff = await request('yield_stoppable_services');
    assert.equal(handoff.managed_state.status, 'quiescent');
    assert.equal(handoff.managed_state.execution_id, id);
    const receipt = await request('stop_owned_execution');
    assert.equal(receipt.status, 'stopped', JSON.stringify(receipt));
    assert.equal(receipt.execution_id, id);
    assert.equal(receipt.scope, 'root-descendants');
  } finally {
    child.stdin.end();
    const [code, signal] = await closed;
    assert.equal(signal, null, stderr);
    assert.equal(code, 0, stderr);
  }
  // Read the final receipt produced from persisted registrations after EOF.
  const output = execFileSync(node, [
    path.join(install, 'managed.mjs'), 'cleanup', '--execution-dir', directory, '--execution-id', id,
  ], { cwd: root, env: { HOME: root, PATH: env.PATH }, encoding: 'utf8', timeout: 30000 });
  const offline = JSON.parse(output);
  assert.equal(offline.status, 'stopped', output);
  assert.equal(offline.execution_id, id);
  assert.equal(offline.scope, 'execution');
  assert.equal(offline.coverage, 'registered-process-groups-and-inherited-execution-markers');
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(directory, 'execution-stop.json'), 'utf8')), offline);
});
