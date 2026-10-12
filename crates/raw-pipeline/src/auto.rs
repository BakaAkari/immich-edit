mod stats;

#[cfg(test)]
mod tests;

use crate::edits::{BasicEdits, Edits, ToneEdits};
use crate::frame::RawFrame;
use crate::sensor_sample::{decimate_mosaic, display_color};
use stats::{
    collect_stats_direct, collect_stats_output, hist_fraction_above, hist_fraction_below,
    hist_percentile, needs_output_pass,
};

const HIST_BINS: usize = 256;
const MIN_VALID_SAMPLES: u32 = 1000;
pub fn scale_matrix(m: [[f32; 3]; 3], gain: f32) -> [[f32; 3]; 3] {
    [
        [m[0][0] * gain, m[0][1] * gain, m[0][2] * gain],
        [m[1][0] * gain, m[1][1] * gain, m[1][2] * gain],
        [m[2][0] * gain, m[2][1] * gain, m[2][2] * gain],
    ]
}
pub fn auto_adjust(frame: &RawFrame, context: &Edits) -> Edits {
    let context = context.clamped();
    let (wb, m) = display_color(frame);
    let decimated = decimate_mosaic(frame);
    let frame = decimated.as_ref().unwrap_or(frame);
    let stats = if needs_output_pass(&context) {
        collect_stats_output(frame, &context, wb, m).or_else(|| collect_stats_direct(frame, wb, m))
    } else {
        collect_stats_direct(frame, wb, m)
    };
    let Some(s) = stats else {
        return Edits::default();
    };

    let p01 = hist_percentile(&s.hist, s.total, 0.01);
    let p50 = hist_percentile(&s.hist, s.total, 0.50);
    let p99 = hist_percentile(&s.hist, s.total, 0.99);

    let range = (p99 as f64 - p01 as f64).max(1.0);

    let highlight_frac = hist_fraction_above(&s.hist, s.total, 240);
    let clipped_frac = hist_fraction_above(&s.hist, s.total, 250);

    // Social-ready, Capture One style punchy & luminous heuristics:
    // Target perceptual luminous midtone around 140 (standard for mobile/social), not dull 128.
    let diff = 136.0 - p50 as f64;
    let target_ev = if diff > 0.0 {
        (diff / 39.0).min(2.5)
    } else {
        (diff / 50.0).max(-2.0)
    };

    // Calculate headroom with specular clipping awareness
    let ev_headroom = ((254.0 - p99 as f64) / 12.0).max(0.0);
    let exposure_ev = if target_ev > 0.0 {
        let cap = if clipped_frac > 0.08 {
            0.0
        } else if clipped_frac > 0.03 {
            ev_headroom.max(0.7)
        } else {
            ev_headroom.max(1.2)
        };
        target_ev.min(cap)
    } else {
        target_ev
    };

    let brightness = ((target_ev - exposure_ev) * 35.0).clamp(0.0, 35.0);

    // C1-style punchy tone curve & contrast:
    let mut contrast = 15.0f64;
    if range < 210.0 {
        contrast += ((210.0 / range) - 1.0) * 12.0;
    }
    if highlight_frac > 0.08 {
        contrast *= 0.75;
    }
    contrast = contrast.clamp(-15.0, 35.0);

    // Deep shadow lifting (HDR style):
    let shadow_frac = hist_fraction_below(&s.hist, s.total, 40);
    let mut shadows = 18.0f64;
    if shadow_frac > 0.02 {
        shadows = (shadows + shadow_frac * 60.0).min(50.0);
    }

    let simulated_p01 = (p01 as f64 + exposure_ev * 24.0).clamp(0.0, 255.0);
    let simulated_p99 = (p99 as f64 + exposure_ev * 24.0).clamp(0.0, 255.0);

    // Aggressive highlight recovery to protect highlights under lifted exposure:
    let mut highlights = -25.0f64;
    if highlight_frac > 0.005 {
        highlights -= (highlight_frac * 200.0).min(45.0);
    }
    if clipped_frac > 0.002 {
        highlights -= (clipped_frac * 800.0).min(30.0);
    }
    if simulated_p99 > 235.0 {
        highlights -= (simulated_p99 - 235.0) * 1.5;
    }
    highlights = highlights.clamp(-90.0, -10.0);

    // Microcontrast clarity
    let mut clarity = 10.0f64;
    if highlight_frac > 0.08 {
        clarity *= 0.7;
    }
    if shadow_frac > 0.35 {
        clarity *= 0.8;
    }
    clarity = clarity.clamp(4.0, 18.0);

    // Auto-levels stretching:
    // Pin black point down to ensure inky blacks & remove haze/fog:
    let blacks = -((simulated_p01).max(8.0) * 0.45).clamp(8.0, 28.0);

    // Stretch whites up if headroom permits, else compress:
    let whites = if simulated_p99 < 235.0 {
        ((240.0 - simulated_p99) * 0.3).clamp(5.0, 25.0)
    } else {
        ((simulated_p99 - 245.0) * -0.4).clamp(-30.0, 0.0)
    };

    // Rich vibrance for vibrant social look:
    let vibrance = (14.0 + (0.34 - s.mean_sat as f64) * 65.0).clamp(10.0, 35.0);
    Edits {
        basic: BasicEdits {
            exposure_ev,
            brightness,
            contrast,
            clarity,
            vibrance,
            ..Default::default()
        },
        tone: ToneEdits {
            highlights,
            shadows,
            blacks,
            whites,
        },
        color: Default::default(),
        detail: Default::default(),
        effects: Default::default(),
        lens: Default::default(),
        geometry: Default::default(),
        masks: Vec::new(),
        retouch: Vec::new(),
        unknown_ops: Default::default(),
    }
}
