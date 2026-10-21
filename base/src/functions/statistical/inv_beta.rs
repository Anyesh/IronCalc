use statrs::function::beta::{checked_beta_reg, ln_beta};

use super::large_df::{fisher_inverse_cdf_large_df, students_t_inverse_cdf_large_df};

const MAX_ITERATIONS: usize = 1000;

/// Inverse of the regularized incomplete beta function I_x(a, b), or `None`
/// when it does not converge.
///
/// Same arithmetic as statrs' `inv_beta_reg` (AS 109), so inputs whose root
/// AS 109 resolves give identical results. statrs loops forever once the
/// Newton step overflows (huge shape parameters, or tails where the density
/// underflows); that returns `None` here. Where `x * x` underflows statrs'
/// starting point is NaN, and where AS 109's absolute stopping test leaves the
/// root unresolved its answer can be orders of magnitude off; both go to
/// `inv_beta_reg_bisect` instead.
pub(crate) fn inv_beta_reg(a: f64, b: f64, x: f64) -> Option<f64> {
    if !(a > 0.0 && b > 0.0 && a.is_finite() && b.is_finite() && (0.0..=1.0).contains(&x)) {
        return None;
    }
    if x == 0.0 {
        return Some(0.0);
    }
    if x == 1.0 {
        return Some(1.0);
    }
    if x * x == 0.0 {
        return inv_beta_reg_bisect(a, b, x);
    }
    let (root, resolved) = as109(a, b, x)?;
    if resolved || brackets_root(a, b, x, root)? {
        Some(root)
    } else {
        inv_beta_reg_bisect(a, b, x)
    }
}

/// statrs' AS 109 for `0 < x < 1` with `x * x > 0`: the root, and whether its
/// last Newton step was small enough to trust it.
fn as109(mut a: f64, mut b: f64, mut x: f64) -> Option<(f64, bool)> {
    const SAE: i32 = -30;
    const FPU: f64 = 1e-30;

    let ln_beta = ln_beta(a, b);
    let mut p;
    let mut q;

    let flip = 0.5 < x;
    if flip {
        p = a;
        a = b;
        b = p;
        x = 1.0 - x;
    }

    p = (-(x * x).ln()).sqrt();
    q = p - (2.30753 + 0.27061 * p) / (1.0 + (0.99229 + 0.04481 * p) * p);

    if 1.0 < a && 1.0 < b {
        let r = (q * q - 3.0) / 6.0;
        let s = 1.0 / (2.0 * a - 1.0);
        let t = 1.0 / (2.0 * b - 1.0);
        let h = 2.0 / (s + t);
        let w = q * (h + r).sqrt() / h - (t - s) * (r + 5.0 / 6.0 - 2.0 / (3.0 * h));
        p = a / (a + b * (2.0 * w).exp());
    } else {
        let mut t = 1.0 / (9.0 * b);
        t = 2.0 * b * (1.0 - t + q * t.sqrt()).powf(3.0);
        if t <= 0.0 {
            p = 1.0 - ((((1.0 - x) * b).ln() + ln_beta) / b).exp();
        } else {
            t = 2.0 * (2.0 * a + b - 1.0) / t;
            if t <= 1.0 {
                p = (((x * a).ln() + ln_beta) / a).exp();
            } else {
                p = 1.0 - 2.0 / (t + 1.0);
            }
        }
    }

    p = p.clamp(0.0001, 0.9999);

    let e = (-5.0 / a / a - 1.0 / x.powf(0.2) - 13.0) as i32;
    let acu = if e > SAE { f64::powi(10.0, e) } else { FPU };

    let mut pnext;
    let mut qprev = 0.0;
    let mut sq = 1.0;
    let mut prev = 1.0;

    let mut converged = false;
    'outer: for _ in 0..MAX_ITERATIONS {
        q = checked_beta_reg(a, b, p).ok()?;
        q = (q - x) * (ln_beta + (1.0 - a) * p.ln() + (1.0 - b) * (1.0 - p).ln()).exp();
        // With q finite, g underflows to 0 and pnext reaches p, which lies in
        // (0, 1), so the step-halving loops below exit.
        if !q.is_finite() {
            return None;
        }

        if q * qprev <= 0.0 {
            prev = if sq > FPU { sq } else { FPU };
        }

        let mut g = 1.0;
        loop {
            loop {
                let adj = g * q;
                sq = adj * adj;

                if sq < prev {
                    pnext = p - adj;
                    if (0.0..=1.0).contains(&pnext) {
                        break;
                    }
                }
                g /= 3.0;
            }

            if prev <= acu || q * q <= acu {
                p = pnext;
                converged = true;
                break 'outer;
            }

            if pnext != 0.0 && pnext != 1.0 {
                break;
            }

            g /= 3.0;
        }

        if pnext == p {
            converged = true;
            break;
        }

        p = pnext;
        qprev = q;
    }

    if !converged {
        return None;
    }
    // AS 109 accepts a step of up to sqrt(acu) in absolute terms. The error
    // that step leaves is about step^2 / d, d being the root's distance from
    // the nearer end of [0, 1]; trust the root only if that error is within
    // the same tolerance relative to d.
    let resolved = q * q <= acu.sqrt() * p.min(1.0 - p).powi(2);
    Some((if flip { 1.0 - p } else { p }, resolved))
}

