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
pub const SHIP_MAX_VELOCITY_UNITS_PER_SEC: f32 = 6.0;
pub const SHIP_THRUST_ACCEL_UNITS_PER_SEC_SQUARED: f32 = 0.05 * 60.0;
pub const SCORE_PLACEHOLDER: u32 = 0;
pub const ASTEROIDS_PER_WAVE_BOOTSTRAP: u32 = 2;
pub const ASTEROIDS_PER_WAVE_INCREMENT: u32 = 2;
pub const ASTEROIDS_PER_WAVE_MAX: u32 = 11;
pub const ASTEROID_HULL_VERTEX_COUNT: usize = 10;
pub const INITIAL_LIVES: u32 = 3;
pub const BULLET_LIFETIME_SECONDS: f32 = 1.0;
pub const BULLET_SPEED_NDC_PER_SEC: f32 = 1.65;
pub const BULLET_RADIUS_NDC: f32 = 0.012;
pub const SHIP_COLLISION_RADIUS_NDC: f32 = 0.44 * tuning::SHIP_SPINNING_SCALE;
pub const SHIP_RESPAWN_DELAY_SECONDS: f32 = 1.25;
pub const SHIP_RESPAWN_INVULNERABILITY_SECONDS: f32 = 1.25;

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
    ShipDied,
    LivesDecremented,
    Respawn,
    GameOver,
}

impl GameEventKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::BulletFired => "bullet-fired",
            Self::BulletExpired => "bullet-expired",
            Self::BulletHitAsteroid => "bullet-hit-asteroid",
            Self::AsteroidSplit => "asteroid-split",
            Self::AsteroidDestroyed => "asteroid-destroyed",
            Self::ShipDied => "ship-died",
            Self::LivesDecremented => "lives-decremented",
            Self::Respawn => "respawn",
            Self::GameOver => "game-over",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameEvent {
    pub kind: GameEventKind,
    pub asteroid_size: Option<AsteroidSize>,
}

impl GameEvent {
    fn new(kind: GameEventKind) -> Self {
        Self {
            kind,
            asteroid_size: None,
        }
    }

    fn asteroid(kind: GameEventKind, asteroid_size: AsteroidSize) -> Self {
        Self {
            kind,
            asteroid_size: Some(asteroid_size),
        }
    }

    pub fn name(self) -> &'static str {
        self.kind.name()
    }
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
    next_asteroid_id: u32,
    next_bullet_id: u32,
    respawn_timer_seconds: f32,
    invulnerability_timer_seconds: f32,
    fire_was_down: bool,
    events: Vec<GameEvent>,
    rng: SeededRng,
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
            next_asteroid_id: 1,
            next_bullet_id: 1,
            respawn_timer_seconds: 0.0,
            invulnerability_timer_seconds: 0.0,
            fire_was_down: false,
            events: Vec::new(),
            rng: rng_for_seed(seed),
        }
    }

    fn step(&mut self, input: &ControlState, dt: f32) {
        self.events.clear();
        self.update_respawn(dt);
        if self.alive {
            self.ship.integrate(input, dt);
        }
        for asteroid in &mut self.asteroids {
            asteroid.integrate(dt);
        }
        self.update_fire(input);
        for bullet in &mut self.bullets {
            bullet.integrate(dt);
        }
        self.despawn_expired_bullets();
        self.resolve_bullet_asteroid_collisions();
        // UFO collision is intentionally deferred until the UFO task adds UFO
        // actors and score rules; bullet-vs-UFO will use the same circle model.
        self.resolve_ship_asteroid_collisions();
        if self.invulnerability_timer_seconds > 0.0 {
            self.invulnerability_timer_seconds = (self.invulnerability_timer_seconds - dt).max(0.0);
        }
        self.sync_asteroid_count();
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
        let Some(index) = self.asteroids.iter().position(|asteroid| asteroid.id == id) else {
            return false;
        };
        let parent = self.asteroids.remove(index);
        self.push_asteroid_event(GameEventKind::BulletHitAsteroid, parent.size);
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

    pub fn any_asteroid_wrapped_last_tick(&self) -> bool {
        self.asteroids
            .iter()
            .any(|asteroid| asteroid.wrapped_last_tick)
    }

    pub fn any_bullet_wrapped_last_tick(&self) -> bool {
        self.bullets.iter().any(|bullet| bullet.wrapped_last_tick)
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

    fn fire_bullet(&mut self) {
        let (angle_sin, angle_cos) = self.ship.angle.sin_cos();
        let forward = Vec2::new(angle_cos, angle_sin);
        let id = self.next_bullet_id;
        self.next_bullet_id = self.next_bullet_id.wrapping_add(1).max(1);
        let position = wrap_position(
            self.ship.position + forward * (SHIP_COLLISION_RADIUS_NDC + BULLET_RADIUS_NDC),
        );
        let velocity = self.ship.velocity + forward * BULLET_SPEED_NDC_PER_SEC;
        self.bullets.push(Bullet::new(id, position, velocity));
        self.push_event(GameEventKind::BulletFired);
    }

    fn despawn_expired_bullets(&mut self) {
        let before = self.bullets.len();
        self.bullets.retain(|bullet| !bullet.is_expired());
        for _ in self.bullets.len()..before {
            self.push_event(GameEventKind::BulletExpired);
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

    fn spawn_large_asteroid(&mut self, index: u32) -> Asteroid {
        let side = index % 4;
        let position = edge_spawn_position(side, &mut self.rng);
        let velocity = edge_spawn_velocity(side, index, &mut self.rng);
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

/// DESIGN.md Open Question 7 decision: gameplay collisions use circle-vs-circle
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
    raw * tuning::ASTEROID_RAW_VELOCITY_TO_NDC_PER_SEC
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
        }
    }
}

impl GameLoop {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_seeded(seed: Option<u64>) -> Self {
        let state = GameState::new_seeded(seed);
        Self {
            previous: state.clone(),
            current: state,
            pending_events: Vec::new(),
            accumulator_seconds: 0.0,
            tick: 0,
            paused: false,
        }
    }

    pub fn from_state(state: GameState) -> Self {
        Self {
            previous: state.clone(),
            current: state,
            pending_events: Vec::new(),
            accumulator_seconds: 0.0,
            tick: 0,
            paused: false,
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
            self.pending_events.extend_from_slice(self.current.events());
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
            scale: tuning::SHIP_SPINNING_SCALE,
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

    pub fn render_time_seconds(&self) -> f32 {
        self.tick as f32 * FIXED_TIMESTEP_SECONDS + self.accumulator_seconds
    }

    pub fn current(&self) -> &GameState {
        &self.current
    }

    pub fn tick(&self) -> u64 {
        self.tick
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
    fn idle_ship_retains_velocity_without_friction() {
        let mut ship = ShipState {
            position: Vec2::ZERO,
            velocity: Vec2::new(1.25, -0.75),
            angle: 0.0,
        };
        let initial_velocity = ship.velocity;

        for _ in 0..240 {
            ship.integrate(&ControlState::default(), FIXED_TIMESTEP_SECONDS);
        }

        assert_eq!(ship.velocity, initial_velocity);
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
    fn asteroid_spawn_count_progression_matches_original_wave_counter() {
        let counts: Vec<u32> = (1..=7).map(asteroid_spawn_count_for_round).collect();
        assert_eq!(counts, vec![4, 6, 8, 10, 11, 11, 11]);
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

    fn empty_test_state() -> GameState {
        GameState::empty_seeded(Some(7))
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
