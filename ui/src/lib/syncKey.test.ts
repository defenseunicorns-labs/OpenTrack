import { describe, expect, it } from 'vitest'
import { looksLikeFingerprint } from './syncKey'

describe('a key pasted to pin', () => {
  it('is told apart from a fingerprint', () => {
    expect(looksLikeFingerprint('b22f e92b f60c 12db e070 4f09 4ec2 7006')).toBe(true)
    expect(looksLikeFingerprint('  B22F E92B F60C 12DB E070 4F09 4EC2 7006\n')).toBe(true)
    expect(looksLikeFingerprint('ed25519:sx/pK2DxzHq4l2s0bD9lJqXJmZ3cRk6vYp4T1nH8wQE=')).toBe(false)
    expect(looksLikeFingerprint('sx/pK2DxzHq4l2s0bD9lJqXJmZ3cRk6vYp4T1nH8wQE=')).toBe(false)
  })
})
