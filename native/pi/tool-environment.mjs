import path from 'node:path';

// Execution-local XDG state must not silently change ordinary gh's credential
// lookup (including git's `gh auth git-credential` helper). Preserve the caller's
// normal path, never copy or expose credential values. Supported hosts are Unix.
export function preserveToolConfig(environment, home) {
  if (!environment.GH_CONFIG_DIR) {
    const config = environment.XDG_CONFIG_HOME || path.join(home, '.config');
    environment.GH_CONFIG_DIR = path.join(config, 'gh');
  }
}
