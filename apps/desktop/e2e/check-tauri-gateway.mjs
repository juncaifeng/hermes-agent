/**
 * Ad-hoc CDP check for the running Tauri desktop app — v2.
 * Uses innerText (visible text only) to avoid CSS/style false positives.
 */
import { chromium } from '@playwright/test'

const CDP_URL = process.env.CDP_URL || 'http://127.0.0.1:9222'
const browser = await chromium.connectOverCDP(CDP_URL)
const page = browser.contexts().flatMap((c) => c.pages())[0]

await page.waitForTimeout(3000)

const visible = (await page.evaluate(() => document.body.innerText)) ?? ''
const hasComposer = await page.locator('textarea, [contenteditable="true"]').count()

const failureIndicators = [
  '连接不到网关',
  'Could not connect to Hermes gateway',
  'Desktop boot failed',
  'Lost connection to the Hermes gateway',
  'boot failed',
  'Connection settings',
]
const failures = failureIndicators.filter((s) => visible.includes(s))

console.log(`[CDP] composer count: ${hasComposer}`)
console.log(`[CDP] failure indicators in visible text: ${failures.length ? failures.join(' | ') : 'none'}`)
console.log('[CDP] visible text:')
console.log('----------------------------')
console.log(visible)
console.log('----------------------------')

const ok = hasComposer > 0 && failures.length === 0
console.log(`\n[RESULT] gateway auto-launch OK: ${ok ? 'YES' : 'NO'}`)
await browser.close()
process.exit(ok ? 0 : 1)