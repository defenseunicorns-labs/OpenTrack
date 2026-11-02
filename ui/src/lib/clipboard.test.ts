import { afterEach, describe, expect, it, vi } from 'vitest'
import { copyText } from './clipboard'

/** A stand-in document with one textarea, and what `copy` copied. */
function fakeDocument(copies: boolean) {
  const copied: string[] = []
  let area: { value: string; removed: boolean } | null = null
  const doc = {
    createElement: () => {
      area = { value: '', removed: false }
      return Object.assign(area, { setAttribute: () => {}, style: {}, select: () => {}, remove: () => void (area!.removed = true) })
    },
    body: { appendChild: () => {} },
    execCommand: () => {
      if (copies) copied.push(area!.value)
      return copies
    },
  }
  return { doc, copied, area: () => area }
}

afterEach(() => vi.unstubAllGlobals())

describe('copyText', () => {
  it('uses the Clipboard API where the page is secure', async () => {
    const writeText = vi.fn(async () => {})
    vi.stubGlobal('isSecureContext', true)
    vi.stubGlobal('navigator', { clipboard: { writeText } })
    await copyText('ed25519:abc')
    expect(writeText).toHaveBeenCalledWith('ed25519:abc')
  })

  it('copies a selection on plain HTTP, where there is no Clipboard API', async () => {
    const f = fakeDocument(true)
    vi.stubGlobal('isSecureContext', false)
    vi.stubGlobal('navigator', {})
    vi.stubGlobal('document', f.doc)
    await copyText('ed25519:abc')
    expect(f.copied).toEqual(['ed25519:abc'])
    expect(f.area()?.removed).toBe(true)
  })

  it('rejects when the browser will not copy', async () => {
    const f = fakeDocument(false)
    vi.stubGlobal('isSecureContext', false)
    vi.stubGlobal('navigator', {})
    vi.stubGlobal('document', f.doc)
    await expect(copyText('x')).rejects.toThrow()
    expect(f.area()?.removed).toBe(true)
  })
})
