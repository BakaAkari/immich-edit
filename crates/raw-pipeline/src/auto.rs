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

    let diff = 128.0 - p50 as f64;
    let target_ev = if diff > 0.0 {
        (diff / 55.0).min(2.5)
    } else {
        (diff / 64.0).max(-2.0)
    };

    let ev_headroom = ((252.0 - p99 as f64) / 18.0).max(0.0);
    let exposure_ev = if target_ev > 0.0 {
        let cap = if clipped_frac > 0.05 {
            0.0
        } else if clipped_frac > 0.015 {
            ev_headroom.max(0.5)
        } else {
            ev_headroom.max(0.6)
        };
        target_ev.min(cap)
    } else {
        target_ev
    };

    let brightness = ((target_ev - exposure_ev) * 35.0).clamp(0.0, 30.0);

    let mut contrast = 10.0f64;
    if range < 200.0 {
        contrast += ((200.0 / range) - 1.0) * 8.0;
    }
    if highlight_frac > 0.06 {
        contrast *= 0.7;
    }
    contrast = contrast.clamp(-25.0, 30.0);

    let shadow_frac = hist_fraction_below(&s.hist, s.total, 32);
    let mut shadows = 0.0f64;
    if shadow_frac > 0.03 {
        shadows = (shadow_frac * 50.0 + 5.0).min(40.0);
    }

    let simulated_p01 = (p01 as f64 + exposure_ev * 22.0).clamp(0.0, 255.0);
    let simulated_p99 = (p99 as f64 + exposure_ev * 22.0).clamp(0.0, 255.0);

    let mut highlights = 0.0f64;
    if highlight_frac > 0.008 {
        highlights = -(highlight_frac * 180.0).min(70.0);
    }
    if clipped_frac > 0.003 {
        highlights -= (clipped_frac * 700.0).min(30.0);
    }
    if simulated_p99 > 240.0 {
        highlights -= (simulated_p99 - 240.0) * 1.5;
    }
    highlights = highlights.clamp(-85.0, 0.0);

    let mut clarity = 8.0f64;
    if highlight_frac > 0.06 {
        clarity *= 0.6;
    }
    if shadow_frac > 0.30 {
        clarity *= 0.7;
    }
    clarity = clarity.clamp(0.0, 14.0);

    let blacks = -((simulated_p01 - 5.0) * 0.4).clamp(-10.0, 20.0);
    let whites = if simulated_p99 < 240.0 {
        ((245.0 - simulated_p99) * 0.2).clamp(-30.0, 5.0)
    } else {
        ((simulated_p99 - 250.0) * -0.3).clamp(-30.0, 0.0)
    };

    let vibrance = (8.0 + (0.34 - s.mean_sat as f64) * 60.0).clamp(6.0, 30.0);

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
