# Asteroids — Build Plan

Source: `/office-hours` design session, 2026-04-25. See `DESIGN.md` for full vision and specs.

Visuals-first ordering. Each milestone is a "the magic is real" checkpoint, not a "it works" checkpoint.

## Distribution Plan

None. Native Linux binary built via `cargo build --release`, run via `./target/release/asteroids` or `cargo run --release`. No CI, no installer, no GitHub release. Lives in `/home/david/workspace/asteroids/` and runs on the user's machine. If sharing later, that's a separate decision; for v1, no.

## Build Steps

### 0. Capability probe (~30 min)

Throwaway Rust binary that:
- Opens a fullscreen window, prints monitor refresh rate
- Lists supported swap-chain formats (verify `Rgba16Float` or `Rgba8Unorm`)
- Measures GPU time for a 4K full-screen blit
- Opens cpal default output, prints actual block size + sample rate
- Attempts SCHED_FIFO promotion via rtkit, reports success

Validates the assumptions in `DESIGN.md` (Display & Timing Model, Audio Realtime Contract) before any real architecture is committed.

### 1. Project skeleton

`cargo init`. Add deps: `wgpu`, `winit`, `cpal`, `fundsp`, `ringbuf`, `bytemuck`, `pollster`. Get a fullscreen black window opening.

### 2. Hello triangle through wgpu

Verify the render pipeline works end-to-end. Throwaway triangle.

### 3. Beam-emit API draft

Define `BeamCommand { start: Vec2, end: Vec2, intensity: f32, dwell_us: f32 }`. Build a vertex buffer of line segments per frame.

### 4. Phosphor accumulator (the magic milestone)

Two `Rgba16Float` textures, ping-pong. Each frame: render current beam commands into phosphor texture, blend with previous frame's phosphor at a decay rate. This alone, with hard white lines, gives you the trail effect.

### 5. Bloom pass

Downsample-and-upsample Gaussian blur in WGSL. Add to phosphor output. Lines start to glow.

> **Decision point:** aspect-ratio strategy (DESIGN.md Open Question 1). Decide here, before step 6, with a real glowing line on your actual monitor for reference.

### 6. One spinning ship — *soul-visible milestone*

Define the 4-vertex Asteroids ship outline. Rotate it. Watch the glow trail behind it. Stop here, look at it, decide if it's right before adding gameplay.

> **Soft deadline:** ~3-4 weekends to reach this milestone. If bloom-and-phosphor work overruns, course-correct rather than scope-creep. The deadline is an anti-rabbit-hole guard, not a shipping deadline.

### 7. Final composite

Combine phosphor + bloom into the swap-chain texture. CRT curvature deferred (Open Question 5; vector tubes had little geometric distortion, may be raster-aesthetic creep).

### 8. Audio scaffolding

cpal output stream, fundsp DSP graph wired to a stereo sample buffer.

### 9. First voice: thrust — *soul-visible-and-audible milestone*

Sawtooth through low-pass with envelope, gated by W-key.

### 10. Game loop and physics

Ship inertia, screen wrap, rotation, thrust integration. No friction (per original).

### 11. Asteroids

Three size tiers, breaking on hit. Spawn pattern from original disassembly. Bookmark Norbert Kehrer's disassembly + computerarcheology.com here.

### 12. Bullets, collision, game over

Basic gameplay loop. Collision: circle vs circle using sprite-extent radii (Open Question 7).

### 13. Remaining synth voices

Fire, explosion (white noise through bandpass), ufo siren, heartbeat coupled to asteroid count. Confirm exact heartbeat tempo curve from original (Open Question 4).

### 14. UFO AI, scoring, lives, hyperspace

All the original mechanics. Hyperspace keeps the ~10% self-destruct chance (Open Question 6).

### 15. Polish pass

Tune phosphor decay, bloom intensity, beam dwell, audio mix. The 1% that takes 30% of the time.

## Reviewer Concerns (acknowledged, not blocking)

Flagged in spec review and accepted as either acceptable for v1 or to-be-resolved during implementation:

- **Spiral-of-death guard for fixed timestep** — handled at impl time with a max-substeps cap of 4 per render frame.
- **High-score persistence format** — single ASCII-decimal integer in `~/.local/share/asteroids/highscore`. Trivial.
- **PipeWire vs ALSA selection** — verified in step 0 capability probe.
- **Hyprland Wayland fallback to X11 plan B is rough** — accepted; if borderless fullscreen on Hyprland behaves correctly per step 0 probe, no fallback needed.
- **4K @ 144Hz post-processing GPU budget** — measured in step 0, not assumed.
