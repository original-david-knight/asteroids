use std::{array, f32::consts::TAU};

use crate::{
    audio::GameSnapshot,
    beam::Vec2,
    rng::{SeededRng, rng_for_seed},
    tuning,
};

pub const FIXED_TIMESTEP_SECONDS: f32 = 1.0 / 240.0;
pub const MAX_SUBSTEPS_PER_FRAME: u32 = 4;
pub const PLAYFIELD_MIN: Vec2 = Vec2::new(-1.0, -1.0);
pub const PLAYFIELD_MAX: Vec2 = Vec2::new(1.0, 1.0);
/// Polish-pass disassembly recheck:
/// - ship direction changes by +/-3 direction bytes per input sample at $7086-$7099,
///   which this build keeps as the DESIGN-level 3 rad/s feel constant in tuning.rs;
/// - thrust is applied through the original acceleration registers every other
///   60 Hz frame at $709b-$70de; this build scales the DESIGN 0.05 units/frame
///   down to the NDC playfield so the ship does not jump across the screen;
/// - ship max velocity uses the same playable NDC scale, because the original
///   byte clamp is hardware-scale specific;
/// - idle drag is a deliberate v1 feel change so releasing thrust lets the ship
///   slow down instead of coasting forever.
pub const SHIP_MAX_VELOCITY_UNITS_PER_SEC: f32 = 1.5;
pub const SHIP_THRUST_ACCEL_PLAYABILITY_SCALE: f32 = 0.30;
pub const SHIP_THRUST_ACCEL_UNITS_PER_SEC_SQUARED: f32 =
    0.05 * 60.0 * SHIP_THRUST_ACCEL_PLAYABILITY_SCALE;
pub const SHIP_IDLE_DRAG_PER_SEC: f32 = 1.6;
const SHIP_IDLE_STOP_SPEED_UNITS_PER_SEC: f32 = 0.01;
pub const SCORE_PLACEHOLDER: u32 = 0;
/// Original score constants from the 6502 disassembly:
/// - Asteroid table `AstPointsTbl` at $7659 is BCD `$10,$05,$02`, documented
///   as score increases 100, 50, 20.
/// - Saucer hit code at $6b85/$6b89 loads `SmallScrPnts` `$99` and
///   `LargeScrPnts` `$20`, documented as 990 and 200 points.
///
/// Sources:
/// https://6502disassembly.com/va-asteroids/Asteroids.html#SymAstPointsTbl
/// https://6502disassembly.com/va-asteroids/Asteroids.html#SymSaucerHit
pub const ASTEROID_LARGE_SCORE: u32 = 20;
pub const ASTEROID_MEDIUM_SCORE: u32 = 50;
pub const ASTEROID_SMALL_SCORE: u32 = 100;
pub const UFO_LARGE_SCORE: u32 = 200;
pub const UFO_SMALL_SCORE: u32 = 990;
pub const EXTRA_LIFE_SCORE_INTERVAL: u32 = 10_000;
pub const MAX_DISPLAYED_LIVES: u32 = 6;
pub const ASTEROIDS_PER_WAVE_BOOTSTRAP: u32 = 2;
pub const ASTEROIDS_PER_WAVE_INCREMENT: u32 = 2;
pub const ASTEROIDS_PER_WAVE_MAX: u32 = 11;
pub const ASTEROID_HULL_VERTEX_COUNT: usize = 10;
pub const INITIAL_LIVES: u32 = 3;
pub const BULLET_LIFETIME_SECONDS: f32 = 1.0;
pub const BULLET_SPEED_NDC_PER_SEC: f32 = 1.65;
pub const BULLET_RADIUS_NDC: f32 = 0.012;
pub const SHIP_COLLISION_RADIUS_NDC: f32 = 0.44 * tuning::SHIP_GAMEPLAY_SCALE;
pub const SHIP_RESPAWN_DELAY_SECONDS: f32 = 1.25;
pub const SHIP_RESPAWN_INVULNERABILITY_SECONDS: f32 = 1.25;
pub const HYPERSPACE_COOLDOWN_SECONDS: f32 = 1.0;
pub const HYPERSPACE_SELF_DESTRUCT_CHANCE: f32 = 0.10;
/// DESIGN.md fixes the small-saucer transition at 10,000 points for v1.
/// The disassembly's BCD score gate is ambiguous enough that the project
/// contract wins here.
pub const UFO_SMALL_SCORE_THRESHOLD: u32 = 10_000;
pub const UFO_SPAWN_SCORE_STEP_POINTS: u32 = 2_500;
pub const UFO_BULLET_SPEED_NDC_PER_SEC: f32 = 1.25;

