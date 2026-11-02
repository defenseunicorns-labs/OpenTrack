/**
 * Put `text` on the clipboard. The Clipboard API exists only on HTTPS or localhost, so on plain
 * HTTP (a node reached by its address) this falls back to copying a selection. Rejects when
 * neither works, for the caller to say so.
 */
export async function copyText(text: string): Promise<void> {
  if (globalThis.isSecureContext && navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text)
    return
  }
  const area = document.createElement('textarea')
  area.value = text
  area.setAttribute('readonly', '')
  area.style.position = 'fixed'
  area.style.opacity = '0'
  document.body.appendChild(area)
  try {
    area.select()
    if (!document.execCommand('copy')) throw new Error('copy refused')
  } finally {
    area.remove()
  }
}
