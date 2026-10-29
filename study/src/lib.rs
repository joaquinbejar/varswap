//! Off-chain simulation study of the canonical-print contract (`docs/PLAN.md` 4.1, milestone
//! 5): how far the contractual realized variance (outages compressed into one return) is from
//! the latent variance the full grid would have seen, how often the outage guard unwinds a
//! window, and how much a pusher could move the payoff if prints were not canonical.
//!
//! Everything here is a research harness, never on-chain code: it computes in `f64`, indexes
//! its own vectors, and panics on impossible states, so the crate allows the corresponding
//! lints (reviewed in the PR). Every run is deterministic (splitmix64 seeds, Box–Muller).

#![allow(
    clippy::float_arithmetic,
    clippy::arithmetic_side_effects,
    clippy::indexing_slicing,
    clippy::integer_division,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::needless_range_loop
)]

/// Seconds per year, as the program (`varswap_core::YEAR_SECONDS`).
pub const YEAR_SECONDS: f64 = 31_536_000.0;

/// splitmix64 with Box–Muller normals.
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    /// A generator seeded with `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in (0, 1).
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal.
    pub fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }

    /// Exponential with the given mean.
    pub fn exponential(&mut self, mean: f64) -> f64 {
        -mean * self.uniform().ln()
    }
}

/// The latent return process on the hourly grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Model {
    /// Geometric Brownian motion with annual drift `mu` (log-return drift `mu − σ²/2`).
    Gbm {
        /// Annual drift.
        mu: f64,
    },
    /// Hourly log returns with AR(1) autocorrelation `rho` (stationary variance σ_h²).
    Ar1 {
        /// Lag-one autocorrelation of hourly returns.
        rho: f64,
    },
    /// GBM plus endogenous halts: an hourly move above the trigger halts the feed during the
    /// move (from inside that hour, across the next boundary), and during the halt the latent
    /// price keeps drifting in the same direction (continuation) or back (reversal).
    Halts {
        /// `true` for continuation, `false` for reversal.
        continuation: bool,
    },
}

/// Study parameters (one window).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// Annualized volatility.
    pub sigma: f64,
    /// Grid intervals in the window (hourly).
    pub n: usize,
    /// Exogenous outages: expected starts per hour.
    pub outage_rate_per_hour: f64,
    /// Exogenous outage length, mean of an exponential, hours.
    pub outage_mean_hours: f64,
    /// Halt trigger: |hourly return| above this many hourly sigmas.
    pub halt_trigger_sigmas: f64,
    /// Halt length past the boundary that ends the move, mean of an exponential, hours.
    pub halt_mean_hours: f64,
    /// Latent drift during a halt, hourly sigmas per hour.
    pub halt_drift_sigmas: f64,
}

impl Params {
    /// Hourly volatility.
    #[must_use]
    pub fn sigma_hour(&self) -> f64 {
        self.sigma * (3_600.0 / YEAR_SECONDS).sqrt()
    }
}

/// A Pyth outage: no aggregate between `start` and `end` (hours from `b_0`); the first print
/// after it is at `end` with log price `reopen`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Outage {
    /// Hours from `b_0`.
    pub start: f64,
    /// Hours from `b_0`.
    pub end: f64,
    /// Latent log price at `end`.
    pub reopen: f64,
}

/// One simulated window: latent log prices at every hour (past `n`, so outages over the end
/// reopen on the path) and the outages.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// Log price at hour `k`.
    pub latent: Vec<f64>,
    /// Non-overlapping outages, by start.
    pub outages: Vec<Outage>,
}

/// Extra hours simulated after `b_N`, so an outage spanning the end has a reopening print.
const TAIL_HOURS: usize = 72;

