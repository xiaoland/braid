import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import {
  createAgentSessionFromServices,
  createAgentSessionRuntime,
  createAgentSessionServices,
  createBashToolDefinition,
  getShellConfig,
  ModelRuntime,
  parseArgs,
  resolveCliModel,
  runPrintMode,
  runRpcMode,
  SessionManager,
  SettingsManager,
  VERSION,
  detectSupportedImageMimeTypeFromFile,
} from '@earendil-works/pi-coding-agent';
import { runBraidRpcMode } from './rpc-adapter.mjs';

const managedRuntime = await import(process.env.BRAID_NATIVE_RUNTIME_MODULE);

function absolutePath(value, cwd) {
  const expanded = value.startsWith('~/') ? path.join(os.homedir(), value.slice(2)) : value;
  return path.resolve(cwd, expanded);
}

function assertSupportedCliOptions(args) {
  const unsupported = [
    [args.models !== undefined, '--models'],
    [args.listModels !== undefined, '--list-models'],
    [args.export !== undefined, '--export'],
    [args.offline, '--offline'],
    [args.name !== undefined, '--name'],
    [args.promptTemplates?.length, '--prompt-template'],
    [args.themes?.length, '--theme'],
    [args.useTheme !== undefined, '--use-theme'],
    [args.verbose, '--verbose'],
    [args.tuiMode !== undefined, '--tui-mode'],
  ].filter(([used]) => used);
  if (unsupported.length) throw new Error(`Unsupported Pi CLI option(s): ${unsupported.map(([, name]) => name).join(', ')}`);
}

async function readPipedStdin() {
  if (process.stdin.isTTY) return undefined;
  process.stdin.setEncoding('utf8');
  let content = '';
  for await (const chunk of process.stdin) content += chunk;
  return content.trim() || undefined;
}

async function processFileArguments(fileArgs, cwd) {
  let text = '';
  const images = [];
  for (const fileArg of fileArgs) {
    const filename = absolutePath(fileArg, cwd);
    let stat;
    try { stat = await fs.promises.stat(filename); }
    catch (error) { throw new Error(`File not found: ${filename}: ${error instanceof Error ? error.message : String(error)}`); }
    if (stat.size === 0) continue;
    const mimeType = await detectSupportedImageMimeTypeFromFile(filename);
    if (mimeType) {
      const data = await fs.promises.readFile(filename);
      images.push({ type: 'image', mimeType, data: data.toString('base64') });
      text += `<file name="${filename}"></file>\n`;
      continue;
    }
    try {
      const content = (await fs.promises.readFile(filename, 'utf8')).replace(/^\uFEFF/, '');
      text += `<file name="${filename}">\n${content}\n</file>\n`;
    } catch (error) {
      throw new Error(`Could not read file ${filename}: ${error instanceof Error ? error.message : String(error)}`);
    }
  }
  return { text, images };
}

async function prepareInitialMessage(args, cwd, stdinContent) {
  const fileContent = args.fileArgs.length ? await processFileArguments(args.fileArgs, cwd) : { text: '', images: [] };
  const parts = [];
  if (stdinContent !== undefined) parts.push(stdinContent);
  if (fileContent.text) parts.push(fileContent.text);
  if (args.messages.length) parts.push(args.messages.shift());
  return {
    initialMessage: parts.length ? parts.join('') : undefined,
    initialImages: fileContent.images.length ? fileContent.images : undefined,
  };
}

async function sessionManagerFor(parsed, cwd, settingsManager) {
  const sessionDir = parsed.sessionDir ? absolutePath(parsed.sessionDir, cwd) : settingsManager.getSessionDir();
  if (parsed.noSession || parsed.help) return SessionManager.inMemory(cwd, parsed.sessionId ? { id: parsed.sessionId } : undefined);
  if (parsed.fork) {
    const requested = absolutePath(parsed.fork, cwd);
    const source = fs.existsSync(requested)
      ? requested
      : (await SessionManager.listAll(sessionDir)).find(item => item.id.startsWith(parsed.fork))?.path;
    if (!source) throw new Error(`No session found matching '${parsed.fork}'.`);
    return SessionManager.forkFrom(source, cwd, sessionDir, parsed.sessionId ? { id: parsed.sessionId } : undefined);
  }
  if (parsed.session) {
    const selected = absolutePath(parsed.session, cwd);
    if (fs.existsSync(selected)) return SessionManager.open(selected, sessionDir, cwd);
    const matches = (await SessionManager.listAll(sessionDir)).filter(item => item.id.startsWith(parsed.session));
    if (matches.length !== 1) throw new Error(matches.length ? `Session '${parsed.session}' is ambiguous.` : `No session found matching '${parsed.session}'.`);
    return SessionManager.open(matches[0].path, sessionDir);
  }
  if (parsed.resume || parsed.continue) return SessionManager.continueRecent(cwd, sessionDir);
  if (parsed.sessionId) {
    const match = SessionManager.findById(cwd, parsed.sessionId, sessionDir);
    return match ? SessionManager.open(match, sessionDir) : SessionManager.create(cwd, sessionDir, { id: parsed.sessionId });
  }
  return SessionManager.create(cwd, sessionDir);
}

