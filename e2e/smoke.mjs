// Smoke test (spec 7.6): start the built app through tauri-driver and wait
// for a match row. Node built-ins only. Run by .github/workflows/smoke.yml:
//   MYMTGO_APP=target/debug/app xvfb-run node e2e/smoke.mjs
import { spawn } from 'node:child_process'
import { writeFileSync } from 'node:fs'
import { setTimeout as sleep } from 'node:timers/promises'

const app = process.env.MYMTGO_APP
const driverUrl = 'http://127.0.0.1:4444'
const timeoutMs = 90_000

async function call(method, path, body) {
  const response = await fetch(`${driverUrl}${path}`, {
    method,
    headers: { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const json = await response.json()
  if (!response.ok) throw new Error(`${method} ${path}: ${JSON.stringify(json)}`)
  return json.value
}

async function waitForDriver() {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      await call('GET', '/status')
      return
    } catch {
      await sleep(200)
    }
  }
  throw new Error('tauri-driver did not start')
}

async function matchRows(session) {
  const rows = await call('POST', `/session/${session}/elements`, {
    using: 'css selector',
    value: '[data-testid="match-row"]',
  })
  return rows.length
}

async function main() {
  if (!app) throw new Error('set MYMTGO_APP to the built app')
  const driver = spawn('tauri-driver', [], { stdio: 'inherit' })
  try {
    await waitForDriver()
    const { sessionId } = await call('POST', '/session', {
      capabilities: { alwaysMatch: { 'tauri:options': { application: app } } },
    })
    try {
      const deadline = Date.now() + timeoutMs
      while (Date.now() < deadline) {
        const count = await matchRows(sessionId)
        if (count > 0) {
          console.log(`smoke: ${count} match row(s) shown`)
          return
        }
        await sleep(1000)
      }
      const screenshot = await call('GET', `/session/${sessionId}/screenshot`)
      writeFileSync('e2e-failure.png', Buffer.from(screenshot, 'base64'))
      throw new Error(`no match row within ${timeoutMs / 1000} s (screenshot: e2e-failure.png)`)
    } finally {
      await call('DELETE', `/session/${sessionId}`).catch(() => {})
    }
  } finally {
    driver.kill()
  }
}

main().catch((error) => {
  console.error(error)
  process.exit(1)
})