/// Simulates one window.
pub fn simulate(model: Model, p: &Params, rng: &mut Rng) -> Path {
    let sh = p.sigma_hour();
    let hours = p.n + TAIL_HOURS;
    let mut returns = vec![0.0; hours];
    // Exogenous outages first (they do not depend on the path).
    let mut windows: Vec<(f64, f64, f64)> = Vec::new(); // (start, end, drift per hour)
    let mut k = 0.0;
    while k < hours as f64 - 1.0 {
        if rng.uniform() < p.outage_rate_per_hour {
            let start = k + rng.uniform();
            let len = rng.exponential(p.outage_mean_hours).max(1.0 / 60.0);
            windows.push((start, start + len, 0.0));
            k = (start + len).ceil();
        } else {
            k += 1.0;
        }
    }
    let mut prev = 0.0;
    let mut halt_until = f64::NEG_INFINITY;
    for i in 0..hours {
        let t = i as f64;
        let z = rng.normal();
        let mut r = match model {
            Model::Gbm { mu } => (mu - p.sigma * p.sigma / 2.0) * 3_600.0 / YEAR_SECONDS + sh * z,
            Model::Ar1 { rho } => rho * prev + sh * (1.0 - rho * rho).sqrt() * z,
            Model::Halts { .. } => -p.sigma * p.sigma / 2.0 * 3_600.0 / YEAR_SECONDS + sh * z,
        };
        // Drift of a halt covering this hour.
        for w in &windows {
            if w.2 != 0.0 && t + 1.0 > w.0 && t < w.1 {
                let overlap = (t + 1.0).min(w.1) - t.max(w.0);
                r += w.2 * sh * overlap;
            }
        }
        if let Model::Halts { continuation } = model {
            if r.abs() > p.halt_trigger_sigmas * sh && t >= halt_until {
                // The feed fails during the move itself: the halt starts inside this hour,
                // covers the boundary b_{t+1} that ends it, and lasts an exponential time past
                // that boundary, while the latent price drifts with the move (continuation)
                // or back against it (reversal). The move is then inside the compressed return.
                let start = t + rng.uniform();
                let end = t + 1.0 + rng.exponential(p.halt_mean_hours).max(0.25);
                let dir = if continuation {
                    r.signum()
                } else {
                    -r.signum()
                };
                windows.push((start, end, dir * p.halt_drift_sigmas));
                halt_until = end;
            }
        }
        returns[i] = r;
        prev = r;
    }
    let mut latent = vec![0.0; hours + 1];
    for i in 0..hours {
        latent[i + 1] = latent[i] + returns[i];
    }
    // Merge overlapping windows and give each reopening a Brownian-bridge price.
    windows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut outages: Vec<Outage> = Vec::new();
    for (s, e, _) in windows {
        let e = e.min((hours - 1) as f64);
        if s >= e {
            continue;
        }
        if let Some(last) = outages.last_mut() {
            if s <= last.end {
                last.end = last.end.max(e);
                continue;
            }
        }
        outages.push(Outage {
            start: s,
            end: e,
            reopen: 0.0,
        });
    }
    for o in &mut outages {
        let j = o.end.floor() as usize;
        let f = o.end - j as f64;
        o.reopen = latent[j]
            + f * (latent[j + 1] - latent[j])
            + sh * (f * (1.0 - f)).max(0.0).sqrt() * rng.normal();
    }
    Path { latent, outages }
}

/// Latent realized variance of the full grid, annualized.
#[must_use]
pub fn latent_variance(path: &Path, n: usize) -> f64 {
    let s: f64 = (1..=n)
        .map(|k| (path.latent[k] - path.latent[k - 1]).powi(2))
        .sum();
    s * YEAR_SECONDS / (n as f64 * 3_600.0)
}

/// The contract's realized variance with outage guard `tolerance_hours`, annualized, or
/// `None` when a boundary has no canonical print within the guard (the window degrades and
/// unwinds). The canonical print of `b_k` is the first aggregate at or after it: the latent
/// price at `b_k` outside outages, the reopening print inside one.
#[must_use]
pub fn contract_variance(path: &Path, n: usize, tolerance_hours: f64) -> Option<f64> {
    let mut prices = Vec::with_capacity(n + 1);
    for k in 0..=n {
        let b = k as f64;
        let inside = path.outages.iter().find(|o| o.start <= b && b < o.end);
        match inside {
            Some(o) => {
                if o.end - b > tolerance_hours {
                    return None;
                }
                prices.push(o.reopen);
            }
            None => prices.push(path.latent[k]),
        }
    }
    let s: f64 = prices.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
    Some(s * YEAR_SECONDS / (n as f64 * 3_600.0))
}

/// Summary statistics of a sample.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Summary {
    /// Number of values.
    pub count: usize,
    /// Mean.
    pub mean: f64,
    /// Standard deviation.
    pub std: f64,
    /// 5th percentile.
    pub p5: f64,
    /// Median.
    pub p50: f64,
    /// 95th percentile.
    pub p95: f64,
    /// 99th percentile.
    pub p99: f64,
    /// Largest absolute value.
    pub max_abs: f64,
}

/// Summary of `xs` (sorted copy, nearest-rank percentiles).
#[must_use]
pub fn summarize(xs: &[f64]) -> Summary {
    if xs.is_empty() {
        return Summary::default();
    }
    let mut v = xs.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    let q = |p: f64| v[((p * n as f64).ceil() as usize).clamp(1, n) - 1];
    Summary {
        count: n,
        mean,
        std: var.sqrt(),
        p5: q(0.05),
        p50: q(0.50),
        p95: q(0.95),
        p99: q(0.99),
        max_abs: v.iter().fold(0.0, |m, x| m.max(x.abs())),
    }
}

