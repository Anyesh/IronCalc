//! T and F quantiles for degrees of freedom large enough that the inverse
//! incomplete beta loses accuracy or fails: statrs' `beta_reg` takes the
//! difference of log-gammas near 1e11 for df = 1e10 and stops its continued
//! fraction after 140 terms, so near the limit it is off in the fifth digit.

use statrs::function::erf::{erf_inv, erfc_inv};
use statrs::function::gamma::{checked_gamma_lr, checked_gamma_ur, ln_gamma};

/// Hill's expansion for Student's t is used from this df, and only while z²
/// is at most `T_MAX_Z2_PER_DF` of the df; inside that region it is within
/// 1e-13 of the mpmath quantile, while the beta route is already 3e-6 off at
/// df = 1e5.
const T_MIN_DF: f64 = 1e4;
const T_MAX_Z2_PER_DF: f64 = 0.01;

/// For the central quantiles near the median Hill's expansion is used from
/// this df: its first omitted term, about z / (20 df⁵) there, is then below
/// the relative error 2^-52 lnΓ(df/2) that statrs' ln_beta costs the beta
/// route.
const T_CENTRAL_MIN_DF: f64 = 3e2;

/// F quantiles use an expansion only once the larger df reaches this; below
/// it the beta route stays within about 1e-11.
const F_MIN_LARGE_DF: f64 = 1e5;

/// The Cornish-Fisher series for ln F needs both df at least
/// `F_CORNISH_FISHER_MIN_DF` and z² at most `F_CORNISH_FISHER_MAX_Z2_PER_DF`
/// of the smaller df; from `F_CORNISH_FISHER_PREFERRED_DF` on it is within
/// 1e-12 and preferred to the perturbation.
const F_CORNISH_FISHER_MIN_DF: f64 = 3e3;
const F_CORNISH_FISHER_MAX_Z2_PER_DF: f64 = 1e-3;
const F_CORNISH_FISHER_PREFERRED_DF: f64 = 1e5;

/// The perturbation needs the larger df at least this multiple of the
/// smaller; its O(ratio³) error is then within about 2e-11.
const F_PERTURBATION_MIN_RATIO: f64 = 1e3;

/// Standard normal quantile of the lower tail probability `x` (x <= 0.5),
/// returned as a magnitude.
fn normal_tail_quantile(x: f64) -> f64 {
    std::f64::consts::SQRT_2 * erfc_inv(2.0 * x)
}

/// Student's t quantile from the normal quantile by Hill's expansion
/// (Abramowitz and Stegun 26.7.5), or `None` outside the region where it is
/// accurate.
pub(crate) fn students_t_inverse_cdf_large_df(df: f64, p: f64) -> Option<f64> {
    if df < T_MIN_DF {
        return None;
    }
    let tail = if p < 0.5 { p } else { 1.0 - p };
    let t = hill_expansion(df, normal_tail_quantile(tail))?;
    Some(if p < 0.5 { -t } else { t })
}

/// The t with P(|T| <= t) = `central` by Hill's expansion, or `None` outside
/// the region where it is accurate.
pub(crate) fn students_t_central_quantile_large_df(df: f64, central: f64) -> Option<f64> {
    if df < T_CENTRAL_MIN_DF {
        return None;
    }
    hill_expansion(df, std::f64::consts::SQRT_2 * erf_inv(central))
}

fn hill_expansion(df: f64, z: f64) -> Option<f64> {
    let z2 = z * z;
    if z2.is_nan() || z2 > T_MAX_Z2_PER_DF * df {
        return None;
    }
    let g1 = (z2 + 1.0) * z / 4.0;
    let g2 = ((5.0 * z2 + 16.0) * z2 + 3.0) * z / 96.0;
    let g3 = (((3.0 * z2 + 19.0) * z2 + 17.0) * z2 - 15.0) * z / 384.0;
    let g4 = ((((79.0 * z2 + 776.0) * z2 + 1482.0) * z2 - 1920.0) * z2 - 945.0) * z / 92160.0;
    Some(z + (g1 + (g2 + (g3 + g4 / df) / df) / df) / df)
}

