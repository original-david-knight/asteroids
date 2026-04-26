/// Polish-pass beam dwell defaults. These keep the original Asteroids object
/// hierarchy readable on a modern high-DPI display: ship lines are the
/// reference brightness, asteroids sit slightly behind them, bullets pop, and
/// corners retain a small endpoint pause.
pub const SHIP_OUTLINE_SEGMENT_DWELL_US: f32 = 30.0;
pub const ASTEROID_HULL_SEGMENT_DWELL_US: f32 = 25.0;
pub const BULLET_DOT_DWELL_US: f32 = 40.0;
pub const ENDPOINT_DWELL_BONUS_US: f32 = 10.0;

/// Closed DESIGN color question: original Asteroids was a monochrome XY
/// display, so v1 stays pure white-on-black instead of a taste-tuned
/// green/amber phosphor tint.
pub const DEFAULT_BEAM_RGB: [f32; 3] = [1.0, 1.0, 1.0];

pub const SHIP_ROTATION_RATE_RAD_PER_SEC: f32 = 3.0;
pub const SHIP_GAMEPLAY_SCALE: f32 = 0.10;
pub const SHIP_SPINNING_SCALE: f32 = 0.55;

/// Step-11 asteroid constants tuned against the original 6502 listings:
/// - Norbert Kehrer static binary translation / disassembly-derived exact port:
///   https://norbertkehrer.github.io/ast_js/AsteroidsJS.html
/// - https://computerarcheology.com/Arcade/Asteroids/Code.html
/// - https://6502disassembly.com/va-asteroids/Asteroids.html
///
/// The listings store object coordinates at 8 raw position units per visible
/// game unit and clamp asteroid velocity bytes to +/-6..31 at $7233-$724e.
pub const ASTEROID_ORIGINAL_PLAYFIELD_HEIGHT_UNITS: f32 = 768.0;
pub const ASTEROID_ORIGINAL_VISIBLE_UNITS_TO_NDC: f32 =
    2.0 / ASTEROID_ORIGINAL_PLAYFIELD_HEIGHT_UNITS;
pub const ASTEROID_ORIGINAL_RAW_UNITS_PER_VISIBLE_UNIT: f32 = 8.0;
pub const ASTEROID_ORIGINAL_FPS: f32 = 60.0;
pub const ASTEROID_RAW_VELOCITY_TO_NDC_PER_SEC: f32 = ASTEROID_ORIGINAL_FPS
    * ASTEROID_ORIGINAL_VISIBLE_UNITS_TO_NDC
    / ASTEROID_ORIGINAL_RAW_UNITS_PER_VISIBLE_UNIT;
/// Playability scale over the disassembly-derived raw conversion. The unscaled
/// value remains available for saucer movement, but rocks need a calmer drift
/// on the modern full-screen playfield.
pub const ASTEROID_DRIFT_SPEED_SCALE: f32 = 0.45;
pub const ASTEROID_RAW_VELOCITY_TO_DRIFT_NDC_PER_SEC: f32 =
    ASTEROID_RAW_VELOCITY_TO_NDC_PER_SEC * ASTEROID_DRIFT_SPEED_SCALE;
pub const ASTEROID_RAW_VELOCITY_MIN: f32 = 6.0;
pub const ASTEROID_RAW_VELOCITY_MAX: f32 = 31.0;
pub const ASTEROID_DRIFT_SPEED_MIN_NDC_PER_SEC: f32 =
    ASTEROID_RAW_VELOCITY_MIN * ASTEROID_RAW_VELOCITY_TO_DRIFT_NDC_PER_SEC;
pub const ASTEROID_DRIFT_SPEED_MAX_NDC_PER_SEC: f32 =
    ASTEROID_RAW_VELOCITY_MAX * ASTEROID_RAW_VELOCITY_TO_DRIFT_NDC_PER_SEC;
pub const ASTEROID_LARGE_RADIUS_UNITS: f32 = 30.0;
pub const ASTEROID_MEDIUM_RADIUS_UNITS: f32 = 15.0;
pub const ASTEROID_SMALL_RADIUS_UNITS: f32 = 7.0;

pub const BEAM_QUAD_HALF_WIDTH_PIXELS: f32 = 6.0;
/// Flat center of the vector beam in physical pixels.
pub const BEAM_CORE_RADIUS_PIXELS: f32 = 1.35;
/// Soft edge falloff around the solid beam core in physical pixels.
pub const BEAM_SIGMA_PIXELS: f32 = 0.85;
/// Extra spot growth when dwell rises above the ship-line reference dwell.
pub const BEAM_SIGMA_DWELL_GROWTH: f32 = 0.15;

pub const PHOSPHOR_TRAIL_LOW_DWELL_US: f32 = 10.0;
pub const PHOSPHOR_TRAIL_MID_DWELL_US: f32 = SHIP_OUTLINE_SEGMENT_DWELL_US;
pub const PHOSPHOR_TRAIL_HIGH_DWELL_US: f32 = 60.0;

/// Phosphor history is disabled by default so moving gameplay objects render
/// only at their current positions. Debug controls can raise this for decay
/// verification and intentional trail captures.
pub const PHOSPHOR_TAU_DEFAULT_MS: f32 = 0.0;
pub const PHOSPHOR_TAU_MIN_MS: f32 = 0.0;
pub const PHOSPHOR_TAU_MAX_MS: f32 = 100.0;
pub const PHOSPHOR_TAU_STEP_MS: f32 = 5.0;
pub const PHOSPHOR_MAX_LUMA: f32 = 8.0;
pub const PHOSPHOR_FALLBACK_MAX_LUMA: f32 = 1.0;

/// Number of half-resolution bloom levels below the phosphor target.
pub const BLOOM_MIP_LEVELS: usize = 2;
/// Default bloom is intentionally subtle: a readable CRT halo without burying
/// vector lines and title text under the glow.
pub const BLOOM_INTENSITY_DEFAULT: f32 = 0.02;
/// HDR phosphor luma threshold used by the first Gaussian downsample prefilter.
pub const BLOOM_THRESHOLD_DEFAULT: f32 = 1.00;
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
