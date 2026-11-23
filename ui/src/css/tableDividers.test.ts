import { describe, expect, it } from 'vitest'
import appCss from './app.css?raw'

// Issue #331: WebKit (the macOS and Linux webviews) paints a collapsed
// table border once from each side, doubling a translucent divider. Tables
// use separate borders, and cells carry their row's top and bottom border.
function rule(selector: string): string {
  const start = appCss.indexOf(`\n    ${selector} {`)
  expect(start, `rule for ${selector}`).toBeGreaterThan(-1)
  return appCss.slice(start, appCss.indexOf('}', start))
}

describe('table dividers', () => {
  it('draws table borders in the separate model', () => {
    const table = rule('table')
    expect(table).toContain('border-collapse: separate;')
    expect(table).toContain('border-spacing: 0;')
  })

  it('moves row borders onto the cells', () => {
    const cells = rule('td,\n    th')
    expect(cells).toContain('border-top: inherit;')
    expect(cells).toContain('border-bottom: inherit;')
    expect(rule('tfoot > tr:first-child')).toContain('border-top: inherit;')
  })

  it('never collapses a table border again', () => {
    expect(appCss).not.toMatch(/border-collapse(?!: separate)/)
  })
})
