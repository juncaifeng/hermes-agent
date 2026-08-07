'use strict'

// Hermes Desktop — terminal sidecar.
//
// Runs node-pty in a headless Node child process and bridges PTY I/O to the
// Tauri main process over a JSON-lines protocol on stdin/stdout. This keeps the
// node-pty native addon inside a Node runtime (where it must live) while the
// Rust main process owns the integration.
//
//   ← stdin (one JSON object per line):
//       {"type":"start",   "id":"...", "payload":{cwd,cols,rows}}
//       {"type":"write",   "id":"...", "data":"..."}
//       {"type":"resize",  "id":"...", "cols":N,"rows":N}
//       {"type":"cwd",     "id":"..."}
//       {"type":"dispose", "id":"..."}
//       {"type":"shutdown"}
//   → stdout (one JSON object per line):
//       {"type":"ready"}
//       {"type":"started","id":"...","shell":"...","cwd":"..."}
//       {"type":"data","id":"...","data":"..."}
//       {"type":"exit","id":"...","code":N,"signal":"..."|null}
//       {"type":"cwd","id":"...","cwd":"/path"|null}
//       {"type":"error","id":"...","message":"..."}
//
// The shell-detection logic below is a direct port of the corresponding
// helpers in `electron/main.ts` (shellSpecFor / windowsShellSpec /
// terminalShellCommand / terminalShellEnv / readProcessCwd).

const fs = require('fs')
const os = require('os')
const path = require('path')
// node-pty is a native addon; the Rust side spawns us with its cwd set to the
// desktop app root so this bare require resolves from the app's node_modules.
const nodePty = require('node-pty')

const IS_WINDOWS = process.platform === 'win32'

const sessions = new Map()

// ---------------------------------------------------------------------------
// Logging
//
// stdout is reserved for the JSON protocol to the Rust main process — everyday
// diagnostics MUST go to stderr, otherwise they'd corrupt the protocol stream.
// INFO is on always; DEBUG is opt-in via HERMES_TERMINAL_DEBUG=1 (data chunks
// are high-frequency and only logged at DEBUG to avoid log spam).
// ---------------------------------------------------------------------------

const DEBUG = process.env.HERMES_TERMINAL_DEBUG === '1' || process.env.HERMES_TERMINAL_DEBUG === 'true'

function ts() {
  return new Date().toISOString()
}

function logInfo(...args) {
  process.stderr.write(`[terminal-sidecar ${ts()}] INFO ${args.join(' ')}\n`)
}

function logDebug(...args) {
  if (DEBUG) {
    process.stderr.write(`[terminal-sidecar ${ts()}] DEBUG ${args.join(' ')}\n`)
  }
}

function logError(...args) {
  process.stderr.write(`[terminal-sidecar ${ts()}] ERROR ${args.join(' ')}\n`)
}

// ---------------------------------------------------------------------------
// Shell detection (port of electron/main.ts)
// ---------------------------------------------------------------------------

function fileExists(p) {
  try {
    fs.accessSync(p)
    return true
  } catch {
    return false
  }
}

function isExecutableFile(filePath) {
  if (!filePath || !path.isAbsolute(filePath)) {
    return false
  }
  try {
    fs.accessSync(filePath, fs.constants.X_OK)
    return true
  } catch {
    return false
  }
}

function buildPathExtCandidates(pathext, isWindows) {
  if (!isWindows) {
    return ['']
  }
  return [...(pathext || '.COM;.EXE;.BAT;.CMD').split(';').filter(Boolean), '']
}

function findOnPath(command) {
  if (!command) {
    return null
  }
  if (path.isAbsolute(command) || command.includes(path.sep) || command.includes('/')) {
    return fileExists(command) ? command : null
  }
  const pathEntries = String(process.env.PATH || '')
    .split(path.delimiter)
    .filter(Boolean)
  const extensions = buildPathExtCandidates(process.env.PATHEXT, IS_WINDOWS)
  for (const entry of pathEntries) {
    for (const extension of extensions) {
      const candidate = path.join(entry, `${command}${extension}`)
      if (fileExists(candidate)) {
        return candidate
      }
    }
  }
  return null
}

function posixShellSpec(shellPath) {
  const shellName = path.basename(shellPath)
  const interactiveArgs = shellName.includes('zsh') || shellName.includes('bash') ? ['-il'] : ['-i']
  return { args: interactiveArgs, command: shellPath, name: shellName }
}

function windowsPowerShellPath() {
  const systemRoot = process.env.SystemRoot || process.env.windir || 'C:\\Windows'
  const builtin = path.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe')
  return isExecutableFile(builtin) ? builtin : findOnPath('powershell.exe')
}

