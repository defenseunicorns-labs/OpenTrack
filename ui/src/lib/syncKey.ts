/** A key's fingerprint as Settings → Nodes shows it (`b22f e92b …`): pasted where the key goes, it is not one. */
export function looksLikeFingerprint(text: string): boolean {
  return /^([0-9a-f]{4}\s+){3,}[0-9a-f]{4}$/i.test(text.trim())
}