/// F quantile for a large df, or `None` when neither expansion is accurate
/// for these degrees of freedom and `p`.
pub(crate) fn fisher_inverse_cdf_large_df(df1: f64, df2: f64, p: f64) -> Option<f64> {
    let small = df1.min(df2);
    let large = df1.max(df2);
    if large < F_MIN_LARGE_DF {
        return None;
    }
    if let Some(f) = fisher_inverse_cdf_both_large(df1, df2, p) {
        return Some(f);
    }
    if large >= F_PERTURBATION_MIN_RATIO * small {
        return fisher_inverse_cdf_perturbed(df1, df2, p);
    }
    if small >= F_CORNISH_FISHER_MIN_DF {
        return fisher_inverse_cdf_cornish_fisher(df1, df2, p);
    }
    None
}

/// F quantile by the Cornish-Fisher series once both df reach
/// `F_CORNISH_FISHER_PREFERRED_DF`, or `None` when it does not apply.
pub(crate) fn fisher_inverse_cdf_both_large(df1: f64, df2: f64, p: f64) -> Option<f64> {
    if df1.min(df2) < F_CORNISH_FISHER_PREFERRED_DF {
        return None;
    }
    fisher_inverse_cdf_cornish_fisher(df1, df2, p)
}

/// Cumulants κ1..κ6 of ln(X / df) for X chi-square with df = 2a, which are
/// ψ(a) - ln a and the polygammas ψ⁽ʳ⁻¹⁾(a). Uses the asymptotic series in
/// 1/a, accurate to rounding for a >= 1000.
fn log_chi_square_cumulants(a: f64) -> [f64; 6] {
    // Bernoulli numbers B2, B4, B6, B8.
    const B: [f64; 4] = [1.0 / 6.0, -1.0 / 30.0, 1.0 / 42.0, -1.0 / 30.0];
    let r = 1.0 / a;
    let r2 = r * r;
    // ψ(a) - ln a = -1/(2a) - Σ B2k / (2k a^2k)
    let mut k1 = -0.5 * r;
    let mut power = r2;
    for (k, b) in B.iter().enumerate() {
        k1 -= b / (2.0 * (k as f64 + 1.0)) * power;
        power *= r2;
    }
    let mut cumulants = [k1, 0.0, 0.0, 0.0, 0.0, 0.0];
    // ψ⁽ⁿ⁾(a) = (-1)ⁿ⁺¹ [(n-1)!/aⁿ + n!/(2aⁿ⁺¹) + Σ B2k (2k+n-1)!/((2k)! a^(2k+n))]
    let mut factorial_n_minus_1 = 1.0;
    for n in 1..=5 {
        let nf = n as f64;
        let mut sum =
            factorial_n_minus_1 * r.powi(n) + factorial_n_minus_1 * nf / 2.0 * r.powi(n + 1);
        // (2k+n-1)!/(2k)! as a running product.
        let mut ratio = factorial_n_minus_1;
        let mut power = r.powi(n + 2);
        for (k, b) in B.iter().enumerate() {
            let two_k = 2.0 * (k as f64 + 1.0);
            ratio *= (two_k + nf - 2.0) * (two_k + nf - 1.0) / ((two_k - 1.0) * two_k);
            sum += b * ratio * power;
            power *= r2;
        }
        cumulants[n as usize] = if n % 2 == 1 { sum } else { -sum };
        factorial_n_minus_1 *= nf;
    }
    cumulants
}

/// Signed standard normal quantile of `p`.
pub(crate) fn normal_quantile(p: f64) -> f64 {
    if p < 0.5 {
        -normal_tail_quantile(p)
    } else {
        normal_tail_quantile(1.0 - p)
    }
}

