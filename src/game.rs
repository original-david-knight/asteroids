use std::f32::consts::TAU;

use crate::{audio::GameSnapshot, beam::Vec2, tuning};

pub const FIXED_TIMESTEP_SECONDS: f32 = 1.0 / 240.0;
pub const MAX_SUBSTEPS_PER_FRAME: u32 = 4;
pub const PLAYFIELD_MIN: Vec2 = Vec2::new(-1.0, -1.0);
pub const PLAYFIELD_MAX: Vec2 = Vec2::new(1.0, 1.0);
pub const SHIP_MAX_VELOCITY_UNITS_PER_SEC: f32 = 6.0;
pub const SHIP_THRUST_ACCEL_UNITS_PER_SEC_SQUARED: f32 = 0.05 * 60.0;
pub const SCORE_PLACEHOLDER: u32 = 0;
pub const ASTEROID_COUNT_PLACEHOLDER: u32 = 0;

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GameState {
    pub ship: ShipState,
    pub alive: bool,
    pub score: u32,
    pub asteroid_count: u32,
}

impl Default for GameState {
    fn default() -> Self {
        Self {
            ship: ShipState::default(),
            alive: true,
            score: SCORE_PLACEHOLDER,
            asteroid_count: ASTEROID_COUNT_PLACEHOLDER,
        }
    }
}

impl GameState {
    fn step(&mut self, input: &ControlState, dt: f32) {
        self.ship.integrate(input, dt);
    }

    pub fn snapshot(self) -> GameSnapshot {
        GameSnapshot::new(self.asteroid_count, self.alive, self.score)
    }
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
    accumulator_seconds: f32,
    tick: u64,
    paused: bool,
}

impl Default for GameLoop {
    fn default() -> Self {
        let state = GameState::default();
        Self {
            previous: state,
            current: state,
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

    pub fn advance<F>(
        &mut self,
        frame_dt_seconds: f32,
        input: &ControlState,
        mut on_tick: F,
    ) -> AdvanceReport
    where
        F: FnMut(GameSnapshot),
    {
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
            self.previous = self.current;
            self.current.step(input, FIXED_TIMESTEP_SECONDS);
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

    pub fn render_time_seconds(&self) -> f32 {
        self.tick as f32 * FIXED_TIMESTEP_SECONDS + self.accumulator_seconds
    }

    pub fn current(&self) -> GameState {
        self.current
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
    Vec2::new(
        wrap_coordinate(position.x, PLAYFIELD_MIN.x, PLAYFIELD_MAX.x),
        wrap_coordinate(position.y, PLAYFIELD_MIN.y, PLAYFIELD_MAX.y),
    )
}

fn wrap_coordinate(value: f32, min: f32, max: f32) -> f32 {
    let width = max - min;
    if width <= 0.0 {
        return min;
    }
    if value < min || value > max {
        (value - min).rem_euclid(width) + min
    } else {
        value
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
}
