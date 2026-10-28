// The VS Code client of every generated KLF extension (KLF-EDITOR.md
// section 19.5, DE-010). It is the same file in each extension; the
// `manifest.json` beside it says which language it serves.

const fs = require('fs');
const path = require('path');
const vscode = require('vscode');
const { LanguageClient, State } = require('vscode-languageclient/node');
const discovery = require('./discovery');
const views = require('./views');

let client;
let features = [];

function setting(name) {
  const value = vscode.workspace.getConfiguration().get(name);
  return Array.isArray(value) ? value : value || undefined;
}

// The process environment, with the login shell's PATH on macOS so the
// server sees the tools a terminal would.
function environment() {
  return { ...process.env, PATH: discovery.shellPath(process.env, process.platform) };
}

// The sysroot the server came from: the one its variable names, or the
// directory above the `bin/` the driver was found in.
function sysrootOf(manifest, command) {
  const named = process.env[manifest.server.environment];
  if (named) {
    return named;
  }
  if (path.basename(path.dirname(command)) === 'bin') {
    return path.dirname(path.dirname(command));
  }
  return undefined;
}

function offers(feature) {
  return features.includes(feature);
}

async function askStatus(status) {
  if (client && client.state === State.Running && offers('klf/status')) {
    try {
      status.update(await client.sendRequest('klf/status', {}));
    } catch {
      // A server busy restarting answers the next ask.
    }
  }
}

async function askFileInfo(status, manifest, editor) {
  if (!client || client.state !== State.Running || !offers('klf/fileInfo')) {
    status.describe(undefined);
    return;
  }
  if (!editor || editor.document.languageId !== manifest.language) {
    status.describe(undefined);
    return;
  }
  try {
    const textDocument = { uri: editor.document.uri.toString() };
    status.describe(await client.sendRequest('klf/fileInfo', { textDocument }));
  } catch {
    status.describe(undefined);
  }
}

function start(manifest, log, status, documents) {
  const found = discovery.findServer(manifest, setting, process.env, process.platform);
  if (!found) {
    const message = discovery.notFound(manifest);
    log.appendLine(message);
    status.stopped(message);
    vscode.window.showErrorMessage(message, 'Open Settings').then((choice) => {
      if (choice) {
        vscode.commands.executeCommand('workbench.action.openSettings', manifest.server.setting);
      }
    });
    return undefined;
  }
  log.appendLine(`Server: ${found.command} ${found.args.join(' ')}, found through ${found.tier}`);
  const server = {
    command: found.command,
    args: found.args,
    options: { env: environment() },
  };
  client = new LanguageClient(
    manifest.language,
    manifest.displayName,
    server,
    {
      documentSelector: [{ language: manifest.language }],
      outputChannel: log,
      synchronize: { configurationSection: [manifest.language, 'klf'] },
    },
  );
  client.onNotification('klf/status', (health) => status.update(health));
  client.onNotification('klf/shaderOutputChanged', (params) => documents.refresh(params.uris));
  client.onDidChangeState((change) => {
    if (change.newState === State.Stopped) {
      status.stopped(`${manifest.displayName}: the language server stopped`);
    }
  });
  return client.start().then(
    () => {
      const result = client.initializeResult || {};
      features = (result.klf && result.klf.features) || [];
      askStatus(status);
      askFileInfo(status, manifest, vscode.window.activeTextEditor);
      return found;
    },
    (error) => {
      const message = `${manifest.displayName}: the language server failed to start: ${error.message}`;
      log.appendLine(message);
      status.stopped(message);
      return undefined;
    },
  );
}

// `klf.run`: a driver in a terminal. The server sends one object; the
// arguments of section 17.3 may also come one by one.
function run(manifest, log, first, second, third) {
  const argument =
    typeof first === 'string' ? { driver: first, arguments: second, cwd: third } : first || {};
  const driver = argument.driver || manifest.driver;
  const found = discovery.findRunDriver(manifest, driver, setting, process.env, process.platform);
  if (!found) {
    const message =
      `${manifest.displayName}: \`${driver}\` was not found. Looked at the setting ` +
      `${manifest.language}.drivers.${driver}, the KLF_<LANG>_SYSROOT variables, and the PATH.`;
    log.appendLine(message);
    vscode.window.showErrorMessage(message);
    return;
  }
  const args = (argument.arguments || []).map(String);
  const folder = vscode.workspace.workspaceFolders && vscode.workspace.workspaceFolders[0];
  const cwd = argument.cwd || (folder && folder.uri.fsPath);
  log.appendLine(`Run: ${found.command} ${args.join(' ')}, found through ${found.tier}`);
  const terminal = vscode.window.createTerminal({
    name: `${driver} ${args.join(' ')}`.trim(),
    shellPath: found.command,
    shellArgs: args,
    cwd,
    env: { PATH: environment().PATH },
  });
  terminal.show();
}

async function activate(context) {
  const manifest = JSON.parse(
    fs.readFileSync(path.join(context.extensionPath, 'manifest.json'), 'utf8'),
  );
  const log = vscode.window.createOutputChannel(manifest.displayName);
  const status = new views.Status(manifest);
  const documents = new views.VirtualDocuments(() => client);
  let roots = [];
  const launch = async () => {
    const found = await start(manifest, log, status, documents);
    roots = found ? views.readOnlyRoots(manifest, sysrootOf(manifest, found.command)) : [];
    views.protect(roots, vscode.window.activeTextEditor);
  };
  context.subscriptions.push(
    log,
    status,
    vscode.workspace.registerTextDocumentContentProvider(views.SCHEME, documents),
    vscode.commands.registerCommand('klf.run', (...args) => run(manifest, log, ...args)),
    vscode.commands.registerCommand('klf.showOutput', (argument) => {
      if (!client || !offers('klf/shaderOutput')) {
        vscode.window.showWarningMessage(`${manifest.displayName}: no shader output views`);
        return undefined;
      }
      return views.showOutput(client, argument).catch((error) => {
        vscode.window.showErrorMessage(error.message);
      });
    }),
    vscode.commands.registerCommand('klf.showKir', () => {
      if (!client || !offers('klf/kir')) {
        vscode.window.showWarningMessage(`${manifest.displayName}: no KIR view`);
        return undefined;
      }
      return views.showKir(client, vscode.window.activeTextEditor).catch((error) => {
        vscode.window.showErrorMessage(error.message);
      });
    }),
    vscode.commands.registerCommand('klf.showExpansion', () => {
      if (!client || !offers('klf/expand')) {
        vscode.window.showWarningMessage(`${manifest.displayName}: no macro expansion view`);
        return undefined;
      }
      return views.showExpansion(client, vscode.window.activeTextEditor).catch((error) => {
        vscode.window.showErrorMessage(error.message);
      });
    }),
    vscode.commands.registerCommand('klf.reloadProjects', async () => {
      if (client && offers('klf/reloadProjects')) {
        await client.sendRequest('klf/reloadProjects', {});
      } else if (client) {
        await client.restart();
      }
      askStatus(status);
    }),
    vscode.commands.registerCommand('klf.restartServer', async () => {
      if (client) {
        const stopping = client;
        client = undefined;
        await stopping.stop().catch(() => undefined);
      }
      await launch();
    }),
    vscode.window.onDidChangeActiveTextEditor((editor) => {
      views.protect(roots, editor);
      askFileInfo(status, manifest, editor);
      askStatus(status);
    }),
    vscode.workspace.onDidSaveTextDocument(() => askStatus(status)),
  );
  await launch();
}

function deactivate() {
  return client ? client.stop() : undefined;
}

module.exports = { activate, deactivate };
