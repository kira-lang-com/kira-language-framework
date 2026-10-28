// What the client shows beside the server's answers (KLF-EDITOR.md section
// 19.5): `klf-virtual:` documents, shader output views, the status bar item,
// and sysroot sources opened read-only.

const path = require('path');
const vscode = require('vscode');

const SCHEME = 'klf-virtual';

// The text of `klf-virtual:` documents, asked of the server each time one
// opens or moves to a new revision.
class VirtualDocuments {
  constructor(server) {
    this.server = server;
    this.changes = new vscode.EventEmitter();
    this.onDidChange = this.changes.event;
  }

  async provideTextDocumentContent(uri) {
    const client = this.server();
    if (!client) {
      return '';
    }
    const answer = await client.sendRequest('klf/virtualFile', { uri: uri.toString(true) });
    const document = vscode.workspace.textDocuments.find(
      (open) => open.uri.toString() === uri.toString(),
    );
    if (document && answer.languageId && document.languageId !== answer.languageId) {
      vscode.languages.setTextDocumentLanguage(document, answer.languageId);
    }
    return answer.text;
  }

  // `klf/shaderOutputChanged`: the views whose text moved.
  refresh(uris) {
    for (const uri of uris) {
      this.changes.fire(vscode.Uri.parse(uri, true));
    }
  }
}

const KINDS = ['source', 'ksir', 'reflection'];

// `klf.showOutput`: one view of a shader's output, beside the source.
async function showOutput(client, argument) {
  const listed = argument.targets;
  const targets = (Array.isArray(listed) ? listed : String(listed || '').split(','))
    .map((target) => String(target).trim())
    .filter((target) => target);
  const target =
    targets.length > 1
      ? await vscode.window.showQuickPick(targets, { placeHolder: `${argument.shader}: target` })
      : targets[0];
  if (!target) {
    return;
  }
  const kind = await vscode.window.showQuickPick(KINDS, {
    placeHolder: `${argument.shader} for ${target}: view`,
  });
  if (!kind) {
    return;
  }
  const answer = await client.sendRequest('klf/shaderOutput', {
    textDocument: argument.textDocument,
    shader: argument.shader,
    target,
    stage: argument.stage,
    kind,
  });
  const document = await vscode.workspace.openTextDocument(vscode.Uri.parse(answer.uri, true));
  await vscode.languages.setTextDocumentLanguage(document, answer.languageId);
  await vscode.window.showTextDocument(document, {
    viewColumn: vscode.ViewColumn.Beside,
    preview: false,
  });
}

// `klf.showKir`: the KIR of the function at the cursor (KLF-EDITOR.md
// section 13.7), raw or after the mandatory passes, beside the source.
async function showKir(client, editor) {
  if (!editor) {
    return;
  }
  const stage = await vscode.window.showQuickPick(['raw', 'canonical'], { placeHolder: 'KIR' });
  if (!stage) {
    return;
  }
  const answer = await client.sendRequest('klf/kir', {
    textDocument: { uri: editor.document.uri.toString() },
    position: editor.selection.active,
    canonical: stage === 'canonical',
  });
  if (!answer) {
    vscode.window.showInformationMessage('No function is named at or before the cursor');
    return;
  }
  const document = await vscode.workspace.openTextDocument(vscode.Uri.parse(answer.uri, true));
  await vscode.window.showTextDocument(document, {
    viewColumn: vscode.ViewColumn.Beside,
    preview: false,
  });
}

// `klf.showExpansion`: the text the innermost macro expansion at the cursor
// produced (KLF-EDITOR.md section 13.6), beside the source.
async function showExpansion(client, editor) {
  if (!editor) {
    return;
  }
  const answer = await client.sendRequest('klf/expand', {
    textDocument: { uri: editor.document.uri.toString() },
    position: editor.selection.active,
  });
  if (!answer) {
    vscode.window.showInformationMessage('No macro expansion holds the cursor');
    return;
  }
  const document = await vscode.workspace.openTextDocument(vscode.Uri.parse(answer.uri, true));
  await vscode.window.showTextDocument(document, {
    viewColumn: vscode.ViewColumn.Beside,
    preview: false,
  });
}

// The status bar item: the server's health from `klf/status`, and the
// active file's project from `klf/fileInfo`.
class Status {
  constructor(manifest) {
    this.manifest = manifest;
    this.item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 0);
    this.item.command = 'klf.reloadProjects';
    this.health = { health: 'ok', message: '', indexing: false };
    this.file = undefined;
    this.show();
  }

  update(health) {
    this.health = health;
    this.show();
  }

  describe(file) {
    this.file = file;
    this.show();
  }

  stopped(message) {
    this.health = { health: 'error', message, indexing: false };
    this.file = undefined;
    this.show();
  }

  show() {
    const { health, message, indexing } = this.health;
    const icon = indexing
      ? '$(sync~spin)'
      : health === 'error'
        ? '$(error)'
        : health === 'warning'
          ? '$(warning)'
          : '$(check)';
    this.item.text = `${icon} ${this.manifest.displayName}`;
    this.item.backgroundColor =
      health === 'error'
        ? new vscode.ThemeColor('statusBarItem.errorBackground')
        : health === 'warning'
          ? new vscode.ThemeColor('statusBarItem.warningBackground')
          : undefined;
    const lines = [message || this.manifest.displayName];
    if (this.file) {
      lines.push('', `Project: ${this.file.project}`);
      for (const [label, value] of [
        ['File mode', this.file.fileMode],
        ['Memory', this.file.memory],
        ['Convention', this.file.convention],
        ['Unit', this.file.unit],
      ]) {
        if (value !== undefined && value !== null && value !== '') {
          lines.push(`${label}: ${value}`);
        }
      }
    }
    this.item.tooltip = lines.join('\n');
    this.item.show();
  }

  dispose() {
    this.item.dispose();
  }
}

// The directories whose files open read-only: the sysroot's sources and
// library (KLF-EDITOR.md section 11.3 item 5).
function readOnlyRoots(manifest, sysroot) {
  if (!sysroot) {
    return [];
  }
  const share = path.join(sysroot, 'share', manifest.language);
  return [path.join(share, 'src'), path.join(share, 'library')];
}

function isReadOnly(roots, uri) {
  if (uri.scheme !== 'file') {
    return false;
  }
  const file = path.normalize(uri.fsPath);
  return roots.some((root) => file.startsWith(root + path.sep));
}

// Marks the active editor read-only when it shows a sysroot file. VS Code
// keeps the mark for the rest of the session.
async function protect(roots, editor) {
  if (editor && isReadOnly(roots, editor.document.uri)) {
    await vscode.commands.executeCommand('workbench.action.files.setActiveEditorReadonlyInSession');
  }
}

module.exports = {
  SCHEME,
  VirtualDocuments,
  showOutput,
  showKir,
  showExpansion,
  Status,
  readOnlyRoots,
  isReadOnly,
  protect,
};
