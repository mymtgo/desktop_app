import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { pathToFileURL } from 'node:url';

import { isMainModule, sidecarName } from './stage-endstep-host.mjs';

test('sidecar_names_follow_the_target_triple', () => {
  assert.equal(
    sidecarName('x86_64-pc-windows-msvc', true),
    'mymtgo-endstep-host-x86_64-pc-windows-msvc.exe',
  );
  assert.equal(
    sidecarName('x86_64-unknown-linux-gnu', false),
    'mymtgo-endstep-host-x86_64-unknown-linux-gnu',
  );
});

test('a_script_run_through_a_linked_folder_is_still_the_main_module', (t) => {
  const folder = mkdtempSync(join(tmpdir(), 'stage-endstep-host-'));
  t.after(() => rmSync(folder, { recursive: true, force: true }));
  const realFolder = join(folder, 'real');
  const linkedFolder = join(folder, 'linked');
  writeFileSync(join(folder, 'other.mjs'), '');
  mkdirSync(realFolder);
  const script = join(realFolder, 'stage.mjs');
  writeFileSync(script, '');
  symlinkSync(realFolder, linkedFolder, 'junction');
  const moduleUrl = pathToFileURL(script).href;

  assert.equal(isMainModule(moduleUrl, script), true);
  assert.equal(isMainModule(moduleUrl, join(linkedFolder, 'stage.mjs')), true);
  assert.equal(isMainModule(moduleUrl, join(folder, 'other.mjs')), false);
  assert.equal(isMainModule(moduleUrl, join(folder, 'missing.mjs')), false);
  assert.equal(isMainModule(moduleUrl, undefined), false);
});
