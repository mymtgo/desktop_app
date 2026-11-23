// Copies the release-built Endstep helper to the sidecar path that
// app/tauri.bundle.conf.json declares, named for the host target triple as
// Tauri requires (https://v2.tauri.app/develop/sidecar/). Builds nothing:
// run `cargo build --release --locked -p endstep-host` first.
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, realpathSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

export function sidecarName(triple, windows) {
  return `mymtgo-endstep-host-${triple}${windows ? '.exe' : ''}`;
}

// Real paths on both sides: run from a symlinked or junction folder, node
// gives the module its real path but leaves argv[1] as typed.
export function isMainModule(moduleUrl, scriptPath) {
  if (!scriptPath || !existsSync(scriptPath)) {
    return false;
  }
  return realpathSync(scriptPath) === realpathSync(fileURLToPath(moduleUrl));
}

function hostTriple() {
  const output = execFileSync('rustc', ['-vV'], { encoding: 'utf8' });
  const line = output.split(/\r?\n/).find((text) => text.startsWith('host: '));
  if (!line) {
    throw new Error('`rustc -vV` printed no host line');
  }
  return line.slice('host: '.length).trim();
}

function stage() {
  const root = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
  const windows = process.platform === 'win32';
  const source = join(root, 'target', 'release', windows ? 'mymtgo-endstep-host.exe' : 'mymtgo-endstep-host');
  if (!existsSync(source)) {
    console.error(`The Endstep helper is not built: ${source} is missing. Run \`cargo build --release --locked -p endstep-host\` first.`);
    process.exit(1);
  }
  const folder = join(root, 'app', 'binaries');
  mkdirSync(folder, { recursive: true });
  const destination = join(folder, sidecarName(hostTriple(), windows));
  copyFileSync(source, destination);
  console.log(`Staged ${source} as ${destination}`);
}

if (isMainModule(import.meta.url, process.argv[1])) {
  stage();
}