function isWithin(parent, candidate) {
  const relative = path.relative(path.resolve(parent), path.resolve(candidate));
  return relative === '' || (relative !== '..' && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative));
}

function managedBashOperations(shellPath) {
  return {
    exec: async (command, cwd, { onData, signal, timeout, env }) => {
      if (signal?.aborted) throw new Error('aborted');
      const config = getShellConfig(shellPath);
      await fs.promises.access(cwd);
      const stdinCommand = config.commandTransport === 'stdin';
      const child = managedRuntime.spawnManaged(config.shell, stdinCommand ? config.args : [...config.args, command], {
        cwd,
        env: env ?? process.env,
        detached: true,
        stdio: [stdinCommand ? 'pipe' : 'ignore', 'pipe', 'pipe'],
        windowsHide: true,
      }, { kind: 'pi-bash' });
      let timeoutId;
      child.stdout?.on('data', onData);
      child.stderr?.on('data', onData);
      child.stdin?.on('error', () => {});
      if (stdinCommand) child.stdin?.end(command);
      let resolveStop;
      const stopRequested = new Promise(resolve => { resolveStop = resolve; });
      const terminate = reason => resolveStop(reason);
      const onAbort = () => terminate('aborted');
      signal?.addEventListener('abort', onAbort, { once: true });
      if (timeout !== undefined) timeoutId = setTimeout(() => terminate(`timeout:${timeout}`), timeout * 1000);
      try {
        await managedRuntime.waitManagedStartup(child);
        const result = await Promise.race([child.managedClose.then(close => ({ close })), stopRequested.then(reason => ({ reason }))]);
        if ('reason' in result) {
          const terminal = await managedRuntime.finishManagedProcess(child, { observation: 'cancelled_or_timed_out', observed_at: Date.now() });
          if (terminal?.status !== 'stopped') throw new Error(`Managed bash stop is unknown: ${terminal?.reason ?? 'missing terminal receipt'}`);
          await child.managedClose;
          throw new Error(result.reason);
        }
        const closed = result.close;
        const terminal = await managedRuntime.finishManagedProcess(child, { ...closed, observed_at: Date.now() });
        if (terminal?.status === 'unknown') throw new Error(`Managed bash cleanup is unknown: ${terminal.reason}`);
        return { exitCode: closed.code ?? 1 };
      } finally {
        if (timeoutId) clearTimeout(timeoutId);
        signal?.removeEventListener('abort', onAbort);
      }
    },
  };
}

function deferredInputs(session) {
  const latest = new Map();
  for (const entry of session.sessionManager.getBranch()) {
    if (entry.type !== 'custom' || entry.customType !== 'braid.pending-native-inputs') continue;
    const data = entry.data;
    if (data?.id && data.state === 'pending') latest.set(data.id, data);
    else if (data?.id && data.state === 'restored') latest.delete(data.id);
    else if (data?.id && data.state === 'superseded') latest.delete(data.id);
  }
  return [...latest.values()];
}

