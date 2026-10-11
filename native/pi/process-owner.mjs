import { spawn, spawnSync } from 'node:child_process';
import * as fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { fileURLToPath } from 'node:url';
export const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
export function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true, mode: 0o700 });
  const temporary = `${file}.${process.pid}.${randomUUID()}.tmp`;
  fs.writeFileSync(temporary, `${JSON.stringify(value)}\n`, { mode: 0o600 });
  fs.renameSync(temporary, file);
}
function inspectionEnv() {
  const { BRAID_NATIVE_EXECUTION_ID: _execution, BRAID_NATIVE_START_ID: _start, BRAID_NATIVE_LAUNCH_START_ID: _launch, ...env } = process.env;
  return { ...env, LC_ALL: 'C' };
}
function command(args) {
  const result = spawnSync(args[0], args.slice(1), { encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, env: inspectionEnv() });
  if (result.error || result.status !== 0) throw result.error ?? new Error(`${args[0]}: ${result.stderr.trim()}`);
  return result.stdout;
}
export function bootIdentity() {
  if (process.platform === 'linux') return fs.readFileSync('/proc/sys/kernel/random/boot_id', 'utf8').trim();
  if (process.platform === 'darwin') {
    const identity = command(['/usr/sbin/sysctl', '-n', 'kern.bootsessionuuid']).trim();
    if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(identity)) throw new Error('macOS boot session identity is unavailable.');
    return identity.toLowerCase();
  }
  throw new Error(`Native ownership is unsupported on ${process.platform}.`);
}
export function processIdentity(pid) {
  if (process.platform === 'linux') {
    try {
      const raw = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
      const fields = raw.slice(raw.lastIndexOf(')') + 2).split(' ');
      return { pid, ppid: Number(fields[1]), pgid: Number(fields[2]), starttime: fields[19], state: fields[0] };
    } catch (error) { if (['ENOENT', 'ESRCH'].includes(error.code)) return undefined; throw error; }
  }
  const result = spawnSync('/bin/ps', ['-p', String(pid), '-o', 'pid=,ppid=,pgid=,lstart=,state='], { encoding: 'utf8', env: inspectionEnv() });
  if (result.error) throw result.error;
  if (result.status === 1 && !result.stdout.trim()) return undefined;
  if (result.status !== 0) throw new Error(`ps identity failed: ${result.stderr.trim()}`);
  const row = result.stdout.trim().match(/^(\d+)\s+(\d+)\s+(\d+)\s+(.+?)\s+(\S+)$/);
  if (!row) throw new Error(`Cannot parse process identity for ${pid}.`);
  return { pid: Number(row[1]), ppid: Number(row[2]), pgid: Number(row[3]), starttime: row[4], state: row[5] };
}
export function processMarkers(pid) {
  let raw;
  if (process.platform === 'linux') raw = fs.readFileSync(`/proc/${pid}/environ`, 'utf8').replaceAll('\0', ' ');
  else raw = command(['/bin/ps', 'eww', '-p', String(pid), '-o', 'command=']);
  // Only these random ownership markers leave the environment inspection.
  return { owner: raw.match(/(?:^|\s)BRAID_NATIVE_EXECUTION_ID=([^\s]+)/)?.[1], startId: raw.match(/(?:^|\s)BRAID_NATIVE_LAUNCH_START_ID=([^\s]+)/)?.[1] ?? raw.match(/(?:^|\s)BRAID_NATIVE_START_ID=([^\s]+)/)?.[1] };
}
export function processSnapshot() {
  if (process.platform === 'darwin') {
    const snapshot = spawnSync('/bin/ps', ['eww', '-axo', 'pid=,ppid=,pgid=,lstart=,state=,command='], { encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, env: inspectionEnv() });
    if (snapshot.error || snapshot.status !== 0) throw snapshot.error ?? new Error(`ps snapshot: ${snapshot.stderr.trim()}`);
    return snapshot.stdout.split('\n').filter(Boolean).flatMap(raw => {
      const row = raw.trim().match(/^(\d+)\s+(\d+)\s+(\d+)\s+(.+?\d{4})\s+(\S+)\s+(.*)$/);
      if (!row) throw new Error('Cannot parse macOS process ownership snapshot.');
      if (/^[ZX]/.test(row[5]) || Number(row[1]) === snapshot.pid) return [];
      const command = row[6];
      return [{ pid: Number(row[1]), ppid: Number(row[2]), pgid: Number(row[3]), starttime: row[4], state: row[5], owner: command.match(/(?:^|\s)BRAID_NATIVE_EXECUTION_ID=([^\s]+)/)?.[1], startId: command.match(/(?:^|\s)BRAID_NATIVE_LAUNCH_START_ID=([^\s]+)/)?.[1] ?? command.match(/(?:^|\s)BRAID_NATIVE_START_ID=([^\s]+)/)?.[1] }];
    });
  }
  const ids = process.platform === 'linux' ? fs.readdirSync('/proc').filter(name => /^\d+$/.test(name)).map(Number) : command(['/bin/ps', '-axo', 'pid=']).trim().split(/\s+/).map(Number);
  const result = [];
  for (const pid of ids) {
    const identity = processIdentity(pid);
    if (!identity || /^[ZX]/.test(identity.state)) continue;
    let ownership = {};
    try { ownership = processMarkers(pid); }
    catch (error) {
      if (!processIdentity(pid)) continue;
      ownership = { marker_error: error.message };
    }
    // Recheck birth after reading markers; a reused PID is never associated.
    const after = processIdentity(pid);
    if (after && after.starttime === identity.starttime) result.push({ ...identity, ...ownership });
  }
  return result;
}
export async function withAdmission(directory, operation) {
  fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  const lock = path.join(directory, 'admission.lock');
  const deadline = Date.now() + 10000;
  for (;;) {
    try { fs.mkdirSync(lock, { mode: 0o700 }); break; }
    catch (error) { if (error.code !== 'EEXIST') throw error; if (Date.now() >= deadline) throw new Error('Native admission lock unavailable; refusing stale lock takeover.'); await pause(10); }
  }
  try { return await operation(); } finally { fs.rmdirSync(lock); }
}
async function launch(args) {
  const divider = args.indexOf('--');
  if (divider < 0 || !args[divider + 1]) throw new Error('launch requires -- COMMAND ARGS');
  const value = name => args[args.indexOf(name) + 1];
  const directory = process.env.BRAID_NATIVE_EXECUTION_DIR;
  const id = process.env.BRAID_NATIVE_EXECUTION_ID;
  const startId = value('--start-id');
  if (!directory || !id || !startId || !/^[a-zA-Z0-9-]+$/.test(startId)) throw new Error('Native launch identity unavailable.');
  const receipt = path.join(directory, 'starts', `${startId}.json`);
  let child;
  let closed;
  await withAdmission(directory, async () => {
    let parent = process.env.BRAID_NATIVE_START_ID;
    let fenced = fs.existsSync(path.join(directory, 'stopping.json')) || fs.existsSync(path.join(directory, 'fences', `${startId}.json`));
    const visited = new Set();
    while (parent && !fenced) {
      if (visited.has(parent)) throw new Error('Native ownership parent cycle.');
      visited.add(parent);
      fenced = fs.existsSync(path.join(directory, 'fences', `${parent}.json`));
      const parentRecord = JSON.parse(fs.readFileSync(path.join(directory, 'processes', `${parent}.json`), 'utf8'));
      if (parentRecord.execution_id !== id) throw new Error('Native parent execution identity mismatch.');
      parent = parentRecord.parent_start_id;
    }
    if (fenced) {
      writeJson(receipt, { status: 'start_rejected', reason: 'execution_stopping', execution_id: id, start_id: startId });
      throw new Error('Execution admission fenced.');
    }
    const identity = processIdentity(process.pid);
    if (!identity || identity.pgid !== process.pid) throw new Error('Native launcher requires its own detached process group.');
    const recordPath = path.join(directory, 'processes', `${startId}.json`);
    if (fs.existsSync(recordPath) || fs.existsSync(receipt)) throw new Error('Native start identity cannot be reused.');
    const record = { schema_version: 1, execution_id: id, start_id: startId, parent_start_id: process.env.BRAID_NATIVE_START_ID ?? null, boot_id: bootIdentity(), kind: args.includes('--kind') ? value('--kind') : 'tool', service: args.includes('--service'), ...identity, created_at: Date.now() };
    if (args.includes('--manifest')) record.manifest_path = value('--manifest');
    writeJson(recordPath, record);
    const { BRAID_NATIVE_LAUNCH_START_ID: _launcherMarker, ...payloadEnv } = process.env;
    child = spawn(args[divider + 1], args.slice(divider + 2), { stdio: 'inherit', env: { ...payloadEnv, BRAID_NATIVE_START_ID: startId } });
    // Retain close before any asynchronous startup observation, including tiny commands.
    closed = new Promise(resolve => { child.once('error', error => resolve({ error: error.message })); child.once('close', (code, signal) => resolve({ code, signal })); });
    await new Promise((resolve, reject) => { child.once('spawn', resolve); child.once('error', reject); });
    const payloadIdentity = processIdentity(child.pid);
    if (payloadIdentity) {
      if (payloadIdentity.pgid !== record.pgid) throw new Error('Payload escaped its registered process group during startup.');
      record.payload_identity = payloadIdentity;
      writeJson(recordPath, record);
    }
    writeJson(receipt, { status: 'started', execution_id: id, start_id: startId, process_record: recordPath, pid: process.pid, payload_pid: child.pid, observed_at: Date.now() });
  });
  for (const signal of ['SIGTERM', 'SIGINT']) process.on(signal, () => { try { child.kill(signal); } catch {} });
  const close = await closed;
  writeJson(path.join(directory, 'closes', `${startId}.json`), { execution_id: id, start_id: startId, ...close, observed_at: Date.now() });
  if (close.error) { console.error(close.error); process.exitCode = 126; }
  else if (close.signal) {
    process.removeAllListeners(close.signal);
    process.kill(process.pid, close.signal);
  } else process.exitCode = close.code ?? 128;
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv[2] !== 'launch') throw new Error('Usage: process-owner.mjs launch --start-id ID -- COMMAND ARGS');
  try { await launch(process.argv.slice(3)); } catch (error) {
    const args = process.argv.slice(3);
    const startId = args[args.indexOf('--start-id') + 1];
    const directory = process.env.BRAID_NATIVE_EXECUTION_DIR;
    if (directory && /^[a-zA-Z0-9-]+$/.test(startId ?? '')) {
      const receipt = path.join(directory, 'starts', `${startId}.json`);
      if (!fs.existsSync(receipt)) writeJson(receipt, { status: 'start_failed', execution_id: process.env.BRAID_NATIVE_EXECUTION_ID, start_id: startId, reason: error.message, observed_at: Date.now() });
    }
    console.error(error.message); process.exitCode = 75;
  }
}
