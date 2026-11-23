const SECRET_KEYS = new Set([
  'ticket',
  'token',
  'accesstoken',
  'refreshtoken',
  'idtoken',
  'sessiontoken',
  'authorization',
  'cookie',
  'setcookie',
  'password',
  'secret',
  'email',
  'jwt',
  'apikey',
  'sessionid',
  'csrf',
  'xsrf',
])

// Secret-named: in SECRET_KEYS, or ending in "token" or "ticket" (`authToken`, `gameTicket`).
// `tokens` (a card list) does not end in "token", so game data under it survives.
function isSecretKey(key: string): boolean {
  const name = key.toLowerCase().replace(/[_-]/g, '')
  return SECRET_KEYS.has(name) || name.endsWith('token') || name.endsWith('ticket')
}

// Deep copy that drops secret-named keys. Keys match ignoring case, "_" and "-".
// Fails closed: only a boolean value (an `isToken: true` flag) survives under a secret key.
export function stripSecrets(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map((item) => stripSecrets(item))
  }
  if (value !== null && typeof value === 'object') {
    const out: Record<string, unknown> = {}
    for (const [key, inner] of Object.entries(value)) {
      if (isSecretKey(key) && typeof inner !== 'boolean') {
        continue
      }
      // defineProperty keeps an own "__proto__" key as plain data.
      Object.defineProperty(out, key, {
        value: stripSecrets(inner),
        enumerable: true,
        writable: true,
        configurable: true,
      })
    }
    return out
  }
  return value
}

// Path only: no origin, query or hash.
export function stripUrl(url: string): string {
  try {
    return new URL(url).pathname
  } catch {
    return url.split('#')[0].split('?')[0]
  }
}