function installSessionPolicy(session, terminalHookErrors) {
  const native = { pendingInputs: deferredInputs(session), queueFailure: undefined, hookErrors: terminalHookErrors, unsubscribe: undefined, unsubscribeExtensionError: undefined, abortError: undefined };
  const sendCustomMessage = session.sendCustomMessage.bind(session);
  const executeBash = session.executeBash.bind(session);
  const bashOperations = managedBashOperations(session.settingsManager.getShellPath());
  session.sendCustomMessage = (message, options = {}) => sendCustomMessage(message, { ...options, triggerTurn: false });
  session.executeBash = (command, onChunk, options) => executeBash(
    command,
    onChunk,
    options?.operations === undefined ? { ...options, operations: bashOperations } : options,
  );
  const deferQueuedInputs = reason => {
    const requested = { steering: [...session.getSteeringMessages()], followUp: [...session.getFollowUpMessages()] };
    if (!requested.steering.length && !requested.followUp.length) return;
    const id = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    const record = { id, state: 'pending', ...requested, error: reason };
    let persisted = false;
    try {
      session.sessionManager.appendCustomEntry('braid.pending-native-inputs', record);
      persisted = true;
    } catch (error) {
      native.queueFailure = `cannot persist queued input before deferring: ${error instanceof Error ? error.message : String(error)}`;
    }
    let cleared;
    try { cleared = session.clearQueue(); }
    catch (error) {
      native.queueFailure = `cannot clear queued input after terminal failure: ${error instanceof Error ? error.message : String(error)}`;
      native.pendingInputs = deferredInputs(session);
      return;
    }
    if (JSON.stringify(cleared) !== JSON.stringify(requested)) {
      if (persisted) {
        try { session.sessionManager.appendCustomEntry('braid.pending-native-inputs', { id, state: 'superseded' }); }
        catch (error) { native.queueFailure = `cannot supersede stale queued input record: ${error instanceof Error ? error.message : String(error)}`; }
      }
      if (cleared.steering.length || cleared.followUp.length) {
        const corrected = { ...cleared, error: reason, id: `${id}-corrected`, state: 'pending' };
        native.pendingInputs.push(corrected);
        try { session.sessionManager.appendCustomEntry('braid.pending-native-inputs', corrected); }
        catch (error) { native.queueFailure = `cannot persist cleared Pi input: ${error instanceof Error ? error.message : String(error)}`; }
      }
    } else {
      native.pendingInputs.push(record);
      if (!persisted) {
        try { session.sessionManager.appendCustomEntry('braid.pending-native-inputs', record); }
        catch (error) { native.queueFailure = `cannot persist cleared Pi input: ${error instanceof Error ? error.message : String(error)}`; }
      }
    }
  };
  native.unsubscribeExtensionError = session.extensionRunner.onError(error => {
    if (!['agent_end', 'agent_before_settle', 'agent_settled'].includes(error.event)) return;
    native.hookErrors.push(error);
    const reason = `Pi extension hook ${error.event} failed (${error.extensionPath}): ${error.error}`;
    try { deferQueuedInputs(reason); }
    catch (failure) { native.queueFailure = `cannot preserve queued input after extension hook failure: ${failure instanceof Error ? failure.message : String(failure)}`; }
    if (native.hookErrors.length === 1) {
      void session.abort().catch(failure => {
        native.abortError = failure instanceof Error ? failure.message : String(failure);
      });
    }
  });
  native.unsubscribe = session.subscribe(event => {
    if (event.type !== 'agent_end' || event.willRetry) return;
    const finalMessage = [...event.messages].reverse().find(message => message.role === 'assistant');
    if (finalMessage?.stopReason !== 'error') return;
    deferQueuedInputs(finalMessage.errorMessage ?? 'Agent turn failed.');
  });
  return native;
}

function installRuntimePolicy(runtime) {
  const terminalHookErrors = [];
  let state = installSessionPolicy(runtime.session, terminalHookErrors);
  const setRebindSession = runtime.setRebindSession.bind(runtime);
  runtime.setRebindSession = callback => setRebindSession(async (...args) => {
    await callback?.(...args);
    state.unsubscribe?.();
    state.unsubscribeExtensionError?.();
    state = installSessionPolicy(runtime.session, terminalHookErrors);
  });
  return {
    get sessionState() { return state; },
    async restorePendingInputs() {
      if (terminalHookErrors.length) throw new Error(`Native runtime stopped after Pi extension hook failure: ${terminalHookErrors.map(error => `${error.extensionPath} ${error.event}: ${error.error}`).join('; ')}`);
      if (state.queueFailure) throw new Error(`Deferred Pi input preservation failed: ${state.queueFailure}`);
      if (!state.pendingInputs.length) return;
      const restoring = state.pendingInputs;
      try {
        for (const record of restoring) {
          for (const message of record.steering) await runtime.session.steer(message, undefined, { source: 'extension' });
          for (const message of record.followUp) await runtime.session.followUp(message, undefined, { source: 'extension' });
        }
      } catch (error) {
        runtime.session.clearQueue();
        throw error;
      }
      for (const record of restoring) {
        runtime.session.sessionManager.appendCustomEntry('braid.pending-native-inputs', { id: record.id, state: 'restored' });
      }
      state.pendingInputs = [];
      state.queueFailure = undefined;
    },
  };
}

