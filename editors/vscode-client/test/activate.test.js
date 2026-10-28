// The bundled client against a real server, without VS Code: a stand-in
// `vscode` module records what the client registers, and the client starts
// `klf lsp` as a generated extension's `manifest.json` tells it to. The
// editor session of KLF-EDITOR.md section 24 runs in the editor job.

const test = require('node:test');
const assert = require('node:assert');
const fs = require('fs');
const os = require('os');
const path = require('path');
const Module = require('module');

const bundle = path.join(__dirname, '..', 'dist', 'vscode-client.js');
const klf = path.join(__dirname, '..', '..', '..', 'target', 'debug', 'klf');

// Anything the client or `vscode-languageclient` reaches for: callable,
// constructible, and a disposable.
function anything() {
  const target = function () {};
  return new Proxy(target, {
    get(_, key) {
      if (key === 'then' || key === Symbol.iterator || key === Symbol.toPrimitive) {
        return undefined;
      }
      if (key === 'dispose') {
        return () => undefined;
      }
      return anything();
    },
    apply: () => anything(),
    construct: () => anything(),
  });
}

class EventEmitter {
  constructor() {
    this.listeners = [];
    this.event = (listener) => {
      this.listeners.push(listener);
      return { dispose() {} };
    };
  }
  fire(value) {
    for (const listener of this.listeners) {
      listener(value);
    }
  }
  dispose() {}
}

function standIn(settings, record) {
  const disposable = () => ({ dispose() {} });
  const lines = [];
  const vscode = {
    version: '1.82.0',
    EventEmitter,
    Disposable: class {
      static from() {
        return { dispose() {} };
      }
      dispose() {}
    },
    Uri: { parse: (text) => ({ scheme: text.split(':')[0], toString: () => text }) },
    StatusBarAlignment: { Left: 1, Right: 2 },
    ThemeColor: class {},
    ViewColumn: { Beside: -2 },
    window: {
      createOutputChannel: () => ({
        appendLine: (line) => lines.push(line),
        append: () => undefined,
        show: () => undefined,
        dispose: () => undefined,
        name: 'log',
      }),
      createStatusBarItem: () => {
        const item = { show() {}, dispose() {} };
        record.status = item;
        return item;
      },
      showErrorMessage: (message) => {
        record.errors.push(message);
        return Promise.resolve(undefined);
      },
      showWarningMessage: () => Promise.resolve(undefined),
      showInformationMessage: () => Promise.resolve(undefined),
      onDidChangeActiveTextEditor: disposable,
      activeTextEditor: undefined,
      visibleTextEditors: [],
      tabGroups: { all: [] },
    },
    workspace: {
      getConfiguration: () => ({ get: (name) => settings[name], has: () => false }),
      registerTextDocumentContentProvider: (scheme, provider) => {
        record.providers[scheme] = provider;
        return { dispose() {} };
      },
      onDidSaveTextDocument: disposable,
      onDidOpenTextDocument: disposable,
      onDidCloseTextDocument: disposable,
      onDidChangeTextDocument: disposable,
      onDidChangeConfiguration: disposable,
      onDidChangeWorkspaceFolders: disposable,
      textDocuments: [],
      workspaceFolders: undefined,
    },
    commands: {
      registerCommand: (name, handler) => {
        record.commands[name] = handler;
        return { dispose() {} };
      },
      executeCommand: () => Promise.resolve(undefined),
    },
    env: { machineId: 'test', language: 'en', appName: 'stand-in' },
  };
  return {
    lines,
    module: new Proxy(vscode, {
      get: (target, key) => (key in target ? target[key] : anything()),
    }),
  };
}

test('the client starts the server its manifest names and registers section 17.3', async (t) => {
  if (!fs.existsSync(bundle) || !fs.existsSync(klf)) {
    t.skip('needs `npm run build` and a built `target/debug/klf`');
    return;
  }
  const extension = fs.mkdtempSync(path.join(os.tmpdir(), 'klf-vscode-activate-'));
  fs.writeFileSync(
    path.join(extension, 'manifest.json'),
    JSON.stringify({
      language: 'klfd',
      displayName: 'KLF Definition',
      driver: 'klf',
      server: { arguments: ['lsp'], setting: 'klfd.server.path', environment: 'KLF_KLFD_SYSROOT' },
    }),
  );
  const record = { commands: {}, providers: {}, errors: [] };
  const vscode = standIn({ 'klfd.server.path': klf }, record);
  const load = Module._load;
  Module._load = function (request, ...rest) {
    return request === 'vscode' ? vscode.module : load.call(this, request, ...rest);
  };
  try {
    const client = require(bundle);
    await client.activate({ extensionPath: extension, subscriptions: [] });
    assert.deepStrictEqual(record.errors, []);
    const commands = [
      'klf.run',
      'klf.showOutput',
      'klf.showKir',
      'klf.showExpansion',
      'klf.reloadProjects',
      'klf.restartServer',
    ];
    for (const command of commands) {
      assert.ok(record.commands[command], `${command} is registered`);
    }
    assert.ok(record.providers['klf-virtual'], 'klf-virtual documents have a provider');
    const tier = vscode.lines.find((line) => line.startsWith('Server: '));
    assert.match(tier, /found through the setting klfd\.server\.path/);
    assert.match(record.status.text, /KLF Definition/);
    for (let tries = 0; !/serving/.test(record.status.tooltip) && tries < 50; tries++) {
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    assert.match(record.status.tooltip, /serving \d+ projects/, 'klf/status reached the item');
    await client.deactivate();
  } finally {
    Module._load = load;
    fs.rmSync(extension, { recursive: true, force: true });
  }
});
