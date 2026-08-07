import { spawn } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const here = path.dirname(fileURLToPath(import.meta.url))
const script = path.join(here, 'terminal-sidecar.cjs')
const child = spawn(process.execPath, [script], { stdio: ['pipe', 'pipe', 'inherit'] })

let buffer = ''
let sentWrite = false
let sentCwd = false

child.stdout.setEncoding('utf8')
child.stdout.on('data', (chunk) => {
  buffer += chunk
  let idx
  while ((idx = buffer.indexOf('\n')) >= 0) {
    const line = buffer.slice(0, idx).trim()
    buffer = buffer.slice(idx + 1)
    if (!line) continue
    let msg
    try {
      msg = JSON.parse(line)
    } catch {
      console.log('UNPARSABLE', line)
      continue
    }
    console.log(`EVENT ${msg.type.padEnd(8)} ${JSON.stringify(msg).slice(0, 160)}`)

    if (msg.type === 'ready') {
      child.stdin.write(JSON.stringify({ type: 'start', id: 'test-1', payload: { cols: 80, rows: 24 } }) + '\n')
    } else if (msg.type === 'started') {
      if (!sentWrite) {
        sentWrite = true
        child.stdin.write(JSON.stringify({ type: 'write', id: 'test-1', data: 'echo hello-from-hermes\r\n' }) + '\n')
      }
      if (!sentCwd) {
        sentCwd = true
        child.stdin.write(JSON.stringify({ type: 'cwd', id: 'test-1' }) + '\n')
      }
      setTimeout(() => {
        child.stdin.write(JSON.stringify({ type: 'resize', id: 'test-1', cols: 100, rows: 40 }) + '\n')
        child.stdin.write(JSON.stringify({ type: 'dispose', id: 'test-1' }) + '\n')
        setTimeout(() => {
          child.stdin.end()
        }, 400)
      }, 1200)
    }
  }
})

child.on('exit', (code, signal) => {
  console.log('SIDECAR_EXIT', code, signal)
  process.exit(0)
})

setTimeout(() => {
  console.log('TIMEOUT')
  child.kill()
  process.exit(1)
}, 15000)