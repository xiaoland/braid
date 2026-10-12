import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import * as fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { bootIdentity, processIdentity, processMarkers, processSnapshot, withAdmission, writeJson } from "./process-owner.mjs";
const providers = globalThis[Symbol.for("braid.native-managed-providers.v1")] ??= new Map();
// MCPorter services share only this execution's namespace, never the user daemon.
if (process.env.BRAID_NATIVE_MCPORTER_ENABLED === "1" && process.env.BRAID_NATIVE_EXECUTION_DIR && process.env.BRAID_NATIVE_EXECUTION_ID) {
  const directory = path.resolve(process.env.BRAID_NATIVE_EXECUTION_DIR, "mcporter");
  let namespace = directory;
  const limit = process.platform === "darwin" ? 104 : 108;
  if (Buffer.byteLength(path.join(directory, "daemon/user.sock")) >= limit) {
    const root = process.env.BRAID_NATIVE_SOCKET_ROOT;
    if (!root || !path.isAbsolute(root)) throw new Error(`Execution MCPorter Unix socket exceeds ${limit - 1} bytes; configure an absolute shorter BRAID_NATIVE_SOCKET_ROOT.`);
    fs.mkdirSync(root, { recursive: true, mode: 0o700 });
    const info = fs.lstatSync(root);
    if (!info.isDirectory() || info.isSymbolicLink() || info.uid !== process.getuid() || (info.mode & 0o777) !== 0o700) throw new Error("Native socket root must be a private owned directory.");
    fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
    if (fs.statSync(root).dev !== fs.statSync(directory).dev) throw new Error("Native socket alias must remain on the execution filesystem.");
    namespace = path.join(root, createHash("sha256").update(directory).update("\0").update(process.env.BRAID_NATIVE_EXECUTION_ID).digest("hex").slice(0, 24));
    if (Buffer.byteLength(path.join(namespace, "daemon/user.sock")) >= limit) throw new Error("Native socket root is still too long.");
    try { fs.symlinkSync(directory, namespace); }
    catch (error) { if (error.code !== "EEXIST" || !fs.lstatSync(namespace).isSymbolicLink() || fs.readlinkSync(namespace) !== directory) throw error; }
  }
  process.env.MCPORTER_DAEMON_DIR = namespace;
}
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const diagnostic = (error) => error instanceof Error ? error.message : String(error);
const readJson = (file) => JSON.parse(fs.readFileSync(file, "utf8"));

/** The detached launcher registers ownership before starting the payload. */
export function spawnManaged(command, args, options = {}, metadata = {}) {
  const env = options.env ?? process.env;
  if (!env.BRAID_NATIVE_EXECUTION_DIR || !env.BRAID_NATIVE_EXECUTION_ID) throw new Error("Native execution identity is unavailable.");
  const startId = randomUUID();
  const launchArgs = [fileURLToPath(new URL("./process-owner.mjs", import.meta.url)), "launch", "--start-id", startId, "--kind", metadata.kind ?? "tool"];
  if (metadata.manifest) launchArgs.push("--manifest", metadata.manifest);
  if (metadata.service) launchArgs.push("--service");
  launchArgs.push("--", command, ...args);
  const child = spawn(process.execPath, launchArgs, { ...options, detached: true, env: { ...env, BRAID_NATIVE_LAUNCH_START_ID: startId } });
  child.managedStartId = startId;
  child.managedResourceDirectory = env.BRAID_NATIVE_EXECUTION_DIR;
  child.managedStartReceipt = path.join(env.BRAID_NATIVE_EXECUTION_DIR, "starts", `${startId}.json`);
  child.managedClose = new Promise(resolve => {
    child.once("error", error => resolve({ error: diagnostic(error) }));
    child.once("close", (code, signal) => resolve({ code, signal }));
  });
  return child;
}