/// Inverse of I_x(a, b) by bisection over the bit patterns of [0, 1], which
/// keeps relative accuracy at any size and ends after at most 64 steps.
fn inv_beta_reg_bisect(a: f64, b: f64, x: f64) -> Option<f64> {
    let mut below = 0.0f64.to_bits();
    let mut above = 1.0f64.to_bits();
    while above - below > 1 {
        let mid = below + (above - below) / 2;
        if below_root(a, b, x, f64::from_bits(mid))? {
            below = mid;
        } else {
            above = mid;
        }
    }
    Some(f64::from_bits(above))
}

/// Whether the root of I_t(a, b) = x lies within one ulp of `t`.
fn brackets_root(a: f64, b: f64, x: f64, t: f64) -> Option<bool> {
    if !(t > 0.0 && t < 1.0) {
        return Some(false);
    }
    Some(below_root(a, b, x, t.next_down())? && !below_root(a, b, x, t.next_up())?)
}

/// Whether I_t(a, b) < x. For x above 1/2 and t at or above 1/2 it compares
/// the upper tails instead, where 1 - x and 1 - t are exact and I_{1-t}(b, a)
/// keeps the digits that I_t(a, b) rounds away near 1.
fn below_root(a: f64, b: f64, x: f64, t: f64) -> Option<bool> {
    Some(if x > 0.5 && t >= 0.5 {
        checked_beta_reg(b, a, 1.0 - t).ok()? > 1.0 - x
    } else {
        checked_beta_reg(a, b, t).ok()? < x
    })
}

/// Quantile of Student's t with `df` degrees of freedom: an asymptotic
/// expansion for large df, otherwise as statrs computes it.
pub(crate) fn students_t_inverse_cdf(df: f64, p: f64) -> Option<f64> {
    if let Some(t) = students_t_inverse_cdf_large_df(df, p) {
        return Some(t);
    }
    let x1 = if p >= 0.5 { 1.0 - p } else { p };
    let y = inv_beta_reg(0.5 * df, 0.5, 2.0 * x1)?;
    let t = (df * (1. - y) / y).sqrt();
    Some(if p >= 0.5 { t } else { -t })
}

/// Quantile of the F distribution: an asymptotic expansion for large df,
/// otherwise as statrs computes it.
pub(crate) fn fisher_inverse_cdf(df1: f64, df2: f64, p: f64) -> Option<f64> {
    if let Some(f) = fisher_inverse_cdf_large_df(df1, df2, p) {
        return Some(f);
    }
    let z = inv_beta_reg(df1 / 2.0, df2 / 2.0, p)?;
    Some(df2 / (df1 * (1.0 / z - 1.0)))
}