/// ln F = L1 - L2 with Li = ln(Xi / dfi) independent; with both df large it
/// is near normal and its cumulants are known exactly, so the
/// Cornish-Fisher series (Abramowitz and Stegun 26.2.49, through the n⁻²
/// terms) gives its quantile.
fn fisher_inverse_cdf_cornish_fisher(df1: f64, df2: f64, p: f64) -> Option<f64> {
    let z = normal_quantile(p);
    if z.is_nan() || z * z > F_CORNISH_FISHER_MAX_Z2_PER_DF * df1.min(df2) {
        return None;
    }
    let c1 = log_chi_square_cumulants(df1 / 2.0);
    let c2 = log_chi_square_cumulants(df2 / 2.0);
    // κ1, κ3 and κ5 of -L2 are those of L2 with the sign changed.
    let k: [f64; 6] = std::array::from_fn(|r| {
        if r % 2 == 0 {
            c1[r] - c2[r]
        } else {
            c1[r] + c2[r]
        }
    });
    let s = k[1].sqrt();
    let g1 = k[2] / (s * k[1]);
    let g2 = k[3] / (k[1] * k[1]);
    let g3 = k[4] / (s * k[1] * k[1]);
    let g4 = k[5] / (k[1] * k[1] * k[1]);
    let z2 = z * z;
    let he1 = z;
    let he2 = z2 - 1.0;
    let he3 = (z2 - 3.0) * z;
    let he4 = (z2 - 6.0) * z2 + 3.0;
    let he5 = ((z2 - 10.0) * z2 + 15.0) * z;
    let w = z
        + g1 * he2 / 6.0
        + (g2 * he3 / 24.0 - g1 * g1 * (2.0 * he3 + he1) / 36.0)
        + (g3 * he4 / 120.0 - g1 * g2 * (he4 + he2) / 24.0
            + g1 * g1 * g1 * (12.0 * he4 + 19.0 * he2) / 324.0)
        + (g4 * he5 / 720.0
            - g2 * g2 * (3.0 * he5 + 6.0 * he3 + 2.0 * he1) / 384.0
            - g1 * g3 * (2.0 * he5 + 3.0 * he3) / 180.0
            + g1 * g1 * g2 * (14.0 * he5 + 37.0 * he3 + 8.0 * he1) / 288.0
            - g1 * g1 * g1 * g1 * (252.0 * he5 + 832.0 * he3 + 227.0 * he1) / 7776.0);
    let f = (k[0] + s * w).exp();
    f.is_finite().then_some(f)
}

/// ln F = Y + E, where Y is the log chi-square term of the smaller df (taken
/// exactly, through the incomplete gamma) and E the term of the much larger
/// one, whose moments are O(1/df). Expanding P(Y + E <= u) = p around Y's
/// own quantile u0 gives u = u0 + δ with δ correct to O(df⁻²).
fn fisher_inverse_cdf_perturbed(df1: f64, df2: f64, p: f64) -> Option<f64> {
    let upper = p > 0.5;
    let tail = if upper { 1.0 - p } else { p };
    // Y = L1 when df2 is large; Y = -L2 when df1 is. With c the chi-square
    // value at Y's quantile, m = d/du ln(density of Y) and dc/du = sign * c.
    let (u0, c, m, sign, noise) = if df2 >= df1 {
        let a = df1 / 2.0;
        // P(Y <= u) = P(a, c/2) with c = df1 * e^u.
        let x = inverse_gamma(a, tail, upper)?;
        let c = 2.0 * x;
        let k = log_chi_square_cumulants(df2 / 2.0);
        ((c / df1).ln(), c, a - x, 1.0, [-k[0], k[1], -k[2], k[3]])
    } else {
        let b = df2 / 2.0;
        // P(Y <= u) = Q(b, c/2) with c = df2 * e^-u.
        let x = inverse_gamma(b, tail, !upper)?;
        let c = 2.0 * x;
        let k = log_chi_square_cumulants(df1 / 2.0);
        (-(c / df2).ln(), c, x - b, -1.0, [k[0], k[1], k[2], k[3]])
    };
    let [e1, e2, e3, e4] = noise;
    let n1 = e1;
    let n2 = e2 + e1 * e1;
    let n3 = e3 + 3.0 * e2 * e1 + e1 * e1 * e1;
    let n4 = e4 + 4.0 * e3 * e1 + 3.0 * e2 * e2 + 6.0 * e2 * e1 * e1 + e1 * e1 * e1 * e1;
    // Pk = (k-th derivative of Y's density) / density.
    let p1 = m;
    let p2 = m * m - c / 2.0;
    let p3 = m * m * m - 1.5 * m * c - sign * c / 2.0;
    let s1 = -n1 + n2 * p1 / 2.0;
    let s2 = -n3 * p2 / 6.0 + n4 * p3 / 24.0;
    let s1_slope = -n2 * c / 4.0;
    let delta = -s1 - s2 + s1 * s1_slope + p1 * s1 * s1 / 2.0;
    let f = (u0 + delta).exp();
    f.is_finite().then_some(f)
}