/// Outcome of one (model, tolerance) cell.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    /// Fraction of windows that unwind.
    pub unwind: f64,
    /// Contract minus latent realized variance, over the windows that settle (annualized
    /// variance units).
    pub error: Summary,
    /// Mean latent realized variance over all windows.
    pub mean_latent: f64,
    /// Fraction of windows with at least one outage boundary (compressed or unwound).
    pub touched: f64,
}

/// Runs `paths` windows of `model` and evaluates every tolerance on the same paths.
#[must_use]
pub fn run(model: Model, p: &Params, paths: usize, seed: u64, tolerances: &[f64]) -> Vec<Cell> {
    let mut rng = Rng::new(seed);
    let sims: Vec<Path> = (0..paths).map(|_| simulate(model, p, &mut rng)).collect();
    let mean_latent = sims.iter().map(|s| latent_variance(s, p.n)).sum::<f64>() / paths as f64;
    let touched = sims
        .iter()
        .filter(|s| {
            s.outages
                .iter()
                .any(|o| (0..=p.n).any(|k| o.start <= k as f64 && (k as f64) < o.end))
        })
        .count() as f64
        / paths as f64;
    tolerances
        .iter()
        .map(|&tol| {
            let mut errors = Vec::new();
            let mut unwinds = 0usize;
            for s in &sims {
                match contract_variance(s, p.n, tol) {
                    Some(v) => errors.push(v - latent_variance(s, p.n)),
                    None => unwinds += 1,
                }
            }
            Cell {
                unwind: unwinds as f64 / paths as f64,
                error: summarize(&errors),
                mean_latent,
                touched,
            }
        })
        .collect()
}

/// What a pusher could reach if any of `candidates` prints within `window_seconds` after
/// each boundary were acceptable (first-write-wins with a tolerance), against the canonical
/// print, on one coherent GBM path without outages.
///
/// The candidates are points of the same fine-grained path: the path walks from `b_k` through
/// the candidates (one per `window_seconds / (candidates − 1)` s) and on to `b_{k+1}`, so the
/// hourly prints are a subsequence and shifting every observation by the same time leaves the
/// expected variance unchanged. The best case for the long (max) and the short (min) is exact
/// by dynamic programming over the candidate per boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PusherBias {
    /// `max − canonical`, annualized variance units.
    pub long_gain: f64,
    /// `canonical − min`, annualized variance units.
    pub short_gain: f64,
    /// Control: always the last candidate (a fixed shift of `window_seconds`) minus
    /// canonical; zero in expectation.
    pub fixed_shift: f64,
}

