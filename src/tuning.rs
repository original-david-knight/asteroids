pub const SHIP_OUTLINE_SEGMENT_DWELL_US: f32 = 30.0;
pub const ASTEROID_HULL_SEGMENT_DWELL_US: f32 = 25.0;
pub const BULLET_DOT_DWELL_US: f32 = 40.0;
pub const ENDPOINT_DWELL_BONUS_US: f32 = 10.0;

/// DESIGN.md Open Question 2 autonomous-run lock:
/// original Asteroids was a monochrome XY display, so the default beam is
/// pure white-on-black instead of a taste-tuned green/amber phosphor tint.
pub const DEFAULT_BEAM_RGB: [f32; 3] = [1.0, 1.0, 1.0];

pub const SHIP_ROTATION_RATE_RAD_PER_SEC: f32 = 3.0;
pub const SHIP_SPINNING_SCALE: f32 = 0.55;

pub const BEAM_QUAD_HALF_WIDTH_PIXELS: f32 = 6.0;
pub const BEAM_SIGMA_PIXELS: f32 = 1.0;
pub const BEAM_SIGMA_DWELL_GROWTH: f32 = 0.50;

pub const PHOSPHOR_TRAIL_LOW_DWELL_US: f32 = 10.0;
pub const PHOSPHOR_TRAIL_MID_DWELL_US: f32 = SHIP_OUTLINE_SEGMENT_DWELL_US;
pub const PHOSPHOR_TRAIL_HIGH_DWELL_US: f32 = 60.0;

pub const PHOSPHOR_TAU_DEFAULT_MS: f32 = 70.0;
pub const PHOSPHOR_TAU_MIN_MS: f32 = 50.0;
pub const PHOSPHOR_TAU_MAX_MS: f32 = 100.0;
pub const PHOSPHOR_TAU_STEP_MS: f32 = 5.0;
pub const PHOSPHOR_MAX_LUMA: f32 = 8.0;
pub const PHOSPHOR_FALLBACK_MAX_LUMA: f32 = 1.0;

/// Number of half-resolution bloom levels below the phosphor target.
pub const BLOOM_MIP_LEVELS: usize = 4;
/// Default bloom is intentionally restrained: enough to widen the glow, not the line core.
pub const BLOOM_INTENSITY_DEFAULT: f32 = 1.90;
/// HDR phosphor luma threshold used by the first Gaussian downsample prefilter.
pub const BLOOM_THRESHOLD_DEFAULT: f32 = 0.20;
pub const BLOOM_INTENSITY_MIN: f32 = 0.0;
pub const BLOOM_INTENSITY_MAX: f32 = 3.0;
pub const BLOOM_INTENSITY_STEP: f32 = 0.05;
pub const BLOOM_THRESHOLD_MIN: f32 = 0.0;
pub const BLOOM_THRESHOLD_MAX: f32 = 4.0;
pub const BLOOM_THRESHOLD_STEP: f32 = 0.05;

pub const GAMMA_RAMP_BARS: usize = 11;
pub const GAMMA_RAMP_X_MIN: f32 = -1.0;
pub const GAMMA_RAMP_X_MAX: f32 = 1.0;
pub const GAMMA_RAMP_Y_MIN: f32 = -0.88;
pub const GAMMA_RAMP_Y_MAX: f32 = 0.22;