/** Wait without blocking the Pi event loop; callers attach stdout/error/close first. */
export async function waitManagedStartup(child, timeoutMs = 30_000) {
  if (!child?.managedStartReceipt) return { status: "started", unmanaged: true };
  child.managedStartup ??= (async () => {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      try {
        const start = readJson(child.managedStartReceipt);
        if (start.status !== "started") throw new Error(`Managed process did not start: ${JSON.stringify(start)}`);
        child.managedProcessRecord = start.process_record;
        return start;
      } catch (error) { if (error.code !== "ENOENT") throw error; }
      if ((child.exitCode !== null && child.exitCode !== undefined) ||
          (child.signalCode !== null && child.signalCode !== undefined)) {
        throw new Error(`Managed launcher exited before publishing a start receipt: ${child.managedStartReceipt}.`);
      }
      await pause(10);
    }
    const recordPath = path.join(child.managedResourceDirectory, "processes", `${child.managedStartId}.json`);
    let cleanup;
    await withAdmission(child.managedResourceDirectory, () => writeJson(path.join(child.managedResourceDirectory, "fences", `${child.managedStartId}.json`), { start_id: child.managedStartId }));
    if (fs.existsSync(recordPath)) {
      child.managedProcessRecord = recordPath;
      const record = readJson(recordPath);
      await withAdmission(child.managedResourceDirectory, () => writeJson(path.join(child.managedResourceDirectory, "fences", `${record.start_id}.json`), { execution_id: record.execution_id, start_id: record.start_id }));
      try { await stopRecords([record], record.execution_id, process.pid, false, child.managedResourceDirectory); cleanup = "owned subtree stopped"; }
      catch (error) { cleanup = `owned subtree unknown: ${diagnostic(error)}`; }
    } else { child.kill("SIGKILL"); cleanup = "launcher killed before registration; no subtree proof"; }
    throw new Error(`Managed launcher did not publish a start receipt: ${child.managedStartReceipt}; ${cleanup}.`);
  })();
  return child.managedStartup;
}

export function registerManagedProvider(name, snapshot, stopServices) {
  const provider = { snapshot, stopServices };
  providers.set(name, provider);
  return () => { if (providers.get(name) === provider) providers.delete(name); };
}

function execution(env = process.env) {
  const directory = env.BRAID_NATIVE_EXECUTION_DIR;
  const id = env.BRAID_NATIVE_EXECUTION_ID;
  if (!directory || !id) throw new Error("Native execution identity is unavailable.");
  return { directory, id };
}

function records(directory, id) {
  const entries = fs.readdirSync(path.join(directory, "processes"));
  return entries.filter((name) => name.endsWith(".json")).map((name) => {
    const record = readJson(path.join(directory, "processes", name));
    if (record.schema_version !== 1 || record.execution_id !== id || !Number.isInteger(record.pid) || !Number.isInteger(record.pgid) || !record.starttime || !record.boot_id || !record.start_id) {
      throw new Error(`Invalid native ownership record: ${name}.`);
    }
    return { ...record, ownership_path: path.join(directory, "processes", name) };
  });
}

function ownedProcesses(ownership, id, skipPid, allExecution = false) {
  const bootId = bootIdentity();
  if (ownership.some(record => record.boot_id !== bootId)) throw new Error("Native ownership belongs to another boot identity.");
  const all = processSnapshot();
  const ownGroups = new Set();
  for (const record of ownership) {
    const leader = all.find((item) => item.pid === record.pid);
    if (leader && leader.starttime !== record.starttime) {
      let previous;
      try { previous = readJson(path.join(path.dirname(path.dirname(record.ownership_path)), "terminals", `${record.start_id}.json`)); }
      catch (error) { if (error.code !== "ENOENT") throw error; }
      if (previous?.start_id === record.start_id && previous.execution_id === id && previous.terminal?.status === "stopped") continue;
      throw new Error(`Native PID identity conflict for ${record.start_id}.`);
    }
    if (leader?.starttime === record.starttime && leader.pgid === record.pgid) {
      // macOS birth time has seconds precision; inherited random markers disambiguate reuse.
      if (process.platform === "darwin" && (leader.owner !== id || leader.startId !== record.start_id)) throw new Error(`Unverified native owner marker for ${record.start_id}.`);
      ownGroups.add(record.pgid);
    }
    else if (leader && leader.owner === id) throw new Error(`Native PID identity conflict for ${record.start_id}.`);
    else {
      const payload = record.payload_identity && all.find(item => item.pid === record.payload_identity.pid);
      if (payload && payload.starttime === record.payload_identity.starttime && payload.pgid === record.pgid) {
        ownGroups.add(record.pgid);
        continue;
      }
      const members = all.filter((item) => item.pgid === record.pgid);
      if (members.length && !members.every((item) => item.owner === id)) throw new Error(`Unverified surviving process group ${record.pgid}.`);
      if (members.length) ownGroups.add(record.pgid);
    }
  }
  const startIds = new Set(ownership.map((record) => record.start_id));
  const skipped = new Set(Array.isArray(skipPid) ? skipPid : [skipPid]);
  return all.filter((item) => !skipped.has(item.pid) && (ownGroups.has(item.pgid) || (item.owner === id && (allExecution || startIds.has(item.startId)))));
}

