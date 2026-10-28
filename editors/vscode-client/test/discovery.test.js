// The tiers of server discovery (KLF-EDITOR.md section 19.6), in order.

const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const discovery = require('../src/discovery');

const manifest = {
  language: 'karc',
  displayName: 'KArc',
  driver: 'karc',
  server: { arguments: ['lsp'], setting: 'karc.server.path', environment: 'KLF_KARC_SYSROOT' },
};

function driverIn(directory, name) {
  fs.mkdirSync(directory, { recursive: true });
  const file = path.join(directory, name);
  fs.writeFileSync(file, '#!/bin/sh\n', { mode: 0o755 });
  return file;
}

const scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'klf-vscode-client-'));
const sysroot = path.join(scratch, 'sysroot');
const inSysroot = driverIn(path.join(sysroot, 'bin'), 'karc');
const onPath = driverIn(path.join(scratch, 'path'), 'karc');
const ksl = driverIn(path.join(scratch, 'ksl-sysroot', 'bin'), 'ksl');
const platform = 'linux';
const env = { PATH: path.join(scratch, 'path'), KLF_KARC_SYSROOT: sysroot };
const settings = { 'karc.server.path': '/opt/karc', 'karc.server.arguments': ['--log'] };
const configured = (name) => settings[name];
const unset = () => undefined;

test('the setting comes first, with its arguments after `lsp`', () => {
  const found = discovery.findServer(manifest, configured, env, platform);
  assert.deepStrictEqual(found.command, '/opt/karc');
  assert.deepStrictEqual(found.args, ['lsp', '--log']);
  assert.match(found.tier, /karc\.server\.path/);
});

test('the sysroot variable comes before the PATH', () => {
  const found = discovery.findServer(manifest, unset, env, platform);
  assert.strictEqual(found.command, inSysroot);
  assert.match(found.tier, /KLF_KARC_SYSROOT/);
});

test('the PATH is the last tier', () => {
  const found = discovery.findServer(manifest, unset, { PATH: env.PATH }, platform);
  assert.strictEqual(found.command, onPath);
  assert.strictEqual(found.tier, 'PATH');
});

test('nothing found names all three tiers', () => {
  assert.strictEqual(discovery.findServer(manifest, unset, { PATH: '' }, platform), undefined);
  const message = discovery.notFound(manifest);
  for (const tier of ['karc.server.path', 'KLF_KARC_SYSROOT', 'PATH']) {
    assert.ok(message.includes(tier), message);
  }
});

test('another driver comes from its setting, then the sysroot holding it', () => {
  const runEnv = { ...env, KLF_KSL_SYSROOT: path.join(scratch, 'ksl-sysroot') };
  const found = discovery.findRunDriver(manifest, 'ksl', unset, runEnv, platform);
  assert.strictEqual(found.command, ksl);
  assert.match(found.tier, /KLF_KSL_SYSROOT/);
  const set = (name) => (name === 'karc.drivers.ksl' ? '/opt/ksl' : undefined);
  assert.strictEqual(discovery.findRunDriver(manifest, 'ksl', set, runEnv, platform).command, '/opt/ksl');
});

test.after(() => fs.rmSync(scratch, { recursive: true, force: true }));