/// See [`PusherBias`].
pub fn pusher_bias(
    p: &Params,
    rng: &mut Rng,
    window_seconds: f64,
    candidates: usize,
) -> PusherBias {
    let sigma_s = p.sigma / YEAR_SECONDS.sqrt();
    let drift_s = -p.sigma * p.sigma / 2.0 / YEAR_SECONDS;
    let step = window_seconds / (candidates - 1).max(1) as f64;
    let rest = 3_600.0 - step * (candidates - 1) as f64;
    let mut cand: Vec<Vec<f64>> = Vec::with_capacity(p.n + 1);
    let mut x = 0.0;
    for k in 0..=p.n {
        let mut c = Vec::with_capacity(candidates);
        c.push(x);
        for _ in 1..candidates {
            x += drift_s * step + sigma_s * step.sqrt() * rng.normal();
            c.push(x);
        }
        cand.push(c);
        if k < p.n {
            // On to the next boundary along the same path.
            x += drift_s * rest + sigma_s * rest.sqrt() * rng.normal();
        }
    }
    let path_var = |pick: &dyn Fn(&[f64]) -> f64| -> f64 {
        cand.windows(2)
            .map(|w| (pick(&w[1]) - pick(&w[0])).powi(2))
            .sum()
    };
    let canonical = path_var(&|c: &[f64]| c[0]);
    let shifted = path_var(&|c: &[f64]| c[candidates - 1]);
    let best = |maximize: bool| -> f64 {
        let mut score = vec![0.0; candidates];
        for k in 1..=p.n {
            let mut next = vec![0.0; candidates];
            for (j, nj) in next.iter_mut().enumerate() {
                let vals =
                    (0..candidates).map(|i| score[i] + (cand[k][j] - cand[k - 1][i]).powi(2));
                *nj = if maximize {
                    vals.fold(f64::NEG_INFINITY, f64::max)
                } else {
                    vals.fold(f64::INFINITY, f64::min)
                };
            }
            score = next;
        }
        if maximize {
            score.into_iter().fold(f64::NEG_INFINITY, f64::max)
        } else {
            score.into_iter().fold(f64::INFINITY, f64::min)
        }
    };
    let scale = YEAR_SECONDS / (p.n as f64 * 3_600.0);
    PusherBias {
        long_gain: (best(true) - canonical) * scale,
        short_gain: (canonical - best(false)) * scale,
        fixed_shift: (shifted - canonical) * scale,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_macros)]

    use super::*;

    fn params() -> Params {
        Params {
            sigma: 0.8,
            n: 720,
            outage_rate_per_hour: 0.0,
            outage_mean_hours: 1.0,
            halt_trigger_sigmas: 3.0,
            halt_mean_hours: 2.0,
            halt_drift_sigmas: 1.0,
        }
    }

    #[test]
    fn test_no_outage_contract_equals_latent() {
        let mut rng = Rng::new(1);
        let p = params();
        let s = simulate(Model::Gbm { mu: 0.0 }, &p, &mut rng);
        assert!(s.outages.is_empty());
        let c = contract_variance(&s, p.n, 6.0).unwrap();
        assert!((c - latent_variance(&s, p.n)).abs() < 1e-12);
    }

    #[test]
    fn test_latent_variance_estimates_sigma_squared() {
        let mut rng = Rng::new(2);
        let p = params();
        let mean = (0..400)
            .map(|_| latent_variance(&simulate(Model::Gbm { mu: 0.0 }, &p, &mut rng), p.n))
            .sum::<f64>()
            / 400.0;
        assert!((mean - 0.64).abs() < 0.02, "{mean}");
        let ar = (0..400)
            .map(|_| latent_variance(&simulate(Model::Ar1 { rho: 0.3 }, &p, &mut rng), p.n))
            .sum::<f64>()
            / 400.0;
        assert!((ar - 0.64).abs() < 0.03, "{ar}");
    }

    #[test]
    fn test_outage_beyond_guard_unwinds() {
        let p = params();
        let mut s = simulate(Model::Gbm { mu: 0.0 }, &p, &mut Rng::new(3));
        s.outages = vec![Outage {
            start: 10.5,
            end: 17.5,
            reopen: s.latent[17],
        }];
        // b_11 waits 6.5 h for its print.
        assert!(contract_variance(&s, p.n, 6.0).is_none());
        assert!(contract_variance(&s, p.n, 12.0).is_some());
    }

    #[test]
    fn test_reversal_compression_lowers_variance() {
        // A +5% move into b_11 and its reversal into b_12, hidden by an outage over b_11:
        // the two large returns collapse into one near-zero return.
        let p = params();
        let mut s = simulate(Model::Gbm { mu: 0.0 }, &p, &mut Rng::new(4));
        let shift = s.latent[10] - s.latent[12];
        for x in s.latent.iter_mut().skip(12) {
            *x += shift;
        }
        s.latent[11] = s.latent[10] + 0.05;
        let base = contract_variance(&s, p.n, 6.0).unwrap();
        s.outages = vec![Outage {
            start: 10.5,
            end: 12.0,
            reopen: s.latent[12],
        }];
        let compressed = contract_variance(&s, p.n, 6.0).unwrap();
        // Two 5% returns (0.005 in squared log returns) disappear, annualized by 12.17.
        assert!(compressed < base - 0.004 * 31_536_000.0 / (720.0 * 3_600.0));
    }

    #[test]
    fn test_pusher_bias_signs_and_zero_window() {
        let p = params();
        let b = pusher_bias(&p, &mut Rng::new(5), 60.0, 7);
        assert!(b.long_gain >= 0.0 && b.short_gain >= 0.0);
        let z = pusher_bias(&p, &mut Rng::new(5), 0.0, 1);
        assert!(z.long_gain.abs() < 1e-12 && z.short_gain.abs() < 1e-12);
        assert!(z.fixed_shift.abs() < 1e-12);
    }

    #[test]
    fn test_pusher_fixed_shift_control_is_unbiased() {
        // Shifting every observation by the same 300 s on one coherent path leaves the
        // expected variance unchanged (codex review of the incoherent first model, which
        // added 2 σ² w / 3600 per path instead).
        let p = params();
        let mut rng = Rng::new(6);
        let n = 600;
        let shifts: Vec<f64> = (0..n)
            .map(|_| pusher_bias(&p, &mut rng, 300.0, 7).fixed_shift)
            .collect();
        let s = summarize(&shifts);
        let se = s.std / (n as f64).sqrt();
        assert!(s.mean.abs() < 4.0 * se, "mean {} vs se {}", s.mean, se);
        // The incoherent model's bias, 2 σ² w / 3600 = 0.1067, is far outside that.
        assert!(4.0 * se < 0.01);
    }

    #[test]
    fn test_summary_percentiles() {
        let xs: Vec<f64> = (1..=100).map(f64::from).collect();
        let s = summarize(&xs);
        assert_eq!((s.p5, s.p50, s.p95, s.p99), (5.0, 50.0, 95.0, 99.0));
        assert_eq!(s.max_abs, 100.0);
    }
}