function ownedDescendants(ownership, startId) {
  const selected = new Set([startId]);
  for (let previous = -1; previous !== selected.size;) {
    previous = selected.size;
    for (const record of ownership) if (selected.has(record.parent_start_id)) selected.add(record.start_id);
  }
  return ownership.filter((record) => selected.has(record.start_id));
}

function signalExact(identity, signal) {
  const current = processIdentity(identity.pid);
  if (!current || /^[ZX]/.test(current.state)) return;
  if (current.starttime !== identity.starttime) throw new Error(`PID ${identity.pid} changed birth identity before ${signal}.`);
  if (process.platform === "darwin") {
    let markers;
    try { markers = processMarkers(identity.pid); }
    catch (error) {
      const after = processIdentity(identity.pid);
      if (!after || /^[ZX]/.test(after.state)) return;
      throw error;
    }
    const after = processIdentity(identity.pid);
    // Exit can remove environment markers while ps reads them; verify disappearance.
    if (!after || /^[ZX]/.test(after.state)) return;
    if (after.starttime !== identity.starttime) throw new Error(`PID ${identity.pid} changed birth identity before ${signal}.`);
    if (markers.owner !== identity.owner || markers.startId !== identity.startId) throw new Error(`PID ${identity.pid} changed ownership markers before ${signal}.`);
  }
  try { process.kill(identity.pid, signal); }
  catch (error) { if (error.code !== "ESRCH") throw error; }
}

async function stopRecords(ownership, id, skipPid, allExecution = false, directory = execution().directory) {
  const roots = ownership.map((record) => record.start_id);
  const collect = () => {
    const current = records(directory, id);
    const selected = allExecution ? current : [...new Map(roots.flatMap((root) => ownedDescendants(current, root)).map((record) => [record.start_id, record])).values()];
    return ownedProcesses(selected, id, skipPid, allExecution);
  };
  let active = collect();
  for (const item of active) signalExact(item, "SIGTERM");
  const deadline = Date.now() + 3000;
  while (active.length && Date.now() < deadline) { await pause(25); active = collect(); }
  for (const item of active) signalExact(item, "SIGKILL");
  const killDeadline = Date.now() + 1000;
  while (active.length && Date.now() < killDeadline) { await pause(25); active = collect(); }
  if (active.length) throw new Error(`Owned execution remains active: ${active.map((item) => item.pid).join(", ")}.`);
  // Admission has been fenced; repeat the observation after signals settle.
  await pause(25);
  if (collect().length) throw new Error("Owned execution became active during stop verification.");
}

export function managedState(session = {}) {
  try {
    const { directory, id } = execution();
    if (fs.existsSync(path.join(directory, "stopping.json"))) return { status: "unknown", reason: "execution_stopping", execution_id: id };
    const ownership = records(directory, id);
    if (session.isStreaming || session.isCompacting || session.pendingMessageCount > 0) return { status: "busy", reason: "native_turn_or_messages", execution_id: id };
    if (ownedProcesses(ownership, id, [process.pid, ownership.find(record => record.start_id === process.env.BRAID_NATIVE_START_ID)?.pid], true).length) return { status: "busy", reason: "owned_process_active", execution_id: id };
    for (const [name, provider] of providers) {
      const state = provider.snapshot();
      if (state.status !== "quiescent") return { status: state.status, reason: `${name}: ${state.reason ?? state.status}`.slice(0, 512), execution_id: id };
    }
    return { status: "quiescent", execution_id: id };
  } catch (error) { return { status: "unknown", reason: diagnostic(error).slice(0, 512), execution_id: process.env.BRAID_NATIVE_EXECUTION_ID ?? "unavailable" }; }
}

