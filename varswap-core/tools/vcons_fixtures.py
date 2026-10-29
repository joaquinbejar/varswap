#!/usr/bin/env python3
"""High-precision reference for the conservative realized variance (stdlib only).

Writes tests/fixtures/vcons.txt: one case per line,
  interval cap_raw notional eps v_exact_floor payout_exact_floor p_0 p_1 ... p_N
with every value an integer. Exponent 0, strike 1.0 (collateral exact), notional the largest
the limit allows, floor((2^64 - 1) / eps). V_exact is the annualized sum of squared natural
log returns in Q64.64 raw units, computed with decimal at 100 digits; its floor and the floor
of the exact long payout floor(notional x min(V_exact, cap) / 2^64) let the Rust test check
the direction exactly: for an integer q and any real x, q <= x iff q <= floor(x), which also
covers the zero-variance paths, whose V_exact is the integer 0. eps replicates LnKernel::realized_var_error_bound with
delta_e = 0; the test asserts the two agree.

Usage: python3 tools/vcons_fixtures.py > tests/fixtures/vcons.txt
"""
import math
import random
from decimal import Decimal, getcontext, ROUND_FLOOR

getcontext().prec = 100
YEAR = 31_536_000
TWO64 = 1 << 64
I64_MAX = (1 << 63) - 1


def eps_bound(interval, cap_raw, max_delta_exp=0):
    y = -(-YEAR // interval)
    dr = 4 + max_delta_exp
    root = math.isqrt(cap_raw * y) + 1
    main = ((root * dr * 2) >> 32) + 1
    return main + 3 * y + 2


def v_exact(prices, interval):
    s = Decimal(0)
    for a, b in zip(prices, prices[1:]):
        r = (Decimal(b) / Decimal(a)).ln()
        s += r * r
    span = Decimal(interval * (len(prices) - 1))
    return s * Decimal(YEAR) / span * Decimal(TWO64)


def floor(x):
    return int(x.to_integral_value(rounding=ROUND_FLOOR))


def case(prices, interval, cap_raw):
    eps = eps_bound(interval, cap_raw)
    notional = (TWO64 - 1) // eps
    v = v_exact(prices, interval)
    capped = min(v, Decimal(cap_raw))
    payout = floor(Decimal(notional) * capped / Decimal(TWO64))
    return [interval, cap_raw, notional, eps, floor(v), payout] + prices


def gbm(rng, n, interval, sigma, s0):
    dt = interval / YEAR
    p, out = s0, []
    for _ in range(n + 1):
        out.append(max(1, min(I64_MAX, int(round(p)))))
        p *= math.exp(-sigma * sigma / 2 * dt + sigma * math.sqrt(dt) * rng.gauss(0, 1))
    return out


def main():
    cap = 25 << 64
    cases = [
        # Codex's counterexamples (coordinator #36437, #36470).
        case([2_501_942_101_381_778_875, 5_161_686_394_897_806_096], YEAR, cap),
        case([2522, 7_638_331_619_254_237_224], YEAR, cap),
        case([1_726_933_792_193_200_381, 1_726_933_815_967_915_399], YEAR, cap),
    ]
    rng = random.Random(0x7A25_0A11_C0DE_0003)
    for _ in range(600):
        interval = rng.choice([1, 60, 900, 3_600, 86_400, YEAR])
        n = rng.randint(1, 40)
        sigma = rng.choice([0.05, 0.3, 0.8, 2.0, 5.0, 12.0])
        s0 = rng.choice([1.5e10, 4.0e3, 6.2e6, 2.0e5, 1.0e18, 2.5e18])
        cases.append(case(gbm(rng, n, interval, sigma, s0), interval, cap))
    for c in cases:
        print(" ".join(str(x) for x in c))


if __name__ == "__main__":
    main()