function shellSpecFor(shellPath) {
  const name = path.basename(shellPath).toLowerCase()
  if (name.startsWith('pwsh') || name.startsWith('powershell')) {
    return { args: ['-NoLogo'], command: shellPath, name }
  }
  if (name.startsWith('cmd')) {
    return { args: [], command: shellPath, name }
  }
  return posixShellSpec(shellPath)
}

function windowsShellSpec() {
  const command =
    findOnPath('pwsh.exe') || findOnPath('pwsh') || windowsPowerShellPath() || process.env.COMSPEC || 'cmd.exe'
  return shellSpecFor(command)
}

function terminalShellCommand() {
  const override = (process.env.HERMES_DESKTOP_SHELL || (IS_WINDOWS ? '' : process.env.SHELL) || '').trim()
  if (override) {
    const resolved = isExecutableFile(override) ? override : findOnPath(override)
    if (resolved) {
      return shellSpecFor(resolved)
    }
  }
  if (IS_WINDOWS) {
    return windowsShellSpec()
  }
  const shellPath = ['/bin/zsh', '/bin/bash', '/bin/sh'].find((candidate) => isExecutableFile(candidate))
  return posixShellSpec(shellPath || '/bin/sh')
}

function safeTerminalCwd(cwd) {
  const home = process.env.HOME || process.env.USERPROFILE || os.homedir()
  const candidate = path.resolve(String(cwd || home))
  try {
    const stat = fs.statSync(candidate)
    return stat.isDirectory() ? candidate : path.dirname(candidate)
  } catch {
    return home
  }
}

function terminalShellEnv() {
  const env = { ...process.env }
  for (const key of Object.keys(env)) {
    if (key === 'npm_config_prefix' || key.startsWith('npm_config_') || key.startsWith('npm_package_')) {
      delete env[key]
    }
  }
  delete env.NO_COLOR
  delete env.FORCE_COLOR
  delete env.COLORFGBG
  env.COLORTERM = 'truecolor'
  env.LC_CTYPE = env.LC_CTYPE || 'UTF-8'
  env.TERM = 'xterm-256color'
  env.TERM_PROGRAM = 'Hermes'
  env.TERM_PROGRAM_VERSION = process.env.HERMES_VERSION || 'dev'
  env.HERMES_DESKTOP_TERMINAL = '1'
  return env
}

// Best-effort read of a live PTY child's cwd. Shell-agnostic on POSIX; windows
// has no cheap per-process cwd query, so it returns null there.
function readProcessCwd(pid) {
  return new Promise((resolve) => {
    if (!Number.isInteger(pid) || pid <= 0) {
      resolve(null)
      return
    }
    if (process.platform === 'linux') {
      fs.promises
        .readlink(`/proc/${pid}/cwd`)
        .then((target) => resolve(target || null))
        .catch(() => resolve(null))
      return
    }
    if (process.platform === 'darwin') {
      const { execFile } = require('child_process')
      execFile('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'], { timeout: 2000 }, (err, stdout) => {
        if (err) {
          resolve(null)
          return
        }
        const line = String(stdout || '')
          .split('\n')
          .find((entry) => entry.startsWith('n'))
        resolve(line ? line.slice(1) : null)
      })
      return
    }
    resolve(null)
  })
}

// ---------------------------------------------------------------------------
// Protocol
// ---------------------------------------------------------------------------

function emit(obj) {
  process.stdout.write(JSON.stringify(obj) + '\n')
}

function start(id, payload) {
  try {
    const { args, command, name } = terminalShellCommand()
    const cwd = safeTerminalCwd(payload && payload.cwd)
    const cols = Math.max(2, Number.parseInt(String((payload && payload.cols) || 80), 10) || 80)
    const rows = Math.max(2, Number.parseInt(String((payload && payload.rows) || 24), 10) || 24)
    logInfo(`start id=${id} shell=${JSON.stringify(name)} command=${JSON.stringify(command)} args=${JSON.stringify(args)} cwd=${JSON.stringify(cwd)} cols=${cols} rows=${rows}`)
    const baseOpts = {
      cols,
      cwd,
      env: terminalShellEnv(),
      name: 'xterm-256color',
      rows,
    }
    let pty
    try {
      pty = nodePty.spawn(command, args, baseOpts)
    } catch (err) {
      // ConPTY creation can fail transiently on Windows (e.g. console resource
      // exhaustion under load). Fall back to the winpty backend, which is more
      // tolerant, rather than failing the whole session.
      if (IS_WINDOWS && /conpty/i.test(String(err && err.message))) {
        logInfo(`start id=${id} conpty FAILED (${err.message}), retrying with winpty backend`)
        pty = nodePty.spawn(command, args, { ...baseOpts, useConpty: false })
      } else {
        throw err
      }
    }
    sessions.set(id, pty)
    logInfo(`started id=${id} pid=${pty.pid}`)
    pty.onData((data) => {
      logDebug(`data id=${id} bytes=${Buffer.byteLength(data, 'utf8')}`)
      emit({ type: 'data', id, data })
    })
    pty.onExit(({ exitCode, signal }) => {
      sessions.delete(id)
      logInfo(`exit id=${id} code=${exitCode} signal=${JSON.stringify(signal || null)}`)
      emit({ type: 'exit', id, code: exitCode, signal: signal || null })
    })
    emit({ type: 'started', id, shell: name, cwd })
  } catch (err) {
    const message = err && err.message ? err.message : String(err)
    logError(`start id=${id} FAILED: ${message}`)
    emit({ type: 'error', id, message })
  }
}