/// x with P(a, x) = prob, or Q(a, x) = prob when `upper`. Newton steps on
/// the log of the tail in ln x, kept inside a bracket that bisection
/// tightens whenever a step leaves it.
fn inverse_gamma(a: f64, prob: f64, upper: bool) -> Option<f64> {
    const MAX_ITERATIONS: usize = 200;
    if !(prob > 0.0 && prob < 1.0 && a > 0.0) {
        return None;
    }
    let target = prob.ln();
    let ln_gamma_a = ln_gamma(a);
    let h = 1.0 / (9.0 * a);
    let z = if upper {
        -normal_quantile(prob)
    } else {
        normal_quantile(prob)
    };
    let wilson_hilferty = a * (1.0 - h + z * h.sqrt()).powi(3);
    // ln x, starting from Wilson-Hilferty, or from P(a, x) ~ xᵃ/Γ(a+1) deep
    // in the lower tail where Wilson-Hilferty goes negative.
    let mut v = if wilson_hilferty > 0.0 {
        wilson_hilferty.ln()
    } else {
        (target + ln_gamma_a + a.ln()) / a
    };
    let mut below = f64::NEG_INFINITY;
    let mut above = f64::INFINITY;
    for _ in 0..MAX_ITERATIONS {
        let x = v.exp();
        let log_tail = if upper {
            checked_gamma_ur(a, x).ok()?.ln()
        } else {
            log_gamma_lr(a, v)?
        };
        if !log_tail.is_finite() {
            return None;
        }
        let residual = log_tail - target;
        if residual == 0.0 {
            return Some(x);
        }
        if (residual > 0.0) != upper {
            above = v;
        } else {
            below = v;
        }
        // d ln(tail) / d ln(x) = ± x f(x) / tail, with x f(x) = xᵃ e⁻ˣ / Γ(a).
        let slope = (a * v - x - ln_gamma_a - log_tail).exp();
        let mut next = v - residual / if upper { -slope } else { slope };
        if !(next > below && next < above) {
            next = match (below.is_finite(), above.is_finite()) {
                (true, true) => 0.5 * (below + above),
                (true, false) => below + 1.0,
                (false, true) => above - 1.0,
                (false, false) => return None,
            };
        }
        if (next - v).abs() <= 4.0 * f64::EPSILON * v.abs().max(1.0) {
            return Some(next.exp());
        }
        v = next;
    }
    None
}

/// ln P(a, x) for x = e^v. statrs returns P = 0 for any x below about 1e-15,
/// so below 1 this sums the series xᵃ e⁻ˣ / Γ(a+1) Σ xⁿ / ((a+1)...(a+n)),
/// in logs so that x may underflow.
fn log_gamma_lr(a: f64, v: f64) -> Option<f64> {
    let x = v.exp();
    if x >= 1.0 {
        return Some(checked_gamma_lr(a, x).ok()?.ln());
    }
    let mut term = 1.0;
    let mut sum = 1.0;
    let mut n = 0.0;
    while term > f64::EPSILON * sum {
        n += 1.0;
        term *= x / (a + n);
        sum += term;
    }
    Some(a * v - x - ln_gamma(a + 1.0) + sum.ln())
}
