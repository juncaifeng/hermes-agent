/**
 * CDP check for the loopback asset-server origin (方案四 regression guard).
 *
 * Asserts the running Tauri app's renderer is loaded from a loopback HTTP
 * origin (http://127.0.0.1:<port>) — not the tauri:// custom protocol — and
 * that the gateway boot succeeded (composer visible, no failure overlay).
 *
 * Launch the app with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
 * first, then run: node e2e/check-loopback-origin.mjs
 */
import { chromium } from '@playwright/test'

const CDP_URL = process.env.CDP_URL || 'http://127.0.0.1:9222'
const browser = await chromium.connectOverCDP(CDP_URL)
const page = browser.contexts().flatMap((c) => c.pages())[0]

if (!page) {
  console.log('[CDP] no page found — is the app up?')
  process.exit(1)
}

await page.waitForTimeout(8000)

const href = await page.evaluate(() => window.location.href)
const visible = (await page.evaluate(() => document.body.innerText)) ?? ''
const hasComposer = await page.locator('textarea, [contenteditable="true"]').count()

const failureIndicators = [
  '连接不到网关',
  'Could not connect to Hermes gateway',
  'Desktop boot failed',
  'Lost connection to the Hermes gateway',
  'Desktop IPC bridge is unavailable',
]
const failures = failureIndicators.filter((s) => visible.includes(s))

const loopback = /^http:\/\/(127\.0\.0\.1|localhost)(:\d+)?\//.test(href)

console.log(`[CDP] page href: ${href}`)
console.log(`[CDP] loopback origin: ${loopback ? 'YES' : 'NO'}`)
console.log(`[CDP] composer count: ${hasComposer}`)
console.log(`[CDP] failure indicators: ${failures.length ? failures.join(' | ') : 'none'}`)

const ok = loopback && hasComposer > 0 && failures.length === 0
console.log(`\n[RESULT] loopback origin + gateway boot OK: ${ok ? 'YES' : 'NO'}`)
await browser.close()
process.exit(ok ? 0 : 1)
