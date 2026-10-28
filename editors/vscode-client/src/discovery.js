// Finding a driver (KLF-EDITOR.md section 19.6). The server and every
// driver `klf.run` starts are found the same way, and the tier that found
// one is reported so a wrong pick can be traced.

const fs = require('fs');
const path = require('path');
const childProcess = require('child_process');

let loginPath;

// The PATH of the user's login shell. A macOS app started from the Dock
// inherits launchd's short PATH, without what the shell profile adds.
function shellPath(env, platform) {
  if (platform !== 'darwin') {
    return env.PATH || '';
  }
  if (loginPath === undefined) {
    loginPath = '';
    const shell = env.SHELL || '/bin/zsh';
    try {
      const result = childProcess.spawnSync(shell, ['-l', '-c', 'printf %s "$PATH"'], {
        encoding: 'utf8',
        timeout: 5000,
      });
      if (result.status === 0) {
        loginPath = result.stdout.trim();
      }
    } catch {
      loginPath = '';
    }
  }
  const parts = (env.PATH || '').split(path.delimiter).concat(loginPath.split(path.delimiter));
  return [...new Set(parts.filter((part) => part))].join(path.delimiter);
}

function executable(file, platform) {
  const candidates = platform === 'win32' ? [file + '.exe', file] : [file];
  return candidates.find((candidate) => {
    try {
      return fs.statSync(candidate).isFile();
    } catch {
      return false;
    }
  });
}

function onPath(driver, searchPath, platform) {
  for (const directory of searchPath.split(path.delimiter)) {
    if (directory) {
      const found = executable(path.join(directory, driver), platform);
      if (found) {
        return found;
      }
    }
  }
  return undefined;
}

// The language server of the extension's own language.
// `setting(name)` reads a configuration value of the language.
function findServer(manifest, setting, env, platform) {
  const configured = setting(manifest.server.setting);
  if (configured) {
    const extra = setting(`${manifest.language}.server.arguments`) || [];
    return {
      command: configured,
      args: manifest.server.arguments.concat(extra),
      tier: `the setting ${manifest.server.setting}`,
    };
  }
  const found = findDriver(manifest.driver, manifest.server.environment, env, platform);
  if (found) {
    return { command: found.command, args: manifest.server.arguments, tier: found.tier };
  }
  return undefined;
}

// A driver found through its sysroot variable, then the PATH.
function findDriver(driver, variable, env, platform) {
  const sysroot = env[variable];
  if (sysroot) {
    const found = executable(path.join(sysroot, 'bin', driver), platform);
    if (found) {
      return { command: found, tier: `${variable} (${sysroot})` };
    }
  }
  const found = onPath(driver, shellPath(env, platform), platform);
  if (found) {
    return { command: found, tier: 'PATH' };
  }
  return undefined;
}

// A driver `klf.run` names. Another language's driver (`ksl` from a Kira
// project) lives in that language's sysroot; the client does not know which
// language that is, so it takes the sysroot variable that holds the driver.
function findRunDriver(manifest, driver, setting, env, platform) {
  const configured = setting(`${manifest.language}.drivers.${driver}`);
  if (configured) {
    return { command: configured, tier: `the setting ${manifest.language}.drivers.${driver}` };
  }
  if (driver === manifest.driver) {
    const server = findServer(manifest, setting, env, platform);
    return server && { command: server.command, tier: server.tier };
  }
  const variables = Object.keys(env).filter((name) => /^KLF_[A-Z0-9_]+_SYSROOT$/.test(name));
  for (const variable of variables) {
    const found = executable(path.join(env[variable], 'bin', driver), platform);
    if (found) {
      return { command: found, tier: `${variable} (${env[variable]})` };
    }
  }
  return findDriver(driver, '', env, platform);
}

// The message shown when no tier finds the server.
function notFound(manifest) {
  return (
    `${manifest.displayName}: the language server \`${manifest.driver}\` was not found. ` +
    `Looked at the setting ${manifest.server.setting}, the environment variable ` +
    `${manifest.server.environment}, and the PATH. Set ${manifest.server.setting} ` +
    'to the driver to use.'
  );
}

module.exports = { findServer, findRunDriver, notFound, shellPath };
