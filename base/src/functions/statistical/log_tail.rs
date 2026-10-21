//! Pieces shared by the quantile solvers: Newton's method on the log of a
//! tail probability, and the parts of their densities that keep differences
//! of log-gammas accurate for large parameters through Stirling's series.

use statrs::function::gamma::ln_gamma;

/// ln √(2π)
pub(crate) const LN_SQRT_2PI: f64 = 0.918_938_533_204_672_7;

const MAX_NEWTON_STEPS: usize = 200;

/// Solves ln tail(v) = target by Newton's method, where `eval` returns
/// ln tail(v) and |d ln tail / dv|, and the tail increases with v unless
/// `decreasing`. A step that leaves the bracket found so far is replaced by
/// bisection. Returns the last point evaluated and the step from it, which is
/// down to rounding.
pub(crate) fn newton_log_tail(
    mut v: f64,
    target: f64,
    decreasing: bool,
    eval: impl Fn(f64) -> Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    let mut below = f64::NEG_INFINITY;
    let mut above = f64::INFINITY;
    let mut previous_step = f64::INFINITY;
    for _ in 0..MAX_NEWTON_STEPS {
        let (log_tail, slope) = eval(v)?;
        if !log_tail.is_finite() {
            return None;
        }
        let residual = log_tail - target;
        let step = if decreasing {
            -residual / slope
        } else {
            residual / slope
        };
        let scale = v.abs().max(1.0);
        // Newton's steps shrink quadratically until the rounding in ln tail
        // takes over; a small step that no longer halves is that noise.
        if step.abs() <= 4.0 * f64::EPSILON * scale
            || (step.abs() <= 1e-9 * scale && step.abs() > 0.5 * previous_step)
        {
            return Some((v, step));
        }
        previous_step = step.abs();
        if (residual > 0.0) != decreasing {
            above = v;
        } else {
            below = v;
        }
        let mut next = v - step;
        if !(next > below && next < above) {
            next = match (below.is_finite(), above.is_finite()) {
                (true, true) => 0.5 * (below + above),
                (true, false) => below + 1.0,
                (false, true) => above - 1.0,
                (false, false) => return None,
            };
        }
        v = next;
    }
    None
}

/// u - ln(1 + u), given ln(1 + u). For |u| <= 1/2 it sums
/// u² / (2 + u) - 2 (w³/3 + w⁵/5 + ...) with w = u / (2 + u), since
/// ln(1 + u) = 2 artanh(w), instead of subtracting nearly equal numbers.
pub(crate) fn u_minus_ln_1p(u: f64, ln_1p_u: f64) -> f64 {
    if u.abs() > 0.5 {
        return u - ln_1p_u;
    }
    let w = u / (2.0 + u);
    let w2 = w * w;
    let mut power = w * w2;
    let mut sum = 0.0;
    let mut k = 3.0;
    loop {
        let term = power / k;
        sum += term;
        if term.abs() <= f64::EPSILON / 4.0 * sum.abs() {
            break;
        }
        power *= w2;
        k += 2.0;
    }
    u * u / (2.0 + u) - 2.0 * sum
}

/// ln Γ(z) less (z - 1/2) ln z - z + ln √(2π).
pub(crate) fn stirling_remainder(z: f64) -> f64 {
    if z < 15.0 {
        return ln_gamma(z) - ((z - 0.5) * z.ln() - z + LN_SQRT_2PI);
    }
    // Through the z⁻⁹ term; the next is below 3e-16 from z = 15.
    let r = 1.0 / z;
    let r2 = r * r;
    r * (1.0 / 12.0 - r2 * (1.0 / 360.0 - r2 * (1.0 / 1260.0 - r2 * (1.0 / 1680.0 - r2 / 1188.0))))
}
