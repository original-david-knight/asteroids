use std::ops::{Add, Mul, Neg, Sub};

use bytemuck::{Pod, Zeroable};

use crate::tuning;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self::new(0.0, 0.0);
    pub const X: Self = Self::new(1.0, 0.0);
    pub const Y: Self = Self::new(0.0, 1.0);

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn length_squared(self) -> f32 {
        self.x.mul_add(self.x, self.y * self.y)
    }

    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    pub fn normalized_or(self, fallback: Self) -> Self {
        let length = self.length();
        if length > f32::EPSILON {
            self * (1.0 / length)
        } else {
            fallback
        }
    }

    pub fn left_perp(self) -> Self {
        Self::new(-self.y, self.x)
    }

    pub fn to_array(self) -> [f32; 2] {
        [self.x, self.y]
    }
}

impl From<[f32; 2]> for Vec2 {
    fn from(value: [f32; 2]) -> Self {
        Self::new(value[0], value[1])
    }
}

impl Add for Vec2 {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl Sub for Vec2 {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl Mul<f32> for Vec2 {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs)
    }
}

impl Neg for Vec2 {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self::new(-self.x, -self.y)
    }
}

/// A single vector-beam draw command.
///
/// `dwell_us` is assigned explicitly by the emitter for each segment in v1.
/// The renderer does not synthesize dwell from segment length or a virtual beam-rate model.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct BeamCommand {
    pub start: Vec2,
    pub end: Vec2,
    pub intensity: f32,
    pub dwell_us: f32,
}

impl BeamCommand {
    pub fn new(start: Vec2, end: Vec2, intensity: f32, dwell_us: f32) -> Self {
        Self {
            start,
            end,
            intensity: normalized_intensity(intensity),
            dwell_us: non_negative(dwell_us),
        }
    }

