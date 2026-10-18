import { fromBase64, toBase64 } from 'lib0/buffer'

export const toB64 = (bytes: Uint8Array): string => toBase64(bytes)
export const fromB64 = (s: string): Uint8Array => fromBase64(s)
