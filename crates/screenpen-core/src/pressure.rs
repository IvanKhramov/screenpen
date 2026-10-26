/// Exponential moving average over pen pressure, mapped to a line width.
/// Pressure is smoothed separately from position.
#[derive(Clone, Copy, Debug)]
pub struct PressureFilter {
    pub alpha: f32,
    /// Width multiplier at zero pressure, so light strokes never vanish.
    pub minimum_scale: f32,
    value: Option<f32>,
}

impl Default for PressureFilter {
    fn default() -> Self {
        PressureFilter::new(0.3, 0.3)
    }
}

impl PressureFilter {
    pub fn new(alpha: f32, minimum_scale: f32) -> PressureFilter {
        PressureFilter {
            alpha,
            minimum_scale,
            value: None,
        }
    }

    pub fn value(&self) -> Option<f32> {
        self.value
    }

    pub fn reset(&mut self) {
        self.value = None;
    }

    /// Feeds a raw 0..=1 pressure sample and returns the smoothed value.
    pub fn filter(&mut self, pressure: f32) -> f32 {
        let clamped = pressure.clamp(0.0, 1.0);
        let next = self
            .value
            .map_or(clamped, |v| v + self.alpha * (clamped - v));
        self.value = Some(next);
        next
    }

    pub fn width(&self, base: f32, pressure: f32) -> f32 {
        base * (self.minimum_scale + (1.0 - self.minimum_scale) * pressure)
    }
}