const UFO_ORIGINAL_TIMER_TICK_SECONDS: f32 = 4.0 / tuning::ASTEROID_ORIGINAL_FPS;
const UFO_SPAWN_RELOAD_INITIAL_TICKS: u32 = 0x92;
const UFO_SPAWN_RELOAD_DECREMENT_TICKS: u32 = 0x06;
const UFO_SPAWN_RELOAD_MIN_TICKS: u32 = 0x20;
const UFO_SHOT_RELOAD_TICKS: u32 = 0x0A;
const UFO_DIRECTION_CHANGE_SECONDS: f32 = 128.0 / tuning::ASTEROID_ORIGINAL_FPS;
const UFO_EDGE_MARGIN_NDC: f32 = 0.12;
const UFO_VERTICAL_BOUND_NDC: f32 = 0.86;
const UFO_LARGE_RADIUS_NDC: f32 = 0.092;
const UFO_SMALL_RADIUS_NDC: f32 = 0.062;
const UFO_RAW_HORIZONTAL_SPEED: f32 = 16.0;
const UFO_RAW_VERTICAL_SPEEDS: [f32; 4] = [-16.0, 0.0, 0.0, 16.0];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControlState {
    pub rotate_left: bool,
    pub rotate_right: bool,
    pub thrust: bool,
    pub fire: bool,
    pub hyperspace: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShipState {
    pub position: Vec2,
    pub velocity: Vec2,
    pub angle: f32,
}

impl Default for ShipState {
    fn default() -> Self {
        Self {
            position: Vec2::ZERO,
            velocity: Vec2::ZERO,
            angle: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AsteroidSize {
    Large,
    Medium,
    Small,
}

impl AsteroidSize {
    pub fn original_radius_units(self) -> f32 {
        match self {
            Self::Large => tuning::ASTEROID_LARGE_RADIUS_UNITS,
            Self::Medium => tuning::ASTEROID_MEDIUM_RADIUS_UNITS,
            Self::Small => tuning::ASTEROID_SMALL_RADIUS_UNITS,
        }
    }

    pub fn radius_ndc(self) -> f32 {
        self.original_radius_units() * tuning::ASTEROID_ORIGINAL_VISIBLE_UNITS_TO_NDC
    }

    pub fn next_smaller(self) -> Option<Self> {
        match self {
            Self::Large => Some(Self::Medium),
            Self::Medium => Some(Self::Small),
            Self::Small => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Large => "large",
            Self::Medium => "medium",
            Self::Small => "small",
        }
    }

    pub fn score_value(self) -> u32 {
        match self {
            Self::Large => ASTEROID_LARGE_SCORE,
            Self::Medium => ASTEROID_MEDIUM_SCORE,
            Self::Small => ASTEROID_SMALL_SCORE,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AsteroidHull {
    vertices: [Vec2; ASTEROID_HULL_VERTEX_COUNT],
}

impl AsteroidHull {
    fn random(rng: &mut SeededRng) -> Self {
        let step = TAU / ASTEROID_HULL_VERTEX_COUNT as f32;
        let vertices = array::from_fn(|index| {
            let angle = index as f32 * step + (rng.next_f32() - 0.5) * step * 0.36;
            let radius = 0.78 + rng.next_f32() * 0.40;
            let (sin, cos) = angle.sin_cos();
            Vec2::new(cos, sin) * radius
        });
        Self { vertices }
    }

    fn regular() -> Self {
        let step = TAU / ASTEROID_HULL_VERTEX_COUNT as f32;
        Self {
            vertices: array::from_fn(|index| {
                let (sin, cos) = (index as f32 * step).sin_cos();
                Vec2::new(cos, sin)
            }),
        }
    }

    pub fn vertices(&self) -> &[Vec2; ASTEROID_HULL_VERTEX_COUNT] {
        &self.vertices
    }
}

impl Default for AsteroidHull {
    fn default() -> Self {
        Self::regular()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Asteroid {
    pub id: u32,
    pub size: AsteroidSize,
    pub position: Vec2,
    pub velocity: Vec2,
    pub hull: AsteroidHull,
    pub wrapped_last_tick: bool,
}

impl Asteroid {
    fn new(
        id: u32,
        size: AsteroidSize,
        position: Vec2,
        velocity: Vec2,
        hull: AsteroidHull,
    ) -> Self {
        Self {
            id,
            size,
            position,
            velocity,
            hull,
            wrapped_last_tick: false,
        }
    }

    fn integrate(&mut self, dt: f32) {
        let (position, wrapped) = wrap_position_with_report(self.position + self.velocity * dt);
        self.position = position;
        self.wrapped_last_tick = wrapped;
    }

    pub fn radius_ndc(self) -> f32 {
        self.size.radius_ndc()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bullet {
    pub id: u32,
    pub position: Vec2,
    pub velocity: Vec2,
    pub age_seconds: f32,
    pub wrapped_last_tick: bool,
}

impl Bullet {
    fn new(id: u32, position: Vec2, velocity: Vec2) -> Self {
        Self {
            id,
            position,
            velocity,
            age_seconds: 0.0,
            wrapped_last_tick: false,
        }
    }

    fn integrate(&mut self, dt: f32) {
        let (position, wrapped) = wrap_position_with_report(self.position + self.velocity * dt);
        self.position = position;
        self.age_seconds += dt;
        self.wrapped_last_tick = wrapped;
    }

    pub fn is_expired(self) -> bool {
        self.age_seconds >= BULLET_LIFETIME_SECONDS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UfoVariant {
    Large,
    Small,
}

impl UfoVariant {
    pub fn name(self) -> &'static str {
        match self {
            Self::Large => "large",
            Self::Small => "small",
        }
    }

    pub fn audio_variant(self) -> f32 {
        match self {
            Self::Large => 0.0,
            Self::Small => 1.0,
        }
    }

    pub fn score_value(self) -> u32 {
        match self {
            Self::Large => UFO_LARGE_SCORE,
            Self::Small => UFO_SMALL_SCORE,
        }
    }

    pub fn radius_ndc(self) -> f32 {
        match self {
            Self::Large => UFO_LARGE_RADIUS_NDC,
            Self::Small => UFO_SMALL_RADIUS_NDC,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ufo {
    pub id: u32,
    pub variant: UfoVariant,
    pub position: Vec2,
    pub velocity: Vec2,
    pub shot_timer_seconds: f32,
    pub direction_timer_seconds: f32,
}

impl Ufo {
    fn new(id: u32, variant: UfoVariant, position: Vec2, velocity: Vec2) -> Self {
        Self {
            id,
            variant,
            position,
            velocity,
            shot_timer_seconds: UFO_ORIGINAL_TIMER_TICK_SECONDS,
            direction_timer_seconds: UFO_DIRECTION_CHANGE_SECONDS,
        }
    }

    fn integrate(&mut self, dt: f32) {
        self.position = self.position + self.velocity * dt;
        if self.position.y < -UFO_VERTICAL_BOUND_NDC {
            self.position.y = -UFO_VERTICAL_BOUND_NDC;
            self.velocity.y = self.velocity.y.abs();
        } else if self.position.y > UFO_VERTICAL_BOUND_NDC {
            self.position.y = UFO_VERTICAL_BOUND_NDC;
            self.velocity.y = -self.velocity.y.abs();
        }
    }

    pub fn radius_ndc(self) -> f32 {
        self.variant.radius_ndc()
    }

    fn is_offscreen(self) -> bool {
        self.position.x < PLAYFIELD_MIN.x - UFO_EDGE_MARGIN_NDC
            || self.position.x > PLAYFIELD_MAX.x + UFO_EDGE_MARGIN_NDC
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderAsteroid {
    pub id: u32,
    pub size: AsteroidSize,
    pub position: Vec2,
    pub radius: f32,
    pub hull: AsteroidHull,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderBullet {
    pub id: u32,
    pub position: Vec2,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderUfo {
    pub id: u32,
    pub variant: UfoVariant,
    pub position: Vec2,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AsteroidSizeCounts {
    pub large: u32,
    pub medium: u32,
    pub small: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameEventKind {
    BulletFired,
    BulletExpired,
    BulletHitAsteroid,
    AsteroidSplit,
    AsteroidDestroyed,
    UfoSpawned,
    UfoSirenOn,
    UfoSirenOff,
    UfoDespawned,
    UfoDestroyed,
    UfoFiredRandom,
    UfoFiredAimed,
    ScoreIncreased,
    ExtraLifeAwarded,
    ScoreGte10000,
    ShipDied,
    LivesDecremented,
    Respawn,
    GameOver,
    HyperspaceTriggered,
    HyperspaceCooldownRejected,
    HyperspaceSelfDestruct,
    HighScoreIncreased,
}

impl GameEventKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::BulletFired => "bullet-fired",
            Self::BulletExpired => "bullet-expired",
            Self::BulletHitAsteroid => "bullet-hit-asteroid",
            Self::AsteroidSplit => "asteroid-split",
            Self::AsteroidDestroyed => "asteroid-destroyed",
            Self::UfoSpawned => "ufo-spawned",
            Self::UfoSirenOn => "ufo-siren-on",
            Self::UfoSirenOff => "ufo-siren-off",
            Self::UfoDespawned => "ufo-despawned",
            Self::UfoDestroyed => "ufo-destroyed",
            Self::UfoFiredRandom => "ufo-fired-random",
            Self::UfoFiredAimed => "ufo-fired-aimed",
            Self::ScoreIncreased => "score-increased",
            Self::ExtraLifeAwarded => "extra-life-awarded",
            Self::ScoreGte10000 => "score-gte-10000",
            Self::ShipDied => "ship-died",
            Self::LivesDecremented => "lives-decremented",
            Self::Respawn => "respawn",
            Self::GameOver => "game-over",
            Self::HyperspaceTriggered => "hyperspace-triggered",
            Self::HyperspaceCooldownRejected => "cooldown-rejected",
            Self::HyperspaceSelfDestruct => "hyperspace-self-destruct",
            Self::HighScoreIncreased => "highscore-increased",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameEvent {
    pub kind: GameEventKind,
    pub asteroid_size: Option<AsteroidSize>,
    pub ufo_variant: Option<UfoVariant>,
    pub score_delta: Option<u32>,
    pub extra_life_threshold: Option<u32>,
    pub high_score: Option<u32>,
}

impl GameEvent {
    fn new(kind: GameEventKind) -> Self {
        Self {
            kind,
            asteroid_size: None,
            ufo_variant: None,
            score_delta: None,
            extra_life_threshold: None,
            high_score: None,
        }
    }

    fn asteroid(kind: GameEventKind, asteroid_size: AsteroidSize) -> Self {
        Self {
            kind,
            asteroid_size: Some(asteroid_size),
            ufo_variant: None,
            score_delta: None,
            extra_life_threshold: None,
            high_score: None,
        }
    }

    fn ufo(kind: GameEventKind, ufo_variant: UfoVariant) -> Self {
        Self {
            kind,
            asteroid_size: None,
            ufo_variant: Some(ufo_variant),
            score_delta: None,
            extra_life_threshold: None,
            high_score: None,
        }
    }

    fn score(delta: u32) -> Self {
        Self {
            kind: GameEventKind::ScoreIncreased,
            asteroid_size: None,
            ufo_variant: None,
            score_delta: Some(delta),
            extra_life_threshold: None,
            high_score: None,
        }
    }

    fn extra_life(threshold: u32) -> Self {
        Self {
            kind: GameEventKind::ExtraLifeAwarded,
            asteroid_size: None,
            ufo_variant: None,
            score_delta: None,
            extra_life_threshold: Some(threshold),
            high_score: None,
        }
    }

    fn high_score(score: u32) -> Self {
        Self {
            kind: GameEventKind::HighScoreIncreased,
            asteroid_size: None,
            ufo_variant: None,
            score_delta: None,
            extra_life_threshold: None,
            high_score: Some(score),
        }
    }

    pub fn name(self) -> &'static str {
        self.kind.name()
    }

    pub fn state_log_name(self) -> String {
        match (self.kind, self.extra_life_threshold) {
            (GameEventKind::ExtraLifeAwarded, Some(threshold)) => {
                format!("extra-life-at-{threshold}")
            }
            _ => self.name().to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum ScriptedScenario {
    #[default]
    None,
    BulletHitAsteroidThreeTier,
    AutonomousPlay10Min,
    ScoreProgression,
    SetHighScore12345,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GameState {
    pub ship: ShipState,
    pub alive: bool,
    pub game_over: bool,
    pub lives: u32,
    pub score: u32,
    pub asteroid_count: u32,
    pub round: u32,
    pub asteroids: Vec<Asteroid>,
    pub bullets: Vec<Bullet>,
    pub ufo: Option<Ufo>,
    pub ufo_bullets: Vec<Bullet>,
    next_asteroid_id: u32,
    next_bullet_id: u32,
    next_ufo_id: u32,
    next_extra_life_score: u32,
    respawn_timer_seconds: f32,
    invulnerability_timer_seconds: f32,
    hyperspace_cooldown_timer_seconds: f32,
    hyperspace_was_down: bool,
    ufo_spawn_timer_seconds: f32,
    fire_was_down: bool,
    events: Vec<GameEvent>,
    rng: SeededRng,
    script: ScriptedScenario,
    script_tick: u32,
    script_phase: u32,
    script_wait_until_tick: u32,
}

impl Default for GameState {
    fn default() -> Self {
        Self::new_seeded(None)
    }
}

impl GameState {
    pub fn new_seeded(seed: Option<u64>) -> Self {
        let mut state = Self::empty_seeded(seed);
        state.start_round(1);
        state
    }

    pub fn bullet_hit_asteroid_scenario(seed: Option<u64>) -> Self {
        let mut state = Self::empty_seeded(seed);
        let asteroid = state.allocate_asteroid(
            AsteroidSize::Large,
            Vec2::new(0.55, 0.0),
            Vec2::ZERO,
            AsteroidHull::regular(),
        );
        state.asteroids.push(asteroid);
        state.script = ScriptedScenario::BulletHitAsteroidThreeTier;
        state.sync_asteroid_count();
        state
    }

    pub fn ship_collides_with_asteroid_scenario(seed: Option<u64>) -> Self {
        let mut state = Self::empty_seeded(seed);
        let asteroid = state.allocate_asteroid(
            AsteroidSize::Large,
            Vec2::new(0.18, 0.0),
            Vec2::new(0.22, 0.0),
            AsteroidHull::regular(),
        );
        state.asteroids.push(asteroid);
        state.sync_asteroid_count();
        state
    }

    pub fn lose_all_lives_scenario(seed: Option<u64>) -> Self {
        let mut state = Self::empty_seeded(seed);
        let asteroid = state.allocate_asteroid(
            AsteroidSize::Large,
            Vec2::ZERO,
            Vec2::ZERO,
            AsteroidHull::regular(),
        );
        state.asteroids.push(asteroid);
        state.sync_asteroid_count();
        state
    }

    pub fn ufo_large_scenario(seed: Option<u64>) -> Self {
        let mut state = Self::empty_seeded(seed);
        state.set_score_without_bonus(0);
        state.reset_ufo_spawn_timer();
        state
    }

    pub fn ufo_small_scenario(seed: Option<u64>) -> Self {
        let mut state = Self::empty_seeded(seed);
        state.set_score_without_bonus(UFO_SMALL_SCORE_THRESHOLD);
        state.ship.position = Vec2::new(0.18, -0.12);
        state.reset_ufo_spawn_timer();
        state
    }

    pub fn score_progression_scenario(seed: Option<u64>) -> Self {
        let mut state = Self::empty_seeded(seed);
        state.script = ScriptedScenario::ScoreProgression;
        state.ufo_spawn_timer_seconds = f32::INFINITY;
        state
    }

    pub fn set_highscore_12345_scenario(seed: Option<u64>) -> Self {
        let mut state = Self::empty_seeded(seed);
        state.script = ScriptedScenario::SetHighScore12345;
        state.ufo_spawn_timer_seconds = f32::INFINITY;
        state
    }

    pub fn eight_extra_lives_scenario(seed: Option<u64>) -> Self {
        let mut state = Self::empty_seeded(seed);
        state.set_score_without_bonus(50_000);
        state.lives = 8;
        state.ufo_spawn_timer_seconds = f32::INFINITY;
        state
    }

    pub fn hyperspace_spam_scenario(seed: Option<u64>) -> Self {
        let mut state = Self::empty_seeded(seed);
        state.ufo_spawn_timer_seconds = f32::INFINITY;
        state
    }

    pub fn autonomous_play_10min_scenario(seed: Option<u64>) -> Self {
        let mut state = Self::new_seeded(seed);
        state.script = ScriptedScenario::AutonomousPlay10Min;
        state.invulnerability_timer_seconds = f32::INFINITY;
        state
    }

    fn empty_seeded(seed: Option<u64>) -> Self {
        Self {
            ship: ShipState::default(),
            alive: true,
            game_over: false,
            lives: INITIAL_LIVES,
            score: SCORE_PLACEHOLDER,
            asteroid_count: 0,
            round: 1,
            asteroids: Vec::new(),
            bullets: Vec::new(),
            ufo: None,
            ufo_bullets: Vec::new(),
            next_asteroid_id: 1,
            next_bullet_id: 1,
            next_ufo_id: 1,
            next_extra_life_score: EXTRA_LIFE_SCORE_INTERVAL,
            respawn_timer_seconds: 0.0,
            invulnerability_timer_seconds: 0.0,
            hyperspace_cooldown_timer_seconds: 0.0,
            hyperspace_was_down: false,
            ufo_spawn_timer_seconds: ufo_spawn_interval_seconds_for_score(SCORE_PLACEHOLDER),
            fire_was_down: false,
            events: Vec::new(),
            rng: rng_for_seed(seed),
            script: ScriptedScenario::None,
            script_tick: 0,
            script_phase: 0,
            script_wait_until_tick: 0,
        }
    }

    fn step(&mut self, input: &ControlState, dt: f32) {
        self.events.clear();
        self.update_scripted_scenario();
        self.update_respawn(dt);
        self.update_hyperspace_cooldown(dt);
        if self.alive {
            self.ship.integrate(input, dt);
        }
        self.update_hyperspace(input);
        for asteroid in &mut self.asteroids {
            asteroid.integrate(dt);
        }
        self.update_ufo(dt);
        self.update_fire(input);
        for bullet in &mut self.bullets {
            bullet.integrate(dt);
        }
        for bullet in &mut self.ufo_bullets {
            bullet.integrate(dt);
        }
        self.despawn_expired_bullets();
        self.despawn_expired_ufo_bullets();
        self.resolve_bullet_ufo_collisions();
        self.resolve_bullet_asteroid_collisions();
        self.resolve_ufo_bullet_asteroid_collisions();
        self.resolve_ship_asteroid_collisions();
        self.resolve_ship_ufo_collisions();
        self.resolve_ufo_bullet_ship_collisions();
        self.resolve_asteroid_ufo_collisions();
        if self.invulnerability_timer_seconds > 0.0 {
            self.invulnerability_timer_seconds = (self.invulnerability_timer_seconds - dt).max(0.0);
        }
        self.sync_asteroid_count();
    }

    fn update_scripted_scenario(&mut self) {
        match self.script {
            ScriptedScenario::None => {}
            ScriptedScenario::BulletHitAsteroidThreeTier => {
                match self.script_phase {
                    0 if self
                        .asteroids
                        .iter()
                        .any(|asteroid| asteroid.size == AsteroidSize::Medium) =>
                    {
                        self.script_phase = 1;
                        self.script_wait_until_tick = self.script_tick.saturating_add(4);
                    }
                    1 if self.script_tick >= self.script_wait_until_tick => {
                        if let Some(id) = self
                            .asteroids
                            .iter()
                            .find(|asteroid| asteroid.size == AsteroidSize::Medium)
                            .map(|asteroid| asteroid.id)
                        {
                            self.hit_asteroid_by_id(id);
                            self.script_phase = 2;
                        }
                    }
                    2 if self
                        .asteroids
                        .iter()
                        .any(|asteroid| asteroid.size == AsteroidSize::Small) =>
                    {
                        self.script_phase = 3;
                        self.script_wait_until_tick = self.script_tick.saturating_add(4);
                    }
                    3 if self.script_tick >= self.script_wait_until_tick => {
                        if let Some(id) = self
                            .asteroids
                            .iter()
                            .find(|asteroid| asteroid.size == AsteroidSize::Small)
                            .map(|asteroid| asteroid.id)
                        {
                            self.hit_asteroid_by_id(id);
                            self.script_phase = 4;
                        }
                    }
                    _ => {}
                }
                self.script_tick = self.script_tick.saturating_add(1);
            }
            ScriptedScenario::AutonomousPlay10Min => {
                if self.asteroids.is_empty() {
                    self.start_round(self.round.saturating_add(1));
                }
                if self.script_tick.is_multiple_of(20)
                    && let Some(id) = self.asteroids.first().map(|asteroid| asteroid.id)
                {
                    self.hit_asteroid_by_id(id);
                }
                self.script_tick = self.script_tick.saturating_add(1);
            }
            ScriptedScenario::ScoreProgression => {
                if matches!(self.script_tick, 0 | 1) {
                    self.add_score(EXTRA_LIFE_SCORE_INTERVAL);
                }
                self.script_tick = self.script_tick.saturating_add(1);
            }
            ScriptedScenario::SetHighScore12345 => {
                if self.script_tick == 0 {
                    self.add_score(12_345);
                }
                self.script_tick = self.script_tick.saturating_add(1);
            }
        }
    }

    pub fn snapshot(&self) -> GameSnapshot {
        GameSnapshot::with_game_over(self.asteroid_count, self.alive, self.score, self.game_over)
    }

    pub fn start_round(&mut self, round: u32) {
        self.round = round.max(1);
        self.asteroids.clear();
        let count = asteroid_spawn_count_for_round(self.round);
        self.asteroids.reserve(count as usize);
        for index in 0..count {
            let asteroid = self.spawn_large_asteroid(index);
            self.asteroids.push(asteroid);
        }
        self.sync_asteroid_count();
    }

    pub fn hit_asteroid_by_id(&mut self, id: u32) -> bool {
        self.hit_asteroid_by_id_with_score(id, true)
    }

    fn hit_asteroid_by_id_with_score(&mut self, id: u32, award_score: bool) -> bool {
        let Some(index) = self.asteroids.iter().position(|asteroid| asteroid.id == id) else {
            return false;
        };
        let parent = self.asteroids.remove(index);
        self.push_asteroid_event(GameEventKind::BulletHitAsteroid, parent.size);
        if award_score {
            self.add_score(parent.size.score_value());
        }
        if let Some(child_size) = parent.size.next_smaller() {
            for child_index in 0..2 {
                let velocity = split_child_velocity(parent.velocity, child_index);
                let hull = AsteroidHull::random(&mut self.rng);
                let child = self.allocate_asteroid(child_size, parent.position, velocity, hull);
                self.asteroids.push(child);
            }
            self.push_asteroid_event(GameEventKind::AsteroidSplit, parent.size);
        } else {
            self.push_asteroid_event(GameEventKind::AsteroidDestroyed, parent.size);
        }
        self.sync_asteroid_count();
        true
    }

    pub fn asteroid_size_counts(&self) -> AsteroidSizeCounts {
        let mut counts = AsteroidSizeCounts::default();
        for asteroid in &self.asteroids {
            match asteroid.size {
                AsteroidSize::Large => counts.large += 1,
                AsteroidSize::Medium => counts.medium += 1,
                AsteroidSize::Small => counts.small += 1,
            }
        }
        counts
    }

    pub fn render_asteroids(&self) -> Vec<RenderAsteroid> {
        self.asteroids
            .iter()
            .map(|asteroid| RenderAsteroid {
                id: asteroid.id,
                size: asteroid.size,
                position: asteroid.position,
                radius: asteroid.radius_ndc(),
                hull: asteroid.hull,
            })
            .collect()
    }

    pub fn render_bullets(&self) -> Vec<RenderBullet> {
        self.bullets
            .iter()
            .map(|bullet| RenderBullet {
                id: bullet.id,
                position: bullet.position,
                radius: BULLET_RADIUS_NDC,
            })
            .collect()
    }

    pub fn render_ufo(&self) -> Option<RenderUfo> {
        self.ufo.map(|ufo| RenderUfo {
            id: ufo.id,
            variant: ufo.variant,
            position: ufo.position,
            radius: ufo.radius_ndc(),
        })
    }

    pub fn render_ufo_bullets(&self) -> Vec<RenderBullet> {
        self.ufo_bullets
            .iter()
            .map(|bullet| RenderBullet {
                id: bullet.id,
                position: bullet.position,
                radius: BULLET_RADIUS_NDC,
            })
            .collect()
    }

    pub fn any_asteroid_wrapped_last_tick(&self) -> bool {
        self.asteroids
            .iter()
            .any(|asteroid| asteroid.wrapped_last_tick)
    }

    pub fn any_bullet_wrapped_last_tick(&self) -> bool {
        self.bullets.iter().any(|bullet| bullet.wrapped_last_tick)
    }

    pub fn any_ufo_bullet_wrapped_last_tick(&self) -> bool {
        self.ufo_bullets
            .iter()
            .any(|bullet| bullet.wrapped_last_tick)
    }

    pub fn events(&self) -> &[GameEvent] {
        &self.events
    }

    fn update_respawn(&mut self, dt: f32) {
        if self.game_over || self.alive || self.lives == 0 {
            return;
        }
        self.respawn_timer_seconds = (self.respawn_timer_seconds - dt).max(0.0);
        if self.respawn_timer_seconds <= 0.0 {
            self.ship = ShipState::default();
            self.alive = true;
            self.invulnerability_timer_seconds = SHIP_RESPAWN_INVULNERABILITY_SECONDS;
            self.push_event(GameEventKind::Respawn);
        }
    }

    fn update_fire(&mut self, input: &ControlState) {
        let fire_pressed = input.fire && !self.fire_was_down;
        self.fire_was_down = input.fire;
        if self.alive && !self.game_over && fire_pressed {
            self.fire_bullet();
        }
    }

    fn update_hyperspace_cooldown(&mut self, dt: f32) {
        if self.hyperspace_cooldown_timer_seconds > 0.0 {
            self.hyperspace_cooldown_timer_seconds =
                (self.hyperspace_cooldown_timer_seconds - dt).max(0.0);
        }
    }

    fn update_hyperspace(&mut self, input: &ControlState) {
        let hyperspace_pressed = input.hyperspace && !self.hyperspace_was_down;
        self.hyperspace_was_down = input.hyperspace;
        if !hyperspace_pressed || !self.alive || self.game_over {
            return;
        }
        if self.hyperspace_cooldown_timer_seconds > 0.0 {
            self.push_event(GameEventKind::HyperspaceCooldownRejected);
            return;
        }

        self.hyperspace_cooldown_timer_seconds = HYPERSPACE_COOLDOWN_SECONDS;
        self.ship.position = random_hyperspace_target(&mut self.rng);
        self.push_event(GameEventKind::HyperspaceTriggered);
        if hyperspace_self_destructs(&mut self.rng) {
            self.push_event(GameEventKind::HyperspaceSelfDestruct);
            self.kill_ship();
        }
    }

    fn fire_bullet(&mut self) {
        let (angle_sin, angle_cos) = self.ship.angle.sin_cos();
        let forward = Vec2::new(angle_cos, angle_sin);
        let id = self.allocate_bullet_id();
        let position = wrap_position(
            self.ship.position + forward * (SHIP_COLLISION_RADIUS_NDC + BULLET_RADIUS_NDC),
        );
        let velocity = self.ship.velocity + forward * BULLET_SPEED_NDC_PER_SEC;
        self.bullets.push(Bullet::new(id, position, velocity));
        self.push_event(GameEventKind::BulletFired);
    }

    fn update_ufo(&mut self, dt: f32) {
        if self.ufo.is_some() {
            self.update_active_ufo(dt);
        } else {
            self.update_ufo_spawn_timer(dt);
        }
    }

    fn update_ufo_spawn_timer(&mut self, dt: f32) {
        if !self.alive || self.game_over {
            return;
        }
        self.ufo_spawn_timer_seconds -= dt;
        if self.ufo_spawn_timer_seconds <= 0.0 {
            self.spawn_ufo();
        }
    }

    fn update_active_ufo(&mut self, dt: f32) {
        let mut shot_request = None;
        let mut should_despawn = false;
        if let Some(ufo) = self.ufo.as_mut() {
            ufo.integrate(dt);
            ufo.direction_timer_seconds -= dt;
            if ufo.direction_timer_seconds <= 0.0 {
                ufo.velocity.y = random_ufo_vertical_velocity(&mut self.rng);
                ufo.direction_timer_seconds += UFO_DIRECTION_CHANGE_SECONDS;
            }
            ufo.shot_timer_seconds -= dt;
            if ufo.shot_timer_seconds <= 0.0 {
                shot_request = Some((ufo.variant, ufo.position, ufo.radius_ndc()));
                ufo.shot_timer_seconds += ufo_shot_interval_seconds();
            }
            should_despawn = ufo.is_offscreen();
        }

        if let Some((variant, position, radius)) = shot_request {
            self.fire_ufo_bullet(variant, position, radius);
        }
        if should_despawn {
            self.clear_ufo(GameEventKind::UfoDespawned, false);
        }
    }

    fn spawn_ufo(&mut self) {
        let variant = ufo_variant_for_score(self.score);
        let from_left = self.rng.next_f32() < 0.5;
        let x = if from_left {
            PLAYFIELD_MIN.x - UFO_EDGE_MARGIN_NDC
        } else {
            PLAYFIELD_MAX.x + UFO_EDGE_MARGIN_NDC
        };
        let horizontal_sign = if from_left { 1.0 } else { -1.0 };
        let y = -0.78 + self.rng.next_f32() * 1.56;
        let velocity = Vec2::new(
            horizontal_sign
                * UFO_RAW_HORIZONTAL_SPEED
                * tuning::ASTEROID_RAW_VELOCITY_TO_NDC_PER_SEC,
            random_ufo_vertical_velocity(&mut self.rng),
        );
        let id = self.next_ufo_id;
        self.next_ufo_id = self.next_ufo_id.wrapping_add(1).max(1);
        self.ufo = Some(Ufo::new(id, variant, Vec2::new(x, y), velocity));
        if self.score >= UFO_SMALL_SCORE_THRESHOLD {
            self.push_event(GameEventKind::ScoreGte10000);
        }
        self.push_ufo_event(GameEventKind::UfoSpawned, variant);
        self.push_ufo_event(GameEventKind::UfoSirenOn, variant);
    }

    fn fire_ufo_bullet(&mut self, variant: UfoVariant, ufo_position: Vec2, ufo_radius: f32) {
        let direction = match variant {
            UfoVariant::Large => random_direction(&mut self.rng),
            UfoVariant::Small => {
                wrapped_delta(self.ship.position, ufo_position).normalized_or(Vec2::X)
            }
        };
        let id = self.allocate_bullet_id();
        let position = wrap_position(ufo_position + direction * (ufo_radius + BULLET_RADIUS_NDC));
        let velocity = direction * UFO_BULLET_SPEED_NDC_PER_SEC;
        self.ufo_bullets.push(Bullet::new(id, position, velocity));
        self.push_ufo_event(
            match variant {
                UfoVariant::Large => GameEventKind::UfoFiredRandom,
                UfoVariant::Small => GameEventKind::UfoFiredAimed,
            },
            variant,
        );
    }

    fn despawn_expired_bullets(&mut self) {
        let before = self.bullets.len();
        self.bullets.retain(|bullet| !bullet.is_expired());
        for _ in self.bullets.len()..before {
            self.push_event(GameEventKind::BulletExpired);
        }
    }

    fn despawn_expired_ufo_bullets(&mut self) {
        self.ufo_bullets.retain(|bullet| !bullet.is_expired());
    }

    fn resolve_bullet_ufo_collisions(&mut self) {
        let Some(ufo) = self.ufo else {
            return;
        };
        let hit_bullet_index = self.bullets.iter().position(|bullet| {
            playfield_circles_overlap(
                bullet.position,
                BULLET_RADIUS_NDC,
                ufo.position,
                ufo.radius_ndc(),
            )
        });
        if let Some(index) = hit_bullet_index {
            self.bullets.remove(index);
            self.clear_ufo(GameEventKind::UfoDestroyed, true);
        }
    }

    fn resolve_bullet_asteroid_collisions(&mut self) {
        let mut bullet_index = 0;
        while bullet_index < self.bullets.len() {
            let bullet = self.bullets[bullet_index];
            let hit_asteroid_id = self
                .asteroids
                .iter()
                .find(|asteroid| {
                    playfield_circles_overlap(
                        bullet.position,
                        BULLET_RADIUS_NDC,
                        asteroid.position,
                        asteroid.radius_ndc(),
                    )
                })
                .map(|asteroid| asteroid.id);

            if let Some(asteroid_id) = hit_asteroid_id {
                self.bullets.remove(bullet_index);
                self.hit_asteroid_by_id(asteroid_id);
            } else {
                bullet_index += 1;
            }
        }
    }

    fn resolve_ufo_bullet_asteroid_collisions(&mut self) {
        let mut bullet_index = 0;
        while bullet_index < self.ufo_bullets.len() {
            let bullet = self.ufo_bullets[bullet_index];
            let hit_asteroid_id = self
                .asteroids
                .iter()
                .find(|asteroid| {
                    playfield_circles_overlap(
                        bullet.position,
                        BULLET_RADIUS_NDC,
                        asteroid.position,
                        asteroid.radius_ndc(),
                    )
                })
                .map(|asteroid| asteroid.id);

            if let Some(asteroid_id) = hit_asteroid_id {
                self.ufo_bullets.remove(bullet_index);
                self.hit_asteroid_by_id_with_score(asteroid_id, false);
            } else {
                bullet_index += 1;
            }
        }
    }

    fn resolve_ship_asteroid_collisions(&mut self) {
        if !self.alive || self.game_over || self.invulnerability_timer_seconds > 0.0 {
            return;
        }
        let hit_ship = self.asteroids.iter().any(|asteroid| {
            playfield_circles_overlap(
                self.ship.position,
                SHIP_COLLISION_RADIUS_NDC,
                asteroid.position,
                asteroid.radius_ndc(),
            )
        });
        if hit_ship {
            self.kill_ship();
        }
    }

    fn resolve_ship_ufo_collisions(&mut self) {
        if !self.alive || self.game_over || self.invulnerability_timer_seconds > 0.0 {
            return;
        }
        let hit_ship = self.ufo.is_some_and(|ufo| {
            playfield_circles_overlap(
                self.ship.position,
                SHIP_COLLISION_RADIUS_NDC,
                ufo.position,
                ufo.radius_ndc(),
            )
        });
        if hit_ship {
            self.kill_ship();
            self.clear_ufo(GameEventKind::UfoDestroyed, false);
        }
    }

    fn resolve_ufo_bullet_ship_collisions(&mut self) {
        if !self.alive || self.game_over || self.invulnerability_timer_seconds > 0.0 {
            return;
        }
        let hit_bullet_index = self.ufo_bullets.iter().position(|bullet| {
            playfield_circles_overlap(
                self.ship.position,
                SHIP_COLLISION_RADIUS_NDC,
                bullet.position,
                BULLET_RADIUS_NDC,
            )
        });
        if let Some(index) = hit_bullet_index {
            self.ufo_bullets.remove(index);
            self.kill_ship();
        }
    }

    fn resolve_asteroid_ufo_collisions(&mut self) {
        let Some(ufo) = self.ufo else {
            return;
        };
        let hit_asteroid_id = self
            .asteroids
            .iter()
            .find(|asteroid| {
                playfield_circles_overlap(
                    ufo.position,
                    ufo.radius_ndc(),
                    asteroid.position,
                    asteroid.radius_ndc(),
                )
            })
            .map(|asteroid| asteroid.id);
        if let Some(asteroid_id) = hit_asteroid_id {
            self.hit_asteroid_by_id_with_score(asteroid_id, false);
            self.clear_ufo(GameEventKind::UfoDestroyed, false);
        }
    }

    fn clear_ufo(&mut self, kind: GameEventKind, award_score: bool) {
        let Some(ufo) = self.ufo.take() else {
            return;
        };
        if award_score {
            self.add_score(ufo.variant.score_value());
        }
        self.push_ufo_event(kind, ufo.variant);
        self.push_ufo_event(GameEventKind::UfoSirenOff, ufo.variant);
        self.reset_ufo_spawn_timer();
    }

    fn reset_ufo_spawn_timer(&mut self) {
        self.ufo_spawn_timer_seconds = ufo_spawn_interval_seconds_for_score(self.score);
    }

    fn set_score_without_bonus(&mut self, score: u32) {
        self.score = score;
        self.next_extra_life_score = next_extra_life_score_after(score);
    }

    fn add_score(&mut self, delta: u32) {
        if delta == 0 {
            return;
        }
        self.score = self.score.saturating_add(delta);
        self.events.push(GameEvent::score(delta));

        let mut threshold = self.next_extra_life_score;
        while threshold <= self.score {
            self.lives = self.lives.saturating_add(1);
            self.events.push(GameEvent::extra_life(threshold));
            let Some(next_threshold) = threshold.checked_add(EXTRA_LIFE_SCORE_INTERVAL) else {
                threshold = u32::MAX;
                break;
            };
            threshold = next_threshold;
        }
        self.next_extra_life_score = threshold;
    }

    fn kill_ship(&mut self) {
        if !self.alive || self.game_over {
            return;
        }
        self.alive = false;
        self.lives = self.lives.saturating_sub(1);
        self.respawn_timer_seconds = if self.lives > 0 {
            SHIP_RESPAWN_DELAY_SECONDS
        } else {
            0.0
        };
        self.invulnerability_timer_seconds = 0.0;
        self.push_event(GameEventKind::ShipDied);
        self.push_event(GameEventKind::LivesDecremented);
        if self.lives == 0 {
            self.game_over = true;
            self.push_event(GameEventKind::GameOver);
        }
    }

    fn push_event(&mut self, kind: GameEventKind) {
        self.events.push(GameEvent::new(kind));
    }

    fn push_asteroid_event(&mut self, kind: GameEventKind, asteroid_size: AsteroidSize) {
        self.events.push(GameEvent::asteroid(kind, asteroid_size));
    }

    fn push_ufo_event(&mut self, kind: GameEventKind, ufo_variant: UfoVariant) {
        self.events.push(GameEvent::ufo(kind, ufo_variant));
    }

    fn spawn_large_asteroid(&mut self, index: u32) -> Asteroid {
        let side = index % 4;
        let position = edge_spawn_position(side, &mut self.rng);
        let velocity = large_asteroid_spawn_velocity(side, index, &mut self.rng);
        let hull = AsteroidHull::random(&mut self.rng);
        self.allocate_asteroid(AsteroidSize::Large, position, velocity, hull)
    }

    fn allocate_asteroid(
        &mut self,
        size: AsteroidSize,
        position: Vec2,
        velocity: Vec2,
        hull: AsteroidHull,
    ) -> Asteroid {
        let id = self.next_asteroid_id;
        self.next_asteroid_id = self.next_asteroid_id.wrapping_add(1).max(1);
        Asteroid::new(id, size, position, velocity, hull)
    }

    fn allocate_bullet_id(&mut self) -> u32 {
        let id = self.next_bullet_id;
        self.next_bullet_id = self.next_bullet_id.wrapping_add(1).max(1);
        id
    }

    fn sync_asteroid_count(&mut self) {
        self.asteroid_count = self.asteroids.len() as u32;
    }
}

/// Original wave initialization bookmarks:
/// - Norbert Kehrer's Asteroids static binary translation / disassembly-derived
///   exact port:
///   https://norbertkehrer.github.io/ast_js/AsteroidsJS.html
/// - Computer Archeology annotated listing:
///   https://computerarcheology.com/Arcade/Asteroids/Code.html#7187
/// - Computer Archeology RAM map:
///   https://computerarcheology.com/Arcade/Asteroids/RAMUse.html#asteroidsPerWave
/// - 6502disassembly SourceGen listing:
///   https://6502disassembly.com/va-asteroids/Asteroids.html#SymInitAstPerWave
///
/// The original reset path seeds AstPerWave with 2 at $6eda. InitAstPerWave
/// then adds 2, stores the result in CurAsteroids and AstPerWave, and caps the
/// initial rocks-per-wave at 11.
pub fn asteroid_spawn_count_for_round(round: u32) -> u32 {
    if round == 0 {
        return 0;
    }
    ASTEROIDS_PER_WAVE_BOOTSTRAP
        .saturating_add(ASTEROIDS_PER_WAVE_INCREMENT.saturating_mul(round))
        .min(ASTEROIDS_PER_WAVE_MAX)
}

pub fn ufo_variant_for_score(score: u32) -> UfoVariant {
    if score >= UFO_SMALL_SCORE_THRESHOLD {
        UfoVariant::Small
    } else {
        UfoVariant::Large
    }
}

/// UFO spawn-rate curve for this score-driven step-14 slice.
///
/// The original saucer code is documented in Norbert Kehrer's Asteroids
/// disassembly and the Computer Archeology annotated listing:
/// https://computerarcheology.com/Arcade/Asteroids/Code.html#6B93
/// https://6502disassembly.com/va-asteroids/Asteroids.html#SymUpdateScr
///
/// Relevant original values:
/// - new-game `ScrTmrReload` is `#$92` at $68f8,
/// - saucer logic runs only every 4th 60 Hz frame at $6b93-$6bb7,
/// - each saucer appearance subtracts `#$06` at $6bd0-$6bda,
/// - reload bottoms out at `#$20`, so the interval is 146..32 saucer ticks.
///
/// This build keeps those exact reload values and the 15 Hz saucer tick cadence,
/// but indexes the curve by score in 2,500-point steps so the task's
/// score-driven spawn-rate requirement is deterministic and testable.
pub fn ufo_spawn_reload_ticks_for_score(score: u32) -> u32 {
    let speedup_steps = score / UFO_SPAWN_SCORE_STEP_POINTS;
    UFO_SPAWN_RELOAD_INITIAL_TICKS
        .saturating_sub(UFO_SPAWN_RELOAD_DECREMENT_TICKS.saturating_mul(speedup_steps))
        .max(UFO_SPAWN_RELOAD_MIN_TICKS)
}

pub fn ufo_spawn_interval_seconds_for_score(score: u32) -> f32 {
    ufo_spawn_reload_ticks_for_score(score) as f32 * UFO_ORIGINAL_TIMER_TICK_SECONDS
}

pub fn displayed_lives(lives: u32) -> u32 {
    lives.min(MAX_DISPLAYED_LIVES)
}

fn next_extra_life_score_after(score: u32) -> u32 {
    let completed_intervals = score / EXTRA_LIFE_SCORE_INTERVAL;
    completed_intervals
        .saturating_add(1)
        .saturating_mul(EXTRA_LIFE_SCORE_INTERVAL)
}

fn ufo_shot_interval_seconds() -> f32 {
    UFO_SHOT_RELOAD_TICKS as f32 * UFO_ORIGINAL_TIMER_TICK_SECONDS
}

/// Closed DESIGN collision-model question: gameplay collisions use circle-vs-circle
/// tests with sprite-extent radii. This is modern, fast, deterministic at edge
/// touch, and accurate enough for the wireframe sprites used in this build.
pub fn circles_overlap(center_a: Vec2, radius_a: f32, center_b: Vec2, radius_b: f32) -> bool {
    let combined_radius = radius_a.max(0.0) + radius_b.max(0.0);
    (center_a - center_b).length_squared() <= combined_radius * combined_radius
}

fn playfield_circles_overlap(center_a: Vec2, radius_a: f32, center_b: Vec2, radius_b: f32) -> bool {
    let delta = wrapped_delta(center_a, center_b);
    let combined_radius = radius_a.max(0.0) + radius_b.max(0.0);
    delta.length_squared() <= combined_radius * combined_radius
}

fn wrapped_delta(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new(
        wrapped_axis_delta(a.x - b.x, PLAYFIELD_MIN.x, PLAYFIELD_MAX.x),
        wrapped_axis_delta(a.y - b.y, PLAYFIELD_MIN.y, PLAYFIELD_MAX.y),
    )
}

fn wrapped_axis_delta(delta: f32, min: f32, max: f32) -> f32 {
    let width = max - min;
    if width <= 0.0 {
        return delta;
    }
    if delta.abs() > width * 0.5 {
        delta - delta.signum() * width
    } else {
        delta
    }
}

fn edge_spawn_position(side: u32, rng: &mut SeededRng) -> Vec2 {
    let edge = 0.999;
    let coordinate = -0.86 + rng.next_f32() * 1.72;
    match side {
        0 => Vec2::new(edge, coordinate),
        1 => Vec2::new(-edge, coordinate),
        2 => Vec2::new(coordinate, edge),
        _ => Vec2::new(coordinate, -edge),
    }
}

fn edge_spawn_velocity(side: u32, index: u32, rng: &mut SeededRng) -> Vec2 {
    let primary_raw = if index == 0 {
        tuning::ASTEROID_RAW_VELOCITY_MAX
    } else {
        random_raw_velocity_magnitude(rng)
    };
    let secondary_raw = signed_random_raw_velocity(rng);
    let raw = match side {
        0 => Vec2::new(primary_raw, secondary_raw),
        1 => Vec2::new(-primary_raw, secondary_raw),
        2 => Vec2::new(secondary_raw, primary_raw),
        _ => Vec2::new(secondary_raw, -primary_raw),
    };
    raw * tuning::ASTEROID_RAW_VELOCITY_TO_DRIFT_NDC_PER_SEC
}

fn large_asteroid_spawn_velocity(side: u32, index: u32, rng: &mut SeededRng) -> Vec2 {
    edge_spawn_velocity(side, index, rng) * tuning::ASTEROID_LARGE_DRIFT_SPEED_SCALE
}

fn signed_random_raw_velocity(rng: &mut SeededRng) -> f32 {
    let sign = if rng.next_f32() < 0.5 { -1.0 } else { 1.0 };
    sign * random_raw_velocity_magnitude(rng)
}

fn random_raw_velocity_magnitude(rng: &mut SeededRng) -> f32 {
    tuning::ASTEROID_RAW_VELOCITY_MIN
        + rng.next_f32() * (tuning::ASTEROID_RAW_VELOCITY_MAX - tuning::ASTEROID_RAW_VELOCITY_MIN)
}

fn split_child_velocity(parent_velocity: Vec2, child_index: usize) -> Vec2 {
    let base_speed = parent_velocity
        .length()
        .max(tuning::ASTEROID_DRIFT_SPEED_MIN_NDC_PER_SEC);
    let parent_direction = parent_velocity.normalized_or(Vec2::X);
    let spread: f32 = if child_index == 0 { 0.85 } else { -0.85 };
    let (sin, cos) = spread.sin_cos();
    let direction = Vec2::new(
        parent_direction.x * cos - parent_direction.y * sin,
        parent_direction.x * sin + parent_direction.y * cos,
    );
    direction * (base_speed * 1.08).min(tuning::ASTEROID_DRIFT_SPEED_MAX_NDC_PER_SEC)
}

fn random_ufo_vertical_velocity(rng: &mut SeededRng) -> f32 {
    let index = (rng.next_u64() as usize) & 0x03;
    UFO_RAW_VERTICAL_SPEEDS[index] * tuning::ASTEROID_RAW_VELOCITY_TO_NDC_PER_SEC
}

fn random_direction(rng: &mut SeededRng) -> Vec2 {
    let angle = rng.next_f32() * TAU;
    let (sin, cos) = angle.sin_cos();
    Vec2::new(cos, sin)
}

fn random_hyperspace_target(rng: &mut SeededRng) -> Vec2 {
    Vec2::new(
        random_range(rng, PLAYFIELD_MIN.x, PLAYFIELD_MAX.x),
        random_range(rng, PLAYFIELD_MIN.y, PLAYFIELD_MAX.y),
    )
}

fn random_range(rng: &mut SeededRng, min: f32, max: f32) -> f32 {
    min + rng.next_f32() * (max - min)
}

fn hyperspace_self_destructs(rng: &mut SeededRng) -> bool {
    rng.next_f32() < HYPERSPACE_SELF_DESTRUCT_CHANCE
}

impl ShipState {
    fn integrate(&mut self, input: &ControlState, dt: f32) {
        let rotation_direction = bool_axis(input.rotate_left) - bool_axis(input.rotate_right);
        self.angle = (self.angle
            + rotation_direction * tuning::SHIP_ROTATION_RATE_RAD_PER_SEC * dt)
            .rem_euclid(TAU);

        if input.thrust {
            let (angle_sin, angle_cos) = self.angle.sin_cos();
            self.velocity = self.velocity
                + Vec2::new(angle_cos, angle_sin) * (SHIP_THRUST_ACCEL_UNITS_PER_SEC_SQUARED * dt);
            self.velocity = clamp_velocity(self.velocity, SHIP_MAX_VELOCITY_UNITS_PER_SEC);
        } else {
            self.velocity = damp_ship_velocity(self.velocity, dt);
        }

        self.position = wrap_position(self.position + self.velocity * dt);
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderShip {
    pub position: Vec2,
    pub angle: f32,
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AdvanceReport {
    pub substeps: u32,
    pub dropped_accumulator_seconds: f32,
    pub interpolation_alpha: f32,
}

impl AdvanceReport {
    pub fn hit_spiral_guard(self) -> bool {
        self.dropped_accumulator_seconds > 0.0
    }
}

#[derive(Clone, Debug)]
pub struct GameLoop {
    previous: GameState,
    current: GameState,
    pending_events: Vec<GameEvent>,
    accumulator_seconds: f32,
    tick: u64,
    paused: bool,
    high_score: u32,
}

impl Default for GameLoop {
    fn default() -> Self {
        let state = GameState::default();
        Self {
            previous: state.clone(),
            current: state,
            pending_events: Vec::new(),
            accumulator_seconds: 0.0,
            tick: 0,
            paused: false,
            high_score: 0,
        }
    }
}

impl GameLoop {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_with_high_score(high_score: u32) -> Self {
        Self::new_seeded_with_high_score(None, high_score)
    }

    pub fn new_seeded(seed: Option<u64>) -> Self {
        Self::new_seeded_with_high_score(seed, 0)
    }

    pub fn new_seeded_with_high_score(seed: Option<u64>, high_score: u32) -> Self {
        let state = GameState::new_seeded(seed);
        Self {
            previous: state.clone(),
            current: state,
            pending_events: Vec::new(),
            accumulator_seconds: 0.0,
            tick: 0,
            paused: false,
            high_score,
        }
    }

    pub fn from_state(state: GameState) -> Self {
        Self::from_state_with_high_score(state, 0)
    }

    pub fn from_state_with_high_score(state: GameState, high_score: u32) -> Self {
        Self {
            previous: state.clone(),
            current: state,
            pending_events: Vec::new(),
            accumulator_seconds: 0.0,
            tick: 0,
            paused: false,
            high_score,
        }
    }

    pub fn advance<F>(
        &mut self,
        frame_dt_seconds: f32,
        input: &ControlState,
        mut on_tick: F,
    ) -> AdvanceReport
    where
        F: FnMut(GameSnapshot),
    {
        self.pending_events.clear();
        if self.paused {
            return AdvanceReport {
                substeps: 0,
                dropped_accumulator_seconds: 0.0,
                interpolation_alpha: self.interpolation_alpha(),
            };
        }

        let frame_dt_seconds = sanitize_frame_dt(frame_dt_seconds);
        self.accumulator_seconds += frame_dt_seconds;

        let mut substeps = 0;
        while self.accumulator_seconds + f32::EPSILON >= FIXED_TIMESTEP_SECONDS
            && substeps < MAX_SUBSTEPS_PER_FRAME
        {
            self.previous = self.current.clone();
            self.current.step(input, FIXED_TIMESTEP_SECONDS);
            if self.current.score > self.high_score {
                self.high_score = self.current.score;
                self.current
                    .events
                    .push(GameEvent::high_score(self.high_score));
            }
            let instant_ship_reposition = self
                .current
                .events()
                .iter()
                .any(|event| event.kind == GameEventKind::HyperspaceTriggered);
            self.pending_events.extend_from_slice(self.current.events());
            if instant_ship_reposition {
                self.previous = self.current.clone();
            }
            self.tick += 1;
            substeps += 1;
            self.accumulator_seconds -= FIXED_TIMESTEP_SECONDS;
            on_tick(self.current.snapshot());
        }

        let dropped_accumulator_seconds =
            if self.accumulator_seconds + f32::EPSILON >= FIXED_TIMESTEP_SECONDS {
                let dropped = self.accumulator_seconds;
                self.accumulator_seconds = 0.0;
                dropped
            } else {
                0.0
            };

        AdvanceReport {
            substeps,
            dropped_accumulator_seconds,
            interpolation_alpha: self.interpolation_alpha(),
        }
    }

    pub fn drain_events(&mut self) -> Vec<GameEvent> {
        std::mem::take(&mut self.pending_events)
    }

    pub fn interpolated_ship(&self) -> RenderShip {
        let alpha = self.interpolation_alpha();
        RenderShip {
            position: Vec2::new(
                lerp_wrapped_coordinate(
                    self.previous.ship.position.x,
                    self.current.ship.position.x,
                    alpha,
                    PLAYFIELD_MIN.x,
                    PLAYFIELD_MAX.x,
                ),
                lerp_wrapped_coordinate(
                    self.previous.ship.position.y,
                    self.current.ship.position.y,
                    alpha,
                    PLAYFIELD_MIN.y,
                    PLAYFIELD_MAX.y,
                ),
            ),
            angle: lerp_angle(self.previous.ship.angle, self.current.ship.angle, alpha),
            scale: tuning::SHIP_GAMEPLAY_SCALE,
        }
    }

    pub fn interpolated_ship_if_alive(&self) -> Option<RenderShip> {
        self.current.alive.then(|| self.interpolated_ship())
    }

    pub fn interpolated_asteroids(&self) -> Vec<RenderAsteroid> {
        let alpha = self.interpolation_alpha();
        self.current
            .asteroids
            .iter()
            .map(|current| {
                let position = self
                    .previous
                    .asteroids
                    .iter()
                    .find(|previous| previous.id == current.id)
                    .map(|previous| {
                        Vec2::new(
                            lerp_wrapped_coordinate(
                                previous.position.x,
                                current.position.x,
                                alpha,
                                PLAYFIELD_MIN.x,
                                PLAYFIELD_MAX.x,
                            ),
                            lerp_wrapped_coordinate(
                                previous.position.y,
                                current.position.y,
                                alpha,
                                PLAYFIELD_MIN.y,
                                PLAYFIELD_MAX.y,
                            ),
                        )
                    })
                    .unwrap_or(current.position);
                RenderAsteroid {
                    id: current.id,
                    size: current.size,
                    position,
                    radius: current.radius_ndc(),
                    hull: current.hull,
                }
            })
            .collect()
    }

    pub fn interpolated_bullets(&self) -> Vec<RenderBullet> {
        let alpha = self.interpolation_alpha();
        self.current
            .bullets
            .iter()
            .map(|current| {
                let position = self
                    .previous
                    .bullets
                    .iter()
                    .find(|previous| previous.id == current.id)
                    .map(|previous| {
                        Vec2::new(
                            lerp_wrapped_coordinate(
                                previous.position.x,
                                current.position.x,
                                alpha,
                                PLAYFIELD_MIN.x,
                                PLAYFIELD_MAX.x,
                            ),
                            lerp_wrapped_coordinate(
                                previous.position.y,
                                current.position.y,
                                alpha,
                                PLAYFIELD_MIN.y,
                                PLAYFIELD_MAX.y,
                            ),
                        )
                    })
                    .unwrap_or(current.position);
                RenderBullet {
                    id: current.id,
                    position,
                    radius: BULLET_RADIUS_NDC,
                }
            })
            .collect()
    }

    pub fn interpolated_ufo(&self) -> Option<RenderUfo> {
        let current = self.current.ufo?;
        let position = self
            .previous
            .ufo
            .filter(|previous| previous.id == current.id)
            .map(|previous| {
                previous.position
                    + (current.position - previous.position) * self.interpolation_alpha()
            })
            .unwrap_or(current.position);
        Some(RenderUfo {
            id: current.id,
            variant: current.variant,
            position,
            radius: current.radius_ndc(),
        })
    }

    pub fn interpolated_ufo_bullets(&self) -> Vec<RenderBullet> {
        let alpha = self.interpolation_alpha();
        self.current
            .ufo_bullets
            .iter()
            .map(|current| {
                let position = self
                    .previous
                    .ufo_bullets
                    .iter()
                    .find(|previous| previous.id == current.id)
                    .map(|previous| {
                        Vec2::new(
                            lerp_wrapped_coordinate(
                                previous.position.x,
                                current.position.x,
                                alpha,
                                PLAYFIELD_MIN.x,
                                PLAYFIELD_MAX.x,
                            ),
                            lerp_wrapped_coordinate(
                                previous.position.y,
                                current.position.y,
                                alpha,
                                PLAYFIELD_MIN.y,
                                PLAYFIELD_MAX.y,
                            ),
                        )
                    })
                    .unwrap_or(current.position);
                RenderBullet {
                    id: current.id,
                    position,
                    radius: BULLET_RADIUS_NDC,
                }
            })
            .collect()
    }

    pub fn render_time_seconds(&self) -> f32 {
        self.tick as f32 * FIXED_TIMESTEP_SECONDS + self.accumulator_seconds
    }

    pub fn current(&self) -> &GameState {
        &self.current
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn high_score(&self) -> u32 {
        self.high_score
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub fn toggle_paused(&mut self) {
        self.paused = !self.paused;
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    fn interpolation_alpha(&self) -> f32 {
        (self.accumulator_seconds / FIXED_TIMESTEP_SECONDS).clamp(0.0, 1.0)
    }
}

pub fn heavy_input_controls(time_seconds: f32) -> ControlState {
    let phase = (time_seconds * 7.0).floor() as i32;
    ControlState {
        rotate_left: phase.rem_euclid(2) == 0,
        rotate_right: phase.rem_euclid(2) != 0,
        thrust: true,
        fire: (time_seconds * 12.0).floor() as i32 % 3 == 0,
        hyperspace: false,
    }
}

fn sanitize_frame_dt(dt: f32) -> f32 {
    if dt.is_finite() && dt > 0.0 { dt } else { 0.0 }
}

fn bool_axis(value: bool) -> f32 {
    if value { 1.0 } else { 0.0 }
}

fn clamp_velocity(velocity: Vec2, max_speed: f32) -> Vec2 {
    let speed = velocity.length();
    if speed > max_speed {
        velocity * (max_speed / speed)
    } else {
        velocity
    }
}

fn damp_ship_velocity(velocity: Vec2, dt: f32) -> Vec2 {
    let stop_speed_squared =
        SHIP_IDLE_STOP_SPEED_UNITS_PER_SEC * SHIP_IDLE_STOP_SPEED_UNITS_PER_SEC;
    if velocity.length_squared() <= stop_speed_squared {
        return Vec2::ZERO;
    }

    let damping = (-SHIP_IDLE_DRAG_PER_SEC * dt).exp();
    let velocity = velocity * damping;
    if velocity.length_squared() <= stop_speed_squared {
        Vec2::ZERO
    } else {
        velocity
    }
}

fn wrap_position(position: Vec2) -> Vec2 {
    wrap_position_with_report(position).0
}

fn wrap_position_with_report(position: Vec2) -> (Vec2, bool) {
    let (x, wrapped_x) = wrap_coordinate_with_report(position.x, PLAYFIELD_MIN.x, PLAYFIELD_MAX.x);
    let (y, wrapped_y) = wrap_coordinate_with_report(position.y, PLAYFIELD_MIN.y, PLAYFIELD_MAX.y);
    (Vec2::new(x, y), wrapped_x || wrapped_y)
}

fn wrap_coordinate(value: f32, min: f32, max: f32) -> f32 {
    wrap_coordinate_with_report(value, min, max).0
}

fn wrap_coordinate_with_report(value: f32, min: f32, max: f32) -> (f32, bool) {
    let width = max - min;
    if width <= 0.0 {
        return (min, false);
    }
    if value < min || value > max {
        ((value - min).rem_euclid(width) + min, true)
    } else {
        (value, false)
    }
}

fn lerp_wrapped_coordinate(start: f32, end: f32, alpha: f32, min: f32, max: f32) -> f32 {
    let width = max - min;
    let mut delta = end - start;
    if delta.abs() > width * 0.5 {
        delta -= delta.signum() * width;
    }
    wrap_coordinate(start + delta * alpha, min, max)
}

fn lerp_angle(start: f32, end: f32, alpha: f32) -> f32 {
    let delta = (end - start + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI;
    (start + delta * alpha).rem_euclid(TAU)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPSILON: f32 = 0.0001;

    #[test]
    fn spiral_of_death_guard_caps_substeps_at_four() {
        let mut game = GameLoop::new();
        let mut snapshots = 0;

        let report = game.advance(1.0, &ControlState::default(), |_| snapshots += 1);

        assert_eq!(report.substeps, MAX_SUBSTEPS_PER_FRAME);
        assert_eq!(snapshots, MAX_SUBSTEPS_PER_FRAME);
        assert_eq!(game.tick(), u64::from(MAX_SUBSTEPS_PER_FRAME));
        assert!(report.hit_spiral_guard());
        assert_eq!(game.accumulator_seconds, 0.0);
    }

    #[test]
    fn screen_wrap_moves_positions_across_playfield_edges() {
        let mut ship = ShipState {
            position: Vec2::new(0.99, -0.99),
            velocity: Vec2::new(2.0, -2.0),
            angle: 0.0,
        };

        ship.integrate(&ControlState::default(), 0.02);

        assert!(ship.position.x < -0.95, "x={}", ship.position.x);
        assert!(ship.position.y > 0.95, "y={}", ship.position.y);
    }

    #[test]
    fn rotation_rate_matches_design_constant_in_radians_per_second() {
        let mut ship = ShipState::default();
        let input = ControlState {
            rotate_left: true,
            ..ControlState::default()
        };

        ship.integrate(&input, 1.0);

        assert!((ship.angle - tuning::SHIP_ROTATION_RATE_RAD_PER_SEC).abs() < EPSILON);
    }

    #[test]
    fn gameplay_ship_uses_playable_scale_instead_of_demo_scale() {
        let game = GameLoop::new();
        let ship = game.interpolated_ship();

        assert_eq!(ship.scale, tuning::SHIP_GAMEPLAY_SCALE);
        assert!(ship.scale < tuning::SHIP_SPINNING_SCALE * 0.25);
        assert!((SHIP_COLLISION_RADIUS_NDC - 0.44 * tuning::SHIP_GAMEPLAY_SCALE).abs() < EPSILON);
    }

    #[test]
    fn thrust_acceleration_accumulates_at_scaled_sixty_hz_rate() {
        let mut ship = ShipState::default();
        let input = ControlState {
            thrust: true,
            ..ControlState::default()
        };

        for _ in 0..240 {
            ship.integrate(&input, FIXED_TIMESTEP_SECONDS);
        }

        assert!(
            (ship.velocity.length() - SHIP_THRUST_ACCEL_UNITS_PER_SEC_SQUARED).abs() < 0.001,
            "speed={}",
            ship.velocity.length()
        );
    }

    #[test]
    fn thrust_velocity_caps_at_documented_max_speed() {
        let mut ship = ShipState::default();
        let input = ControlState {
            thrust: true,
            ..ControlState::default()
        };

        for _ in 0..(240 * 10) {
            ship.integrate(&input, FIXED_TIMESTEP_SECONDS);
        }

        assert!(ship.velocity.length() <= SHIP_MAX_VELOCITY_UNITS_PER_SEC + 0.001);
        assert!(ship.velocity.length() > SHIP_MAX_VELOCITY_UNITS_PER_SEC - 0.001);
    }

    #[test]
    fn idle_ship_velocity_decays_when_thrust_is_released() {
        let mut ship = ShipState {
            position: Vec2::ZERO,
            velocity: Vec2::new(1.25, -0.75),
            angle: 0.0,
        };
        let initial_speed = ship.velocity.length();

        for _ in 0..240 {
            ship.integrate(&ControlState::default(), FIXED_TIMESTEP_SECONDS);
        }

        let expected_speed = initial_speed * (-SHIP_IDLE_DRAG_PER_SEC).exp();
        assert!((ship.velocity.length() - expected_speed).abs() < 0.001);
        assert!(ship.velocity.length() < initial_speed);
    }

    #[test]
    fn idle_ship_velocity_snaps_to_stop_after_slowing_down() {
        let mut ship = ShipState {
            position: Vec2::ZERO,
            velocity: Vec2::new(1.25, -0.75),
            angle: 0.0,
        };

        for _ in 0..(240 * 5) {
            ship.integrate(&ControlState::default(), FIXED_TIMESTEP_SECONDS);
        }

        assert_eq!(ship.velocity, Vec2::ZERO);
    }

    #[test]
    fn asteroid_split_rule_creates_two_next_smaller_rocks() {
        for (size, expected_child_size) in [
            (AsteroidSize::Large, Some(AsteroidSize::Medium)),
            (AsteroidSize::Medium, Some(AsteroidSize::Small)),
            (AsteroidSize::Small, None),
        ] {
            let mut state = state_with_one_asteroid(size);
            assert!(state.hit_asteroid_by_id(1));

            match expected_child_size {
                Some(child_size) => {
                    assert_eq!(state.asteroids.len(), 2);
                    assert!(
                        state
                            .asteroids
                            .iter()
                            .all(|asteroid| asteroid.size == child_size)
                    );
                    assert_eq!(state.asteroid_count, 2);
                }
                None => {
                    assert!(state.asteroids.is_empty());
                    assert_eq!(state.asteroid_count, 0);
                }
            }
        }
    }

    #[test]
    fn asteroid_kill_scores_match_disassembly_table() {
        for (size, expected_score) in [
            (AsteroidSize::Large, ASTEROID_LARGE_SCORE),
            (AsteroidSize::Medium, ASTEROID_MEDIUM_SCORE),
            (AsteroidSize::Small, ASTEROID_SMALL_SCORE),
        ] {
            let mut state = state_with_one_asteroid(size);

            assert!(state.hit_asteroid_by_id(1));

            assert_eq!(state.score, expected_score);
            assert!(state.events().iter().any(|event| {
                event.kind == GameEventKind::ScoreIncreased
                    && event.score_delta == Some(expected_score)
            }));
        }
    }

    #[test]
    fn ufo_kill_scores_match_disassembly_constants() {
        for (variant, expected_score) in [
            (UfoVariant::Large, UFO_LARGE_SCORE),
            (UfoVariant::Small, UFO_SMALL_SCORE),
        ] {
            let mut state = empty_test_state();
            state.ufo = Some(Ufo::new(
                1,
                variant,
                Vec2::new(0.4, 0.0),
                Vec2::new(-0.1, 0.0),
            ));

            state.clear_ufo(GameEventKind::UfoDestroyed, true);

            assert_eq!(state.score, expected_score);
            assert!(state.events().iter().any(|event| {
                event.kind == GameEventKind::ScoreIncreased
                    && event.score_delta == Some(expected_score)
            }));
        }
    }

    #[test]
    fn extra_life_awards_once_per_ten_thousand_point_threshold() {
        let mut state = empty_test_state();

        state.add_score(EXTRA_LIFE_SCORE_INTERVAL - 10);
        assert_eq!(state.lives, INITIAL_LIVES);

        state.add_score(10);
        assert_eq!(state.lives, INITIAL_LIVES + 1);
        assert!(state.events().iter().any(|event| {
            event.kind == GameEventKind::ExtraLifeAwarded
                && event.extra_life_threshold == Some(10_000)
        }));

        state.events.clear();
        state.add_score(EXTRA_LIFE_SCORE_INTERVAL);
        assert_eq!(state.lives, INITIAL_LIVES + 2);
        assert!(state.events().iter().any(|event| {
            event.kind == GameEventKind::ExtraLifeAwarded
                && event.extra_life_threshold == Some(20_000)
        }));
    }

    #[test]
    fn lives_display_count_clamps_to_design_maximum() {
        assert_eq!(displayed_lives(0), 0);
        assert_eq!(displayed_lives(INITIAL_LIVES), INITIAL_LIVES);
        assert_eq!(displayed_lives(8), MAX_DISPLAYED_LIVES);
    }

    #[test]
    fn asteroid_spawn_count_progression_matches_original_wave_counter() {
        let counts: Vec<u32> = (1..=7).map(asteroid_spawn_count_for_round).collect();
        assert_eq!(counts, vec![4, 6, 8, 10, 11, 11, 11]);
    }

    #[test]
    fn asteroid_spawn_velocity_uses_playable_drift_scale() {
        let mut rng = rng_for_seed(Some(11));
        let velocity = large_asteroid_spawn_velocity(0, 0, &mut rng);
        let unscaled_max =
            tuning::ASTEROID_RAW_VELOCITY_MAX * tuning::ASTEROID_RAW_VELOCITY_TO_NDC_PER_SEC;
        let scaled_max =
            tuning::ASTEROID_DRIFT_SPEED_MAX_NDC_PER_SEC * tuning::ASTEROID_LARGE_DRIFT_SPEED_SCALE;

        assert!(
            (velocity.x - scaled_max).abs() < EPSILON,
            "velocity.x={}",
            velocity.x
        );
        assert!(velocity.x < unscaled_max * 0.5);
    }

    #[test]
    fn circle_collision_counts_overlap_and_edge_touch_but_not_separation() {
        assert!(circles_overlap(Vec2::ZERO, 0.5, Vec2::new(0.75, 0.0), 0.3));
        assert!(circles_overlap(Vec2::ZERO, 0.5, Vec2::new(0.8, 0.0), 0.3));
        assert!(!circles_overlap(Vec2::ZERO, 0.5, Vec2::new(0.81, 0.0), 0.3));
    }

    #[test]
    fn bullet_expires_after_lifetime_budget() {
        let mut state = empty_test_state();
        state.bullets.push(Bullet::new(
            1,
            Vec2::ZERO,
            Vec2::new(BULLET_SPEED_NDC_PER_SEC, 0.0),
        ));
        state.bullets[0].age_seconds = BULLET_LIFETIME_SECONDS - FIXED_TIMESTEP_SECONDS * 0.5;

        state.step(&ControlState::default(), FIXED_TIMESTEP_SECONDS);

        assert!(state.bullets.is_empty());
        assert!(
            state
                .events()
                .iter()
                .any(|event| event.kind == GameEventKind::BulletExpired)
        );
    }

    #[test]
    fn ship_death_decrements_lives_and_flips_snapshot_alive_flag() {
        let mut state = empty_test_state();
        state.asteroids.push(Asteroid::new(
            1,
            AsteroidSize::Large,
            Vec2::ZERO,
            Vec2::ZERO,
            AsteroidHull::regular(),
        ));
        state.sync_asteroid_count();

        state.step(&ControlState::default(), FIXED_TIMESTEP_SECONDS);

        assert_eq!(state.lives, INITIAL_LIVES - 1);
        assert!(!state.alive);
        assert!(!state.snapshot().is_alive());
        assert!(
            state
                .events()
                .iter()
                .any(|event| event.kind == GameEventKind::ShipDied)
        );
        assert!(
            state
                .events()
                .iter()
                .any(|event| event.kind == GameEventKind::LivesDecremented)
        );
    }

    #[test]
    fn respawn_flips_snapshot_alive_flag_back_on() {
        let mut state = empty_test_state();
        state.asteroids.push(Asteroid::new(
            1,
            AsteroidSize::Large,
            Vec2::ZERO,
            Vec2::ZERO,
            AsteroidHull::regular(),
        ));
        state.sync_asteroid_count();

        state.step(&ControlState::default(), FIXED_TIMESTEP_SECONDS);
        assert!(!state.snapshot().is_alive());

        state.asteroids.clear();
        state.sync_asteroid_count();
        for _ in 0..=((SHIP_RESPAWN_DELAY_SECONDS / FIXED_TIMESTEP_SECONDS).ceil() as usize) {
            state.step(&ControlState::default(), FIXED_TIMESTEP_SECONDS);
        }

        assert!(state.alive);
        assert!(state.snapshot().is_alive());
    }

    #[test]
    fn hyperspace_rejects_second_invocation_during_cooldown() {
        let mut state = GameState::hyperspace_spam_scenario(Some(1));
        let hyperspace = ControlState {
            hyperspace: true,
            ..ControlState::default()
        };

        state.step(&hyperspace, FIXED_TIMESTEP_SECONDS);
        assert!(
            state
                .events()
                .iter()
                .any(|event| event.kind == GameEventKind::HyperspaceTriggered)
        );
        let position_after_first_jump = state.ship.position;
        let lives_after_first_jump = state.lives;

        state.step(&ControlState::default(), FIXED_TIMESTEP_SECONDS);
        state.step(&hyperspace, FIXED_TIMESTEP_SECONDS);

        assert_eq!(state.ship.position, position_after_first_jump);
        assert_eq!(state.lives, lives_after_first_jump);
        assert!(
            state
                .events()
                .iter()
                .any(|event| event.kind == GameEventKind::HyperspaceCooldownRejected)
        );
        assert!(!state.events().iter().any(|event| {
            matches!(
                event.kind,
                GameEventKind::HyperspaceTriggered
                    | GameEventKind::HyperspaceSelfDestruct
                    | GameEventKind::ShipDied
            )
        }));
    }

    #[test]
    fn hyperspace_self_destruct_roll_is_about_ten_percent_for_seeded_rng() {
        let mut rng = rng_for_seed(Some(7));
        let self_destructs = (0..1000)
            .filter(|_| hyperspace_self_destructs(&mut rng))
            .count();
        let rate = self_destructs as f32 / 1000.0;

        assert!(
            (0.08..=0.12).contains(&rate),
            "self_destructs={self_destructs}, rate={rate:.3}"
        );
    }

    #[test]
    fn hyperspace_targets_stay_inside_playfield_bounds() {
        let mut rng = rng_for_seed(Some(11));

        for _ in 0..1000 {
            let target = random_hyperspace_target(&mut rng);
            assert!(
                (PLAYFIELD_MIN.x..=PLAYFIELD_MAX.x).contains(&target.x),
                "x={}",
                target.x
            );
            assert!(
                (PLAYFIELD_MIN.y..=PLAYFIELD_MAX.y).contains(&target.y),
                "y={}",
                target.y
            );
        }
    }

    #[test]
    fn hyperspace_self_destruct_uses_standard_ship_death_flow() {
        let mut state = GameState::hyperspace_spam_scenario(Some(4));
        let hyperspace = ControlState {
            hyperspace: true,
            ..ControlState::default()
        };

        state.step(&hyperspace, FIXED_TIMESTEP_SECONDS);

        assert!(!state.alive);
        assert_eq!(state.lives, INITIAL_LIVES - 1);
        assert!(
            state
                .events()
                .iter()
                .any(|event| event.kind == GameEventKind::HyperspaceTriggered)
        );
        assert!(
            state
                .events()
                .iter()
                .any(|event| event.kind == GameEventKind::HyperspaceSelfDestruct)
        );
        assert!(
            state
                .events()
                .iter()
                .any(|event| event.kind == GameEventKind::ShipDied)
        );
        assert!(
            state
                .events()
                .iter()
                .any(|event| event.kind == GameEventKind::LivesDecremented)
        );
    }

    #[test]
    fn snapshot_reports_live_asteroid_count_after_each_tick() {
        let mut game = GameLoop::new_seeded(Some(1));
        let mut snapshots = Vec::new();

        game.advance(
            FIXED_TIMESTEP_SECONDS,
            &ControlState::default(),
            |snapshot| {
                snapshots.push(snapshot);
            },
        );

        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].asteroid_count, 4);
    }

    #[test]
    fn game_loop_raises_highscore_event_when_score_crosses_loaded_value() {
        let mut game = GameLoop::from_state_with_high_score(
            GameState::set_highscore_12345_scenario(Some(1)),
            0,
        );

        game.advance(FIXED_TIMESTEP_SECONDS, &ControlState::default(), |_| {});
        let events = game.drain_events();

        assert_eq!(game.high_score(), 12_345);
        assert!(events.iter().any(|event| {
            event.kind == GameEventKind::HighScoreIncreased && event.high_score == Some(12_345)
        }));
    }

    #[test]
    fn ufo_spawn_rate_uses_original_reload_curve_values() {
        assert_eq!(
            ufo_spawn_reload_ticks_for_score(0),
            UFO_SPAWN_RELOAD_INITIAL_TICKS
        );
        assert_eq!(
            ufo_spawn_reload_ticks_for_score(UFO_SPAWN_SCORE_STEP_POINTS),
            UFO_SPAWN_RELOAD_INITIAL_TICKS - UFO_SPAWN_RELOAD_DECREMENT_TICKS
        );
        assert_eq!(
            ufo_spawn_reload_ticks_for_score(1_000_000),
            UFO_SPAWN_RELOAD_MIN_TICKS
        );
    }

    #[test]
    fn ufo_variant_switches_at_disassembly_score_threshold() {
        assert_eq!(
            ufo_variant_for_score(UFO_SMALL_SCORE_THRESHOLD - 1),
            UfoVariant::Large
        );
        assert_eq!(
            ufo_variant_for_score(UFO_SMALL_SCORE_THRESHOLD),
            UfoVariant::Small
        );
    }

    #[test]
    fn ufo_large_scenario_spawns_and_fires_randomly() {
        let mut state = GameState::ufo_large_scenario(Some(1));
        let events = step_for_seconds(&mut state, ufo_spawn_interval_seconds_for_score(0) + 0.8);

        assert!(events.iter().any(|event| {
            event.kind == GameEventKind::UfoFiredRandom
                && event.ufo_variant == Some(UfoVariant::Large)
        }));
    }

    #[test]
    fn ufo_small_scenario_spawns_and_fires_at_ship() {
        let mut state = GameState::ufo_small_scenario(Some(1));
        let events = step_for_seconds(
            &mut state,
            ufo_spawn_interval_seconds_for_score(UFO_SMALL_SCORE_THRESHOLD) + 0.8,
        );

        assert!(events.iter().any(|event| {
            event.kind == GameEventKind::UfoFiredAimed
                && event.ufo_variant == Some(UfoVariant::Small)
        }));
        assert!(
            events
                .iter()
                .any(|event| event.kind == GameEventKind::ScoreGte10000)
        );
    }

    #[test]
    fn bullet_hit_asteroid_script_finishes_all_three_tiers() {
        let mut state = GameState::bullet_hit_asteroid_scenario(Some(1));
        let mut events = Vec::new();
        for tick in 0..300 {
            let input = ControlState {
                fire: tick == 0,
                ..ControlState::default()
            };
            state.step(&input, FIXED_TIMESTEP_SECONDS);
            events.extend_from_slice(state.events());
        }

        let counts = state.asteroid_size_counts();
        assert_eq!(counts.large, 0);
        assert_eq!(counts.medium, 1);
        assert_eq!(counts.small, 1);
        assert!(events.iter().any(|event| {
            event.kind == GameEventKind::AsteroidDestroyed
                && event.asteroid_size == Some(AsteroidSize::Small)
        }));
    }

    #[test]
    fn autonomous_play_script_keeps_reseeding_rounds_after_clears() {
        let mut state = GameState::autonomous_play_10min_scenario(Some(1));
        let ticks = (30.0 / FIXED_TIMESTEP_SECONDS).ceil() as usize;
        let mut saw_clear = false;
        let mut active_after_twenty_seconds = false;
        for tick in 0..ticks {
            state.step(&ControlState::default(), FIXED_TIMESTEP_SECONDS);
            saw_clear |= state.asteroid_count == 0;
            if tick as f32 * FIXED_TIMESTEP_SECONDS >= 20.0 && state.asteroid_count > 0 {
                active_after_twenty_seconds = true;
            }
        }

        assert!(saw_clear);
        assert!(active_after_twenty_seconds);
        assert!(state.round > 1);
        assert!(state.score > SCORE_PLACEHOLDER);
        assert!(state.alive);
        assert!(!state.game_over);
    }

    fn empty_test_state() -> GameState {
        GameState::empty_seeded(Some(7))
    }

    fn step_for_seconds(state: &mut GameState, seconds: f32) -> Vec<GameEvent> {
        let ticks = (seconds / FIXED_TIMESTEP_SECONDS).ceil() as usize;
        let mut events = Vec::new();
        for _ in 0..ticks {
            state.step(&ControlState::default(), FIXED_TIMESTEP_SECONDS);
            events.extend_from_slice(state.events());
        }
        events
    }

    fn state_with_one_asteroid(size: AsteroidSize) -> GameState {
        let mut state = empty_test_state();
        state.next_asteroid_id = 2;
        state.asteroids.push(Asteroid::new(
            1,
            size,
            Vec2::ZERO,
            Vec2::new(tuning::ASTEROID_DRIFT_SPEED_MIN_NDC_PER_SEC, 0.0),
            AsteroidHull::regular(),
        ));
        state.sync_asteroid_count();
        state
    }
}