/** Yield only declared services after finite/native obligations are empty. */
export async function yieldStoppableServices(session = {}) {
  try {
    const { directory, id } = execution();
    if (session.isStreaming || session.isCompacting || session.pendingMessageCount > 0) return managedState(session);
    if (fs.existsSync(path.join(directory, "stopping.json"))) return managedState(session);
    const serviceProviders = [];
    for (const [name, provider] of providers) {
      const state = provider.snapshot();
      if (state.status === "quiescent") continue;
      if (!state.services_only || typeof provider.stopServices !== "function") {
        return { status: state.status, reason: `${name}: ${state.reason ?? state.status}`, execution_id: id };
      }
      serviceProviders.push(provider);
    }
    const ownership = records(directory, id);
    const serviceRoots = ownership.filter((record) => record.service === true);
    const serviceRecords = [...new Map(serviceRoots.flatMap((root) => ownedDescendants(ownership, root.start_id)).map((record) => [record.start_id, record])).values()];
    const covered = new Set(ownedProcesses(serviceRecords, id, process.pid).map((item) => item.pid));
    const active = ownedProcesses(ownership, id, [process.pid, ownership.find(record => record.start_id === process.env.BRAID_NATIVE_START_ID)?.pid], true);
    if (active.some((item) => !covered.has(item.pid))) return { status: "busy", reason: "finite_or_unclassified_owned_process", execution_id: id };
    await withAdmission(directory, () => { for (const root of serviceRoots) writeJson(path.join(directory, "fences", `${root.start_id}.json`), { start_id: root.start_id, execution_id: id }); });
    for (const provider of serviceProviders) await provider.stopServices();
    await stopRecords(serviceRoots, id, process.pid, false, directory);
    const verifiedAt = Date.now();
    for (const record of serviceRecords) {
      const terminalPath = path.join(directory, "terminals", `${record.start_id}.json`);
      let previous;
      try { previous = readJson(terminalPath); } catch (error) { if (error.code !== "ENOENT") throw error; }
      writeJson(terminalPath, {
        start_id: record.start_id, execution_id: id,
        close: previous?.close ?? { observation: "owned_service_handoff", observed_at: verifiedAt },
        terminal: { status: "stopped", verified_at: verifiedAt },
      });
    }
    const state = managedState(session);
    writeJson(path.join(directory, "service-handoff.json"), { ...state, service_start_ids: serviceRoots.map((record) => record.start_id), verified_at: verifiedAt });
    return state;
  } catch (error) { return { status: "unknown", reason: diagnostic(error).slice(0, 1024), execution_id: process.env.BRAID_NATIVE_EXECUTION_ID ?? "unavailable" }; }
}

export async function cleanupOwnedExecution(options = {}) {
  let receiptPath;
  try {
    const { directory, id } = options.directory ? { directory: options.directory, id: options.id } : execution();
    if (!id) throw new Error("Native cleanup requires an execution identity.");
    const self = options.skipSelf ? records(directory, id).find((record) => record.start_id === process.env.BRAID_NATIVE_START_ID) : undefined;
    if (options.skipSelf && !self) throw new Error("Native root ownership identity is unavailable.");
    if (self?.parent_start_id) throw new Error("Nested native RPC cleanup requires its own subtree fence; execution cleanup belongs to the root owner.");
    await withAdmission(directory, () => writeJson(path.join(directory, "stopping.json"), { execution_id: id, requested_at: Date.now() }));
    receiptPath = path.join(directory, options.skipSelf ? "children-stop.json" : "execution-stop.json");
    const ownership = records(directory, id);
    await stopRecords(ownership, id, options.skipSelf ? [process.pid, self?.pid] : process.pid, true, directory);
    const result = { status: "stopped", execution_id: id, scope: options.skipSelf ? "root-descendants" : "execution", verified_at: Date.now(), coverage: "registered-process-groups-and-inherited-execution-markers", receipt_path: receiptPath };
    writeJson(receiptPath, result);
    return result;
  } catch (error) {
    const result = { status: "unknown", reason: diagnostic(error).slice(0, 1024), ...(receiptPath ? { receipt_path: receiptPath } : {}) };
    if (receiptPath) writeJson(receiptPath, result);
    return result;
  }
}

