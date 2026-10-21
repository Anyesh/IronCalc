use statrs::function::beta::{checked_beta_reg, ln_beta};

const MAX_ITERATIONS: usize = 1000;

/// Inverse of the regularized incomplete beta function I_x(a, b), or `None`
/// when it does not converge.
///
/// Same arithmetic as statrs' `inv_beta_reg` (AS 109), so converging inputs
/// give identical results. statrs loops forever once the Newton step
/// overflows (huge shape parameters, or tails where the density underflows)
/// and panics on a NaN starting point; both return `None` here.
pub(crate) fn inv_beta_reg(mut a: f64, mut b: f64, mut x: f64) -> Option<f64> {
    const SAE: i32 = -30;
    const FPU: f64 = 1e-30;

    if !(a > 0.0 && b > 0.0 && a.is_finite() && b.is_finite() && (0.0..=1.0).contains(&x)) {
        return None;
    }
    if x == 0.0 {
        return Some(0.0);
    }
    if x == 1.0 {
        return Some(1.0);
    }

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
    Some(if flip { 1.0 - p } else { p })
}

/// Quantile of Student's t with `df` degrees of freedom, as statrs computes it.
pub(crate) fn students_t_inverse_cdf(df: f64, p: f64) -> Option<f64> {
    let x1 = if p >= 0.5 { 1.0 - p } else { p };
    let y = inv_beta_reg(0.5 * df, 0.5, 2.0 * x1)?;
    let t = (df * (1. - y) / y).sqrt();
    Some(if p >= 0.5 { t } else { -t })
}

/// Quantile of the F distribution, as statrs computes it.
pub(crate) fn fisher_inverse_cdf(df1: f64, df2: f64, p: f64) -> Option<f64> {
    let z = inv_beta_reg(df1 / 2.0, df2 / 2.0, p)?;
    Some(df2 / (df1 * (1.0 / z - 1.0)))
}
