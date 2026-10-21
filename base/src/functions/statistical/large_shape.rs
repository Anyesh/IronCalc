//! Beta quantiles for large shape parameters. statrs' incomplete beta takes
//! ln B(a, b) as a difference of log-gammas, whose rounding grows with the
//! shapes, and stops its continued fraction after 140 terms. Here the density
//! takes those differences from Stirling's series and the continued fraction
//! runs until it converges, except near the mean with both shapes large, where
//! it would take too many terms and a Cornish-Fisher series is used instead.

use super::large_df::{fisher_inverse_cdf_both_large, normal_quantile};
use super::log_tail::{newton_log_tail, stirling_remainder, u_minus_ln_1p, LN_SQRT_2PI};

/// Far more terms than the continued fraction takes where it is used: at most
/// about 350 across the sweep, near the mean with the smaller shape just below
/// where the Cornish-Fisher series takes over.
const MAX_FRACTION_TERMS: usize = 10_000;

/// x with I_x(a, b) = p for 0 < p < 1.
pub(crate) fn beta_inverse_large_shape(a: f64, b: f64, p: f64) -> Option<f64> {
    if !(a > 0.0 && b > 0.0 && p > 0.0 && p < 1.0) {
        return None;
    }
    if let Some(f) = fisher_inverse_cdf_both_large(2.0 * a, 2.0 * b, p) {
        // X / (1 - X) is (a / b) F for F with 2a and 2b degrees of freedom.
        return Some(1.0 / (1.0 + b / a / f));
    }
    let upper = p > 0.5;
    let target = if upper { (1.0 - p).ln() } else { p.ln() };
    // Newton runs on t = ln(x / (1 - x)), which is near normal with mean
    // ln(a / b) and variance 1/a + 1/b.
    let start = (a / b).ln() + normal_quantile(p) * (1.0 / a + 1.0 / b).sqrt();
    let (t, step) = newton_log_tail(start, target, upper, |t| beta_log_tail(a, b, t, upper))?;
    let x = logistic(t);
    Some(if x < 0.5 {
        // The last step applied to x itself: t near -700 resolves x only to
        // about 1e-13, while scaling x keeps all of its digits.
        x * (-(1.0 - x) * step).exp()
    } else {
        logistic(t - step)
    })
}

/// 1 / (1 + e^-t), with relative accuracy for t far below zero.
fn logistic(t: f64) -> f64 {
    if t < 0.0 {
        let e = t.exp();
        e / (1.0 + e)
    } else {
        1.0 / (1.0 + (-t).exp())
    }
}

/// ln I_x(a, b), or ln(1 - I_x(a, b)) when `upper`, at x = 1 / (1 + e^-t),
/// with the magnitude of its derivative in t.
fn beta_log_tail(a: f64, b: f64, t: f64, upper: bool) -> Option<(f64, f64)> {
    let x = logistic(t);
    let y = logistic(-t);
    // (a + b)x - a. Both x and y carry full relative precision, so this is
    // as accurate as t, where taking 1 - x for y would not be once x is small.
    let n = b.mul_add(x, -(a * y));
    let log_density = beta_log_density(a, b, t, n);
    // The continued fraction gives the tail on x's side of the mean.
    let (fraction, fraction_is_upper) = if n <= 0.0 {
        (beta_fraction(a, b, x, y, -n)?, false)
    } else {
        (beta_fraction(b, a, y, x, n)?, true)
    };
    let log_near_tail = log_density + fraction.ln();
    let log_tail = if fraction_is_upper == upper {
        log_near_tail
    } else {
        (-log_near_tail.exp()).ln_1p()
    };
    Some((log_tail, (log_density - log_tail).exp()))
}

/// ln(x^a y^b / B(a, b)) at x = 1 / (1 + e^-t), y = 1 - x, given
/// n = (a + b)x - a: the density of t. With x0 = a / (a + b), u = x / x0 - 1
/// and v = y / (1 - x0) - 1, Stirling's series turns it into
/// -(a φ(u) + b φ(v)) + ln √(ab / (2π(a + b))) less the series' remainders,
/// where φ(u) = u - ln(1 + u); nothing in that cancels as the shapes grow.
fn beta_log_density(a: f64, b: f64, t: f64, n: f64) -> f64 {
    let s = a + b;
    let l = (-t.abs()).exp().ln_1p();
    let (ln_x, ln_y) = if t < 0.0 { (t - l, -l) } else { (-l, -t - l) };
    let u = n / a;
    let v = -n / b;
    // Near u = -1, 1 + u = x / x0 is better taken from ln x.
    let ln_1p_u = if u < -0.5 {
        ln_x + s.ln() - a.ln()
    } else {
        u.ln_1p()
    };
    let ln_1p_v = if v < -0.5 {
        ln_y + s.ln() - b.ln()
    } else {
        v.ln_1p()
    };
    let exponent = -(a * u_minus_ln_1p(u, ln_1p_u) + b * u_minus_ln_1p(v, ln_1p_v));
    exponent + 0.5 * (a.ln() + b.ln() - s.ln())
        - LN_SQRT_2PI
        - (stirling_remainder(a) + stirling_remainder(b) - stirling_remainder(s))
}

/// I_x(a, b) divided by x^a y^b / B(a, b), for λ = a - (a + b)x >= 0 and
/// y = 1 - x: the continued fraction of DiDonato and Morris (ACM TOMS 708,
/// BFRAC), whose terms take x, y and λ as given, so none of them cancels near
/// the mean.
fn beta_fraction(a: f64, b: f64, x: f64, y: f64, lambda: f64) -> Option<f64> {
    let c = lambda + 1.0;
    let c0 = b / a;
    let c1 = 1.0 / a + 1.0;
    let yp1 = y + 1.0;
    let mut n = 0.0;
    let mut p = 1.0;
    let mut s = a + 1.0;
    let mut an = 0.0;
    let mut bn = 1.0;
    let mut anp1 = 1.0;
    let mut bnp1 = c / c1;
    let mut r = c1 / c;
    for _ in 0..MAX_FRACTION_TERMS {
        n += 1.0;
        let t = n / a;
        let w = n * (b - n) * x;
        let e = a / s;
        let alpha = p * (p + c0) * e * e * (w * x);
        let e = (t + 1.0) / (c1 + t + t);
        let beta = n + w / s + e * (c + n * yp1);
        p = t + 1.0;
        s += 2.0;
        (an, anp1) = (anp1, alpha * an + beta * anp1);
        (bn, bnp1) = (bnp1, alpha * bn + beta * bnp1);
        let previous = r;
        r = anp1 / bnp1;
        if !r.is_finite() {
            return None;
        }
        if (r - previous).abs() <= f64::EPSILON * r {
            return Some(r);
        }
        an /= bnp1;
        bn /= bnp1;
        anp1 = r;
        bnp1 = 1.0;
    }
    None
}