    pub fn builder(start: Vec2, end: Vec2) -> BeamCommandBuilder {
        BeamCommandBuilder::new(start, end)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BeamCommandBuilder {
    start: Vec2,
    end: Vec2,
    intensity: f32,
    dwell_us: f32,
}

impl BeamCommandBuilder {
    pub fn new(start: Vec2, end: Vec2) -> Self {
        Self {
            start,
            end,
            intensity: 1.0,
            dwell_us: tuning::SHIP_OUTLINE_SEGMENT_DWELL_US,
        }
    }

    pub fn intensity(mut self, intensity: f32) -> Self {
        self.intensity = intensity;
        self
    }

    pub fn dwell_us(mut self, dwell_us: f32) -> Self {
        self.dwell_us = dwell_us;
        self
    }

    pub fn endpoint_dwell_bonus(mut self) -> Self {
        self.dwell_us += tuning::ENDPOINT_DWELL_BONUS_US;
        self
    }

    pub fn build(self) -> BeamCommand {
        BeamCommand::new(self.start, self.end, self.intensity, self.dwell_us)
    }
}

#[derive(Clone, Debug, Default)]
pub struct BeamEmitter {
    commands: Vec<BeamCommand>,
}

impl BeamEmitter {
    pub fn new() -> Self {
        Self::with_capacity(0)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            commands: Vec::with_capacity(capacity),
        }
    }

    pub fn clear(&mut self) {
        self.commands.clear();
    }

    pub fn emit(&mut self, command: BeamCommand) -> &mut Self {
        self.commands.push(command);
        self
    }

    pub fn emit_segment(
        &mut self,
        start: Vec2,
        end: Vec2,
        intensity: f32,
        dwell_us: f32,
    ) -> &mut Self {
        self.emit(BeamCommand::new(start, end, intensity, dwell_us))
    }

    pub fn emit_segment_with_endpoint_bonus(
        &mut self,
        start: Vec2,
        end: Vec2,
        intensity: f32,
        base_dwell_us: f32,
    ) -> &mut Self {
        self.emit_segment(
            start,
            end,
            intensity,
            base_dwell_us + tuning::ENDPOINT_DWELL_BONUS_US,
        )
    }

    pub fn emit_ship_outline_segment(
        &mut self,
        start: Vec2,
        end: Vec2,
        intensity: f32,
    ) -> &mut Self {
        self.emit_segment(start, end, intensity, tuning::SHIP_OUTLINE_SEGMENT_DWELL_US)
    }

    pub fn emit_ship_outline_segment_with_endpoint_bonus(
        &mut self,
        start: Vec2,
        end: Vec2,
        intensity: f32,
    ) -> &mut Self {
        self.emit_segment_with_endpoint_bonus(
            start,
            end,
            intensity,
            tuning::SHIP_OUTLINE_SEGMENT_DWELL_US,
        )
    }

    pub fn emit_asteroid_hull_segment(
        &mut self,
        start: Vec2,
        end: Vec2,
        intensity: f32,
    ) -> &mut Self {
        self.emit_segment(
            start,
            end,
            intensity,
            tuning::ASTEROID_HULL_SEGMENT_DWELL_US,
        )
    }

    pub fn emit_bullet_dot(&mut self, center: Vec2, half_extent: f32, intensity: f32) -> &mut Self {
        let half_extent = half_extent.abs().max(f32::EPSILON);
        self.emit_segment(
            center - Vec2::X * half_extent,
            center + Vec2::X * half_extent,
            intensity,
            tuning::BULLET_DOT_DWELL_US,
        )
    }

    pub fn commands(&self) -> &[BeamCommand] {
        &self.commands
    }

    #[allow(dead_code)]
    pub fn into_commands(self) -> Vec<BeamCommand> {
        self.commands
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct BeamVertex {
    pub position: [f32; 2],
    pub segment_start: [f32; 2],
    pub segment_end: [f32; 2],
    pub intensity: f32,
    pub dwell_us: f32,
}

impl BeamVertex {
    fn new(position: Vec2, command: BeamCommand) -> Self {
        Self {
            position: position.to_array(),
            segment_start: command.start.to_array(),
            segment_end: command.end.to_array(),
            intensity: command.intensity,
            dwell_us: command.dwell_us,
        }
    }
}

pub fn segment_left_perpendicular(start: Vec2, end: Vec2) -> Vec2 {
    let direction = end - start;
    if direction.length_squared() > f32::EPSILON {
        direction.normalized_or(Vec2::X).left_perp()
    } else {
        Vec2::Y
    }
}

pub fn expand_beam_quad(command: BeamCommand, half_width: f32) -> [BeamVertex; 6] {
    let half_width = half_width.abs();
    let tangent = (command.end - command.start).normalized_or(Vec2::X);
    let normal = segment_left_perpendicular(command.start, command.end);
    let offset = normal * half_width;

    let (start, end) = if (command.end - command.start).length_squared() > f32::EPSILON {
        (command.start, command.end)
    } else {
        (
            command.start - tangent * half_width,
            command.end + tangent * half_width,
        )
    };

    [
        BeamVertex::new(start + offset, command),
        BeamVertex::new(start - offset, command),
        BeamVertex::new(end + offset, command),
        BeamVertex::new(end + offset, command),
        BeamVertex::new(start - offset, command),
        BeamVertex::new(end - offset, command),
    ]
}

pub fn expand_beam_commands(
    commands: &[BeamCommand],
    half_width: f32,
    out_vertices: &mut Vec<BeamVertex>,
) {
    out_vertices.clear();
    out_vertices.reserve(commands.len() * 6);
    for command in commands {
        out_vertices.extend_from_slice(&expand_beam_quad(*command, half_width));
    }
}

fn normalized_intensity(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn non_negative(value: f32) -> f32 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_vec2_close(actual: Vec2, expected: Vec2) {
        const EPSILON: f32 = 0.00001;
        assert!(
            (actual.x - expected.x).abs() <= EPSILON,
            "x mismatch: actual={}, expected={}",
            actual.x,
            expected.x
        );
        assert!(
            (actual.y - expected.y).abs() <= EPSILON,
            "y mismatch: actual={}, expected={}",
            actual.y,
            expected.y
        );
    }

    fn position(vertex: BeamVertex) -> Vec2 {
        Vec2::from(vertex.position)
    }

    #[test]
    fn axis_aligned_segment_offsets_by_left_perpendicular_half_width() {
        let command = BeamCommand::new(Vec2::ZERO, Vec2::X, 1.0, 30.0);
        let vertices = expand_beam_quad(command, 0.25);

        assert_vec2_close(
            segment_left_perpendicular(command.start, command.end),
            Vec2::Y,
        );
        assert_vec2_close(position(vertices[0]), Vec2::new(0.0, 0.25));
        assert_vec2_close(position(vertices[1]), Vec2::new(0.0, -0.25));
        assert_vec2_close(position(vertices[2]), Vec2::new(1.0, 0.25));
        assert_vec2_close(position(vertices[5]), Vec2::new(1.0, -0.25));
    }

    #[test]
    fn diagonal_segment_offsets_by_normalized_left_perpendicular_half_width() {
        let command = BeamCommand::new(Vec2::ZERO, Vec2::new(1.0, 1.0), 1.0, 30.0);
        let vertices = expand_beam_quad(command, 2.0_f32.sqrt());

        assert_vec2_close(
            segment_left_perpendicular(command.start, command.end),
            Vec2::new(-0.5_f32.sqrt(), 0.5_f32.sqrt()),
        );
        assert_vec2_close(position(vertices[0]), Vec2::new(-1.0, 1.0));
        assert_vec2_close(position(vertices[1]), Vec2::new(1.0, -1.0));
        assert_vec2_close(position(vertices[2]), Vec2::new(0.0, 2.0));
        assert_vec2_close(position(vertices[5]), Vec2::new(2.0, 0.0));
    }
}