/** Raw close observations are separate from the proof that the owned group stopped. */
export async function finishManagedProcess(child, close) {
  if (!child.managedProcessRecord && child.managedStartReceipt) {
    try { await waitManagedStartup(child); }
    catch (error) { return { status: "unknown", reason: diagnostic(error) }; }
  }
  if (!child.managedProcessRecord) return undefined;
  const record = readJson(child.managedProcessRecord);
  const { directory, id } = execution();
  let terminal;
  try { await withAdmission(directory, () => writeJson(path.join(directory, "fences", `${record.start_id}.json`), { start_id: record.start_id, execution_id: id })); await stopRecords(ownedDescendants(records(directory, id), record.start_id), id, process.pid); terminal = { status: "stopped", verified_at: Date.now() }; }
  catch (error) { terminal = { status: "unknown", reason: diagnostic(error) }; }
  const terminalPath = path.join(directory, "terminals", `${record.start_id}.json`);
  let previous;
  try { previous = readJson(terminalPath); } catch (error) { if (error.code !== "ENOENT") throw error; }
  writeJson(terminalPath, { start_id: record.start_id, execution_id: id, close: close.observation ? previous?.close ?? close : close, terminal });
  return terminal;
}

export function createManagedProcessTreeController(child) {
  if (!child.managedProcessRecord) return undefined;
  let terminal;
  const terminate = () => terminal ??= (async () => {
    const result = await finishManagedProcess(child, { observation: "cleanup_requested", observed_at: Date.now() });
    return result?.status === "stopped"
      ? { state: "observed", mechanism: "posix-process-group", processGroupId: child.pid, verifiedAt: Date.now() }
      : { state: "unknown", reason: "verification-failed", diagnostic: result?.reason ?? "Managed process record unavailable." };
  })();
  return { terminate, finishAfterWriterClose: terminate };
}

export function publishChildStartup(manifestDirectory, status, reason) {
  if (!process.env.BRAID_NATIVE_EXECUTION_DIR || !manifestDirectory) return;
  writeJson(path.join(manifestDirectory, "native-child-startup.json"), { status, reason, execution_id: process.env.BRAID_NATIVE_EXECUTION_ID, observed_at: Date.now() });
}

export async function waitChildStartup(manifestDirectory, timeoutMs) {
  const file = path.join(manifestDirectory, "native-child-startup.json");
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const receipt = readJson(file);
      if (receipt.execution_id !== process.env.BRAID_NATIVE_EXECUTION_ID) throw new Error("Child startup execution identity mismatch.");
      return receipt;
    }
    catch (error) { if (error.code !== "ENOENT") throw error; }
    await pause(10);
  }
  throw new Error(`Async child did not confirm physical startup within ${timeoutMs}ms: ${file}.`);
}

// Process management is installed by this module; it does not alter model prompts.
export default function managedProcessExtension() {}

globalThis[Symbol.for("braid.native-runtime.v1")] = { spawnManaged, waitManagedStartup, finishManagedProcess, createManagedProcessTreeController, registerManagedProvider, publishChildStartup, waitChildStartup };

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const argument = (name) => args[args.indexOf(name) + 1];
  if (args[0] !== "cleanup" || !args.includes("--execution-dir") || !args.includes("--execution-id")) throw new Error("Usage: native-managed.mjs cleanup --execution-dir DIR --execution-id ID");
  console.log(JSON.stringify(await cleanupOwnedExecution({ directory: argument("--execution-dir"), id: argument("--execution-id") })));
}