function write(id, data) {
  const pty = sessions.get(id)
  if (!pty) {
    logDebug(`write id=${id} MISSING session (ignored)`)
    return
  }
  try {
    logDebug(`write id=${id} bytes=${Buffer.byteLength(String(data || ''), 'utf8')}`)
    pty.write(String(data || ''))
  } catch (err) {
    logError(`write id=${id} FAILED: ${err && err.message ? err.message : String(err)}`)
  }
}

function resize(id, cols, rows) {
  const pty = sessions.get(id)
  if (!pty) {
    logDebug(`resize id=${id} MISSING session (ignored)`)
    return
  }
  cols = Math.max(2, Number.parseInt(String(cols || 80), 10) || 80)
  rows = Math.max(2, Number.parseInt(String(rows || 24), 10) || 24)
  try {
    pty.resize(cols, rows)
    logDebug(`resize id=${id} cols=${cols} rows=${rows}`)
  } catch (err) {
    logError(`resize id=${id} FAILED: ${err && err.message ? err.message : String(err)}`)
  }
}

async function cwd(id) {
  const pty = sessions.get(id)
  if (!pty) {
    logDebug(`cwd id=${id} MISSING session (replied null)`)
    emit({ type: 'cwd', id, cwd: null })
    return
  }
  const c = await readProcessCwd(pty.pid)
  logDebug(`cwd id=${id} pid=${pty.pid} -> ${JSON.stringify(c)}`)
  emit({ type: 'cwd', id, cwd: c })
}

function dispose(id) {
  const pty = sessions.get(id)
  if (!pty) {
    logDebug(`dispose id=${id} MISSING session (no-op)`)
    return
  }
  try {
    pty.kill()
    logInfo(`dispose id=${id} killed pid=${pty.pid}`)
  } catch (err) {
    logError(`dispose id=${id} kill FAILED: ${err && err.message ? err.message : String(err)}`)
  }
  sessions.delete(id)
}

function shutdown() {
  logInfo(`shutdown: killing ${sessions.size} session(s)`)
  for (const [id, pty] of sessions.entries()) {
    try {
      pty.kill()
      logDebug(`shutdown: killed id=${id} pid=${pty.pid}`)
    } catch (err) {
      logError(`shutdown: kill id=${id} FAILED: ${err && err.message ? err.message : String(err)}`)
    }
  }
  sessions.clear()
  process.exit(0)
}

function handle(msg) {
  logDebug(`recv type=${msg.type} id=${msg.id || ''}`)
  switch (msg.type) {
    case 'start':
      return start(msg.id, msg.payload || {})
    case 'write':
      return write(msg.id, msg.data)
    case 'resize':
      return resize(msg.id, msg.cols, msg.rows)
    case 'cwd':
      return cwd(msg.id)
    case 'dispose':
      return dispose(msg.id)
    case 'shutdown':
      return shutdown()
    default:
      logError(`recv unknown message type=${JSON.stringify(msg.type)}`)
      // Unknown message; ignore.
  }
}

let buffer = ''
process.stdin.setEncoding('utf8')
process.stdin.on('data', (chunk) => {
  buffer += chunk
  let idx
  while ((idx = buffer.indexOf('\n')) >= 0) {
    const line = buffer.slice(0, idx).trim()
    buffer = buffer.slice(idx + 1)
    if (!line) {
      continue
    }
    let msg
    try {
      msg = JSON.parse(line)
    } catch (parseErr) {
      logError(`recv unparsable line: ${line.slice(0, 200)}`)
      continue
    }
    handle(msg)
  }
})
process.stdin.on('end', shutdown)

process.on('SIGTERM', shutdown)
process.on('SIGINT', shutdown)

logInfo(`boot node=${process.version} platform=${process.platform} arch=${process.arch} pid=${process.pid}`)
emit({ type: 'ready' })

module.exports = { handle }