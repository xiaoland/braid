import { PassThrough } from 'node:stream';
import readline from 'node:readline';

const BRAID_CAPABILITIES = ['retry_intent', 'settle_extensions', 'owned_inference', 'managed_execution', 'owned_stop'];
const success = (id, command, data) => ({ id, type: 'response', command, success: true, data });

function runtimeState(runtime, policy, managedRuntime, piVersion) {
  const session = runtime.session;
  const deferred = policy.sessionState.pendingInputs;
  const managed = policy.sessionState.queueFailure
    ? { status: 'unknown', reason: `deferred_input_preservation_failed: ${policy.sessionState.queueFailure}`, execution_id: process.env.BRAID_NATIVE_EXECUTION_ID }
    : policy.sessionState.hookErrors.length
    ? { status: 'unknown', reason: 'extension_hook_failure_stop', execution_id: process.env.BRAID_NATIVE_EXECUTION_ID }
    : deferred.length
    ? { status: 'busy', reason: 'deferred_native_inputs', execution_id: process.env.BRAID_NATIVE_EXECUTION_ID }
    : managedRuntime.managedState(session);
  const extensions = runtime.services.resourceLoader.getExtensions();
  return {
    managed_state: managed,
    braid_native_runtime: {
      schema_version: 1,
      pi_version: piVersion,
      capabilities: BRAID_CAPABILITIES,
      execution_id: process.env.BRAID_NATIVE_EXECUTION_ID,
      extensions: extensions.extensions.map(extension => extension.path),
      extension_errors: extensions.errors,
      hook_failure_stop_requested: policy.sessionState.hookErrors.length > 0,
      hook_errors: policy.sessionState.hookErrors.map(({ extensionPath, event, error }) => ({ extension_path: extensionPath, event, error })),
      ...(policy.sessionState.abortError ? { hook_failure_stop_error: policy.sessionState.abortError } : {}),
      deferred_inputs: deferred.map(({ id, steering, followUp, error }) => ({
        id,
        steering_count: steering.length,
        follow_up_count: followUp.length,
        error,
      })),
      ...(policy.sessionState.queueFailure ? { deferred_input_error: policy.sessionState.queueFailure } : {}),
    },
  };
}

function decorateStateResponse(line, runtime, policy, managedRuntime, piVersion) {
  if (!line.endsWith('\n')) return line;
  const payload = line.slice(0, -1);
  let value;
  try { value = JSON.parse(payload); } catch { return line; }
  if (value?.type === 'response' && value.command === 'get_state' && value.success && value.data) {
    Object.assign(value.data, runtimeState(runtime, policy, managedRuntime, piVersion));
    return `${JSON.stringify(value)}\n`;
  }
  return line;
}

export async function runBraidRpcMode(runtime, policy, managedRuntime, piVersion, runRpcMode) {
  const originalInput = process.stdin;
  const rpcInput = new PassThrough();
  Object.defineProperty(process, 'stdin', { configurable: true, value: rpcInput });

  const originalWrite = process.stdout.write.bind(process.stdout);
  process.stdout.write = function writeWithBraidState(chunk, encodingOrCallback, callback) {
    const encoding = typeof encodingOrCallback === 'string' ? encodingOrCallback : undefined;
    const done = typeof encodingOrCallback === 'function' ? encodingOrCallback : callback;
    const text = Buffer.isBuffer(chunk) ? chunk.toString(encoding) : String(chunk);
    return originalWrite(decorateStateResponse(text, runtime, policy, managedRuntime, piVersion), done);
  };

  const response = (value) => originalWrite(`${JSON.stringify(value)}\n`);
  const input = readline.createInterface({ input: originalInput, crlfDelay: Infinity });
  let chain = Promise.resolve();
  input.on('line', line => {
    chain = chain.then(async () => {
      let command;
      try {
        try { command = JSON.parse(line); } catch { rpcInput.write(`${line}\n`); return; }
        if (command?.type === 'stop_owned_execution') {
          response(success(command.id, command.type, await managedRuntime.cleanupOwnedExecution({ skipSelf: true })));
          return;
        }
        if (command?.type === 'yield_stoppable_services') {
          const managedState = policy.sessionState.queueFailure
            ? { status: 'unknown', reason: `deferred_input_preservation_failed: ${policy.sessionState.queueFailure}`, execution_id: process.env.BRAID_NATIVE_EXECUTION_ID }
            : policy.sessionState.hookErrors.length
            ? { status: 'unknown', reason: 'extension_hook_failure_stop', execution_id: process.env.BRAID_NATIVE_EXECUTION_ID }
            : policy.sessionState.pendingInputs.length
            ? { status: 'busy', reason: 'deferred_native_inputs', execution_id: process.env.BRAID_NATIVE_EXECUTION_ID }
            : await managedRuntime.yieldStoppableServices(runtime.session);
          response(success(command.id, command.type, { managed_state: managedState }));
          return;
        }
        if (['prompt', 'steer', 'follow_up'].includes(command?.type)) await policy.restorePendingInputs();
        rpcInput.write(`${line}\n`);
      } catch (error) {
        response({ id: command?.id, type: 'response', command: command?.type ?? 'braid_adapter', success: false, error: error instanceof Error ? error.message : String(error) });
      }
    });
  });
  input.on('close', () => { void chain.finally(() => rpcInput.end()); });
  input.on('error', error => { process.stderr.write(`Braid Pi RPC input failed: ${error.message}\n`); rpcInput.destroy(error); });

  return runRpcMode(runtime);
}
