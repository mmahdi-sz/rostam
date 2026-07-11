//! DDIM scheduler — direct port of the validated reference implementation
//! (moebius-web's `ddim.ts`, itself validated against `diffusers.DDIMScheduler`
//! to ~5e-7). scaled_linear beta schedule, eta=0, clip_sample=false.
//!
//! Do not "simplify" the beta formula or the strength-trimming — both must
//! match the exported model's training schedule exactly or the denoiser
//! receives out-of-distribution alpha values and the output is garbage.

pub const NUM_TRAIN_TIMESTEPS: usize = 1000;
const BETA_START: f64 = 0.00085;
const BETA_END: f64 = 0.012;

pub struct Ddim {
    /// alphas_cumprod[t] for t in 0..NUM_TRAIN_TIMESTEPS.
    pub alphas_cumprod: Vec<f64>,
    /// Descending, strength-trimmed inference timesteps, e.g. [900, 850, ..., 0].
    pub timesteps: Vec<i64>,
}

/// Build the DDIM schedule for `num_steps` inference steps at the given
/// img2img `strength`. Moebius uses strength≈0.99 which drops the very first
/// (highest-noise) timestep from the nominal `num_steps`-length schedule.
pub fn make_ddim(num_steps: usize, strength: f64) -> Ddim {
    let a = BETA_START.sqrt();
    let b = BETA_END.sqrt();
    let mut alphas_cumprod = Vec::with_capacity(NUM_TRAIN_TIMESTEPS);
    let mut acc = 1.0_f64;
    for i in 0..NUM_TRAIN_TIMESTEPS {
        let s = a + (b - a) * (i as f64 / (NUM_TRAIN_TIMESTEPS as f64 - 1.0));
        let beta = s * s;
        acc *= 1.0 - beta;
        alphas_cumprod.push(acc);
    }

    let step_ratio = NUM_TRAIN_TIMESTEPS / num_steps; // floor division, matches np/diffusers
    let mut ts: Vec<i64> = (0..num_steps).map(|i| (i * step_ratio) as i64).collect();
    ts.reverse(); // [950, 900, ..., 0] for num_steps=20

    // Python's `int()` truncates (not rounds) — must match, or the trim point shifts.
    let init_timestep = ((num_steps as f64) * strength).floor().min(num_steps as f64) as usize;
    let t_start = num_steps.saturating_sub(init_timestep);
    let timesteps = ts[t_start..].to_vec();

    Ddim { alphas_cumprod, timesteps }
}

/// One DDIM update (eta=0, clip_sample=false):
///   pred_x0 = (sample - sqrt(1-ac_t)*eps) / sqrt(ac_t)
///   prev    = sqrt(ac_prev)*pred_x0 + sqrt(1-ac_prev)*eps
/// `prev_t = -1` is the sentinel for the last step (uses final_alpha_cumprod=1.0).
pub fn ddim_step(eps: &[f32], sample: &[f32], t: i64, prev_t: i64, ddim: &Ddim) -> Vec<f32> {
    let ac_t = ddim.alphas_cumprod[t as usize];
    let ac_prev = if prev_t >= 0 { ddim.alphas_cumprod[prev_t as usize] } else { 1.0 };
    let sqrt_ac_t = ac_t.sqrt();
    let sqrt_beta_t = (1.0 - ac_t).sqrt();
    let sqrt_ac_prev = ac_prev.sqrt();
    let sqrt_one_minus_ac_prev = (1.0 - ac_prev).sqrt();

    sample
        .iter()
        .zip(eps.iter())
        .map(|(&s, &e)| {
            let (s, e) = (s as f64, e as f64);
            let pred_x0 = (s - sqrt_beta_t * e) / sqrt_ac_t;
            (sqrt_ac_prev * pred_x0 + sqrt_one_minus_ac_prev * e) as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timesteps_20_steps_strength_099() {
        let ddim = make_ddim(20, 0.99);
        // step_ratio = 50 → nominal [950,900,...,0]; strength 0.99 → init_timestep=19 → drop first.
        assert_eq!(ddim.timesteps.first(), Some(&900));
        assert_eq!(ddim.timesteps.last(), Some(&0));
        assert_eq!(ddim.timesteps.len(), 19);
        // uniform 50-spacing preserved after trim
        for w in ddim.timesteps.windows(2) {
            assert_eq!(w[0] - w[1], 50);
        }
    }

    #[test]
    fn alphas_cumprod_monotonically_decreasing() {
        let ddim = make_ddim(20, 0.99);
        for w in ddim.alphas_cumprod.windows(2) {
            assert!(w[1] < w[0]);
        }
        assert!(ddim.alphas_cumprod[0] < 1.0 && ddim.alphas_cumprod[0] > 0.99);
    }
}
