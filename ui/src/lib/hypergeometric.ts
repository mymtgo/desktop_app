// Hypergeometric draw odds, ported unchanged from 0.x
// `composables/useHypergeometric.ts`.

export interface HypergeometricResult {
  /** P(X >= k): chance to draw k or more successes. */
  atLeast: number
  /** P(X = k): chance to draw exactly k successes. */
  exactly: number
  /** P(X <= k): chance to draw k or fewer successes. */
  atMost: number
  /** P(X = 0): chance to draw none of the successes. */
  zero: number
}

const EMPTY_RESULT: HypergeometricResult = { atLeast: 0, exactly: 0, atMost: 0, zero: 0 }

/**
 * Log-gamma via the Lanczos approximation. Binomial coefficients are worked
 * out in log space so C(N, n) stays finite for large populations.
 */
function logGamma(x: number): number {
  const g = 7
  const c = [
    0.99999999999980993, 676.5203681218851, -1259.1392167224028, 771.32342877765313, -176.61502916214059,
    12.507343278686905, -0.13857109526572012, 9.9843695780195716e-6, 1.5056327351493116e-7,
  ]

  if (x < 0.5) {
    // Reflection formula for x < 0.5.
    return Math.log(Math.PI / Math.sin(Math.PI * x)) - logGamma(1 - x)
  }

  x -= 1
  let a = c[0]
  const t = x + g + 0.5
  for (let i = 1; i < g + 2; i++) {
    a += c[i] / (x + i)
  }

  return 0.5 * Math.log(2 * Math.PI) + (x + 0.5) * Math.log(t) - t + Math.log(a)
}

/**
 * log(C(n, r)) through log-gamma. Invalid inputs give -Infinity, so the
 * probability built from it is 0 rather than NaN.
 */
function logChoose(n: number, r: number): number {
  if (r < 0 || r > n || n < 0) {
    return Number.NEGATIVE_INFINITY
  }
  return logGamma(n + 1) - logGamma(r + 1) - logGamma(n - r + 1)
}

/** P(X = i) = C(K, i) * C(N - K, n - i) / C(N, n). */
function pmf(N: number, K: number, n: number, i: number): number {
  const logP = logChoose(K, i) + logChoose(N - K, n - i) - logChoose(N, n)
  if (!Number.isFinite(logP)) {
    return 0
  }
  return Math.exp(logP)
}

/**
 * The odds of drawing a chosen card.
 *
 * - `N`: population size (cards in the deck).
 * - `K`: successes in the population (copies of the card).
 * - `n`: sample size (cards drawn).
 * - `k`: successes in the sample (copies wanted).
 *
 * Every value is between 0 and 1 and never NaN. Inputs are rounded down,
 * anything invalid counts as 0, and each is clamped to what is possible
 * (`K` and `n` to `N`, `k` to the smaller of `n` and `K`).
 */
export function hypergeometric(N: number, K: number, n: number, k: number): HypergeometricResult {
  N = Number.isFinite(N) ? Math.max(0, Math.floor(N)) : 0
  K = Number.isFinite(K) ? Math.max(0, Math.floor(K)) : 0
  n = Number.isFinite(n) ? Math.max(0, Math.floor(n)) : 0
  k = Number.isFinite(k) ? Math.max(0, Math.floor(k)) : 0

  if (N === 0 || n === 0) {
    return { ...EMPTY_RESULT }
  }

  K = Math.min(K, N)
  n = Math.min(n, N)
  k = Math.min(k, Math.min(n, K))

  const maxSuccesses = Math.min(n, K)

  let exactly = 0
  let atMost = 0
  let atLeast = 0
  let zero = 0

  for (let i = 0; i <= maxSuccesses; i++) {
    const p = pmf(N, K, n, i)
    if (i === 0) {
      zero = p
    }
    if (i === k) {
      exactly = p
    }
    if (i <= k) {
      atMost += p
    }
    if (i >= k) {
      atLeast += p
    }
  }

  // Guard against tiny floating point overshoot.
  const clamp01 = (v: number): number => Math.min(1, Math.max(0, v))

  return {
    atLeast: clamp01(atLeast),
    exactly: clamp01(exactly),
    atMost: clamp01(atMost),
    zero: clamp01(zero),
  }
}

/** A probability as a percentage with one decimal (0.x calculator). */
export function formatPercent(value: number): string {
  return `${(value * 100).toFixed(1)}%`
}