async function buildRuntime(parsed) {
  const cwd = process.cwd();
  const agentDir = process.env.PI_CODING_AGENT_DIR;
  const startupSettings = SettingsManager.create(cwd, agentDir);
  const sessionManager = await sessionManagerFor(parsed, cwd, startupSettings);
  const modelRuntime = await ModelRuntime.create({
    authPath: path.join(agentDir, 'auth.json'),
    modelsPath: path.join(agentDir, 'models.json'),
    refreshOnCreate: false,
  });
  const resolved = parsed.model ? resolveCliModel({ cliProvider: parsed.provider, cliModel: parsed.model, cliThinking: parsed.thinking, modelRuntime }) : undefined;
  if (resolved?.error) throw new Error(resolved.error);
  const defaultProvider = startupSettings.getDefaultProvider();
  const defaultModel = startupSettings.getDefaultModel();
  const model = resolved?.model ?? (defaultProvider && defaultModel ? modelRuntime.getModel(defaultProvider, defaultModel) : undefined) ?? modelRuntime.getModels()[0];
  if (!model) throw new Error('No model is available in this Braid native home.');
  if (parsed.apiKey) await modelRuntime.setRuntimeApiKey(model.provider, parsed.apiKey);
  const thinkingLevel = parsed.thinking ?? resolved?.thinkingLevel ?? startupSettings.getDefaultThinkingLevel() ?? 'medium';
  const extensionPaths = (parsed.extensions ?? []).map(item => absolutePath(item, cwd));
  const skillPaths = (parsed.skills ?? []).map(item => absolutePath(item, cwd));
  const createRuntime = async ({ cwd: sessionCwd, agentDir: runtimeAgentDir, sessionManager: targetSessionManager, sessionStartEvent }) => {
    const settingsManager = SettingsManager.create(sessionCwd, runtimeAgentDir);
    const services = await createAgentSessionServices({
      cwd: sessionCwd,
      agentDir: runtimeAgentDir,
      settingsManager,
      modelRuntime,
      extensionFlagValues: parsed.unknownFlags,
      resourceLoaderOptions: {
        additionalExtensionPaths: extensionPaths,
        additionalSkillPaths: skillPaths,
        noExtensions: parsed.noExtensions,
        noSkills: parsed.noSkills,
        disabledBuiltinExtensions: parsed.noMcp ? ['mcp'] : undefined,
        noPromptTemplates: parsed.noPromptTemplates,
        noThemes: parsed.noThemes,
        noContextFiles: parsed.noContextFiles,
        agentsFilesOverride: base => ({
          agentsFiles: base.agentsFiles.filter(item => isWithin(sessionCwd, item.path) || isWithin(runtimeAgentDir, item.path)),
        }),
        systemPrompt: parsed.systemPrompt,
        appendSystemPrompt: parsed.appendSystemPrompt,
      },
    });
    const selectedModel = parsed.model
      ? resolveCliModel({ cliProvider: parsed.provider, cliModel: parsed.model, cliThinking: parsed.thinking, modelRuntime: services.modelRuntime }).model
      : model;
    const created = await createAgentSessionFromServices({
      services,
      sessionManager: targetSessionManager,
      sessionStartEvent,
      model: selectedModel,
      thinkingLevel,
      tools: parsed.tools,
      excludeTools: parsed.excludeTools,
      noTools: parsed.noTools ? 'all' : parsed.noBuiltinTools ? 'builtin' : undefined,
      customTools: [createBashToolDefinition(sessionCwd, {
        operations: managedBashOperations(settingsManager.getShellPath()),
        shellPath: settingsManager.getShellPath(),
        commandPrefix: settingsManager.getShellCommandPrefix(),
      })],
    });
    return { ...created, services };
  };
  return createAgentSessionRuntime(createRuntime, { cwd: sessionManager.getCwd(), agentDir, sessionManager });
}

export async function main(argv = process.argv.slice(2)) {
  const args = parseArgs(argv);
  if (args.diagnostics.some(item => item.type === 'error')) throw new Error(args.diagnostics.filter(item => item.type === 'error').map(item => item.message).join('\n'));
  if (args.version) { process.stdout.write(`${VERSION}\n`); return; }
  if (args.help) return;
  assertSupportedCliOptions(args);
  if (args.mode === 'rpc' && args.fileArgs.length) throw new Error('@file arguments are not supported in RPC mode.');
  const runtime = await buildRuntime(args);
  const policy = installRuntimePolicy(runtime);
  if (args.mode === 'rpc') return runBraidRpcMode(runtime, policy, managedRuntime, VERSION, runRpcMode);
  const stdinContent = await readPipedStdin();
  const { initialMessage, initialImages } = await prepareInitialMessage(args, process.cwd(), stdinContent);
  const exitCode = await runPrintMode(runtime, { mode: args.mode === 'json' ? 'json' : 'text', initialMessage, initialImages, messages: args.messages });
  if (exitCode !== 0) process.exitCode = exitCode;
  else if (policy.sessionState.hookErrors.length) process.exitCode = 1;
}
