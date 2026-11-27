import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { once } from 'node:events';
import { spawnSync } from 'node:child_process';
import { processIdentity, processMarkers } from '../process-owner.mjs';

// A test launched by Braid may inherit its operator's execution markers. Do
// not let module initialization allocate a socket alias in that execution.
const inheritedMcporter = process.env.BRAID_NATIVE_MCPORTER_ENABLED;
delete process.env.BRAID_NATIVE_MCPORTER_ENABLED;
let managed;
try { managed = await import('../managed.mjs'); }
finally {
  if (inheritedMcporter !== undefined) process.env.BRAID_NATIVE_MCPORTER_ENABLED = inheritedMcporter;
}
const { spawnManaged, waitManagedStartup, finishManagedProcess, cleanupOwnedExecution } = managed;

function execution(t) {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'braid-managed-'));
  const id = randomUUID();
  const keys = ['BRAID_NATIVE_EXECUTION_DIR', 'BRAID_NATIVE_EXECUTION_ID', 'BRAID_NATIVE_START_ID', 'BRAID_NATIVE_LAUNCH_START_ID'];
  const original = Object.fromEntries(keys.map(key => [key, process.env[key]]));
  process.env.BRAID_NATIVE_EXECUTION_DIR = directory;
  process.env.BRAID_NATIVE_EXECUTION_ID = id;
  delete process.env.BRAID_NATIVE_START_ID;
  delete process.env.BRAID_NATIVE_LAUNCH_START_ID;
  const children = [];
  const owned = [];
  const track = pid => {
    const identity = processIdentity(pid);
    if (identity) owned.push(identity);
  };
  t.after(async () => {
    try {
      const receipt = await cleanupOwnedExecution({ directory, id });
      // Test teardown must also recover a broken cleanup implementation.
      // Identity helpers are used only for emergency cleanup, never as the
      // oracle that decides whether the production stop was correct.
      for (const identity of owned) {
        const current = processIdentity(identity.pid);
        if (current?.starttime === identity.starttime && !/^[ZX]/.test(current.state)) {
          if (processMarkers(identity.pid).owner !== id) continue;
          try { process.kill(identity.pid, 'SIGKILL'); }
          catch (error) { if (error.code !== 'ESRCH') throw error; }
        }
      }
      await Promise.all(children.map(child => child.managedClose));
      assert.equal(receipt.status, 'stopped', JSON.stringify(receipt));
    } finally {
      for (const key of keys) {
        if (original[key] === undefined) delete process.env[key];
        else process.env[key] = original[key];
      }
      fs.rmSync(directory, { recursive: true, force: true });
    }
  });
  return {
    directory, id, track,
    spawn(code) {
      const child = spawnManaged(process.execPath, ['-e', code], { stdio: ['ignore', 'pipe', 'ignore'] });
      children.push(child);
      if (child.pid) track(child.pid);
      return child;
    },
  };
}

function assertNotExecuting(pid) {
  // Independent OS observation: do not reuse managed.mjs's process selector
  // as the expected value for its own stop decision. Zombies cannot execute.
  const observed = spawnSync('/bin/ps', ['-p', String(pid), '-o', 'stat='], { encoding: 'utf8' });
  assert.equal(observed.error, undefined);
  assert.ok(observed.status === 0 || observed.status === 1, observed.stderr);
  assert.ok(!observed.stdout.trim() || /^[ZX]/.test(observed.stdout.trim()), `PID ${pid} still executes: ${observed.stdout}`);
}

test('owned stop reaches a real grandchild and fences later launches', { timeout: 20000 }, async t => {
  const fixture = execution(t);
  const child = fixture.spawn(`
    const { spawn } = require('node:child_process');
    const leaf = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' });
    process.stdout.write(String(leaf.pid) + '\\n');
    setInterval(() => {}, 1000);
  `);
  const output = once(child.stdout, 'data', { signal: AbortSignal.timeout(5000) });
  const started = await waitManagedStartup(child, 5000);
  const leaf = Number(String((await output)[0]).trim());
  assert.ok(Number.isInteger(leaf) && leaf > 1);
  fixture.track(started.payload_pid);
  fixture.track(leaf);
  assert.doesNotThrow(() => process.kill(leaf, 0));
  const terminal = await finishManagedProcess(child, { observation: 'test_stop', observed_at: Date.now() });
  assert.equal(terminal.status, 'stopped', JSON.stringify(terminal));
  assertNotExecuting(started.payload_pid);
  assertNotExecuting(leaf);
  await child.managedClose;
  const persisted = JSON.parse(fs.readFileSync(path.join(fixture.directory, 'terminals', `${started.start_id}.json`), 'utf8'));
  assert.equal(persisted.execution_id, fixture.id);
  assert.equal(persisted.terminal.status, 'stopped');

  const stopped = await cleanupOwnedExecution({ directory: fixture.directory, id: fixture.id });
  assert.equal(stopped.status, 'stopped', JSON.stringify(stopped));
  const marker = path.join(fixture.directory, 'should-not-launch');
  const rejected = fixture.spawn(`require('node:fs').writeFileSync(${JSON.stringify(marker)}, 'ran')`);
  await assert.rejects(waitManagedStartup(rejected, 5000), /start_rejected.*execution_stopping/);
  await rejected.managedClose;
  assert.equal(fs.existsSync(marker), false);
});

test('ambiguous persisted ownership reports unknown without killing the payload', { timeout: 20000 }, async t => {
  const fixture = execution(t);
  const child = fixture.spawn('setInterval(() => {}, 1000)');
  const started = await waitManagedStartup(child, 5000);
  fixture.track(started.payload_pid);
  const original = fs.readFileSync(started.process_record, 'utf8');
  const record = JSON.parse(original);
  fs.writeFileSync(started.process_record, JSON.stringify({ ...record, boot_id: 'different-boot' }));
  try {
    const result = await cleanupOwnedExecution({ directory: fixture.directory, id: fixture.id });
    assert.equal(result.status, 'unknown');
    assert.match(result.reason, /another boot identity/);
    assert.doesNotThrow(() => process.kill(started.payload_pid, 0));
    const persisted = JSON.parse(fs.readFileSync(path.join(fixture.directory, 'execution-stop.json'), 'utf8'));
    assert.equal(persisted.status, 'unknown');
  } finally {
    // Restore the actual launcher's identity before teardown performs real stop.
    fs.writeFileSync(started.process_record, original);
  }
});
