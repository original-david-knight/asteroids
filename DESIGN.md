# Asteroids — Vector Museum Piece

Source: `/office-hours` design session, 2026-04-25. Status: APPROVED.

## Problem Statement

Build a faithful clone of the 1979 Atari Asteroids arcade game as a personal craft project. "Faithful" means recreating the *visual fidelity* of the original vector display (line glow, phosphor afterglow, beam-driven rendering) and the *audio fidelity* of the original analog synth chip (real-time waveform generation — thrust, fire, four-tone explosion, the heartbeat that speeds up with the round). Higher resolution and higher frame rate than the 1979 cabinet, full-screen on a modern widescreen monitor, native Linux, single-player, no distribution surface.

The point is the building, not the playing — MAME already plays the original ROM perfectly.

## What Makes This Cool

The version 99% of Asteroids clones miss. The original ran on a vector display — a CRT beam drawing actual lines, leaving a soft phosphor glow that hard-edged pixel-art clones can't replicate. Combined with the analog audio synthesis chip, the original arcade cabinet had a feel that no pixel-perfect emulator captures because emulators reproduce the *signal*, not the *display*. This project simulates the *display* itself: a virtual vector tube with phosphor accumulation, beam-intensity falloff, slight CRT curvature, and a real DSP graph for the synth voices.

End result is the version someone screenshots and posts with the caption "this is what 1979 actually looked like."

## Premises

1. **Building project, not playing project.** Value comes from craft of construction, not from access to Asteroids gameplay. MAME exists.
2. **Native Linux binary, zero distribution surface.** `cargo run` and play. No release artifacts.
3. **Tech stack picks for final fidelity, not for shipping speed or joy of writing.** *(Revised mid-session: original premise was 'pick for joy.' User corrected — best end experience matters more than weekly velocity.)*
4. **Renderer first, with a soft deadline.** Visuals-first build order. Time-box first milestone (rotating glowing ship) to ~3-4 weekends; if the bloom-and-phosphor work runs longer, course-correct rather than scope-creep into infinity. The deadline is an anti-rabbit-hole guard, not a shipping deadline.
5. **Aspect-ratio strategy decided before step 6 (the spinning ship milestone), after seeing a real glowing line on the actual monitor.** Decision can't be made cold; needs visual reference.
6. **Audio is real DSP from day 0 in *architecture*, wiring deferred to step 8.** The audio module exists from the start with the right shape (lockfree game→audio channel, pre-built voice graphs, no allocation in callback) so step 8 is "wire up cpal," not "design audio." Synthesis voices, not samples. Architecture supports oscillators, filters, envelopes, mixers.

## Constraints

- Native Linux binary, not web, not cross-platform
- Single-player only (drop alternating 2-player, even for v2)
- No CI/CD, no distribution, no installer — `cargo run` and play
- Full-screen on widescreen modern monitor; aspect-ratio strategy decided before step 6 (see Open Questions)
- Real-time audio synthesis from day 0 (no sample WAVs)
- 144Hz target frame rate, low input latency
- Effort is not a constraint — fidelity is

## Tech Stack — Approach D (Recommended, Approved)

**Rust + wgpu + cpal + fundsp + custom beam simulator.**

Custom GPU pipeline that simulates the actual vector hardware. Game emits beam-draw commands per frame; a WGSL shader rasterizes beam paths into an `Rgba16Float` texture; a phosphor-accumulation pass blends with the previous frame's `Rgba16Float` ping-pong texture using exponential decay. Multi-pass post: phosphor → bloom (Gaussian downsample/upsample) → optional curvature → final composite. Audio: fundsp DSP graph with pre-built named voices (thrust, fire, explosion, ufo, heartbeat). Game→audio communication via lockfree ringbuffer.

Effort: L. Risk: Med-High. Fidelity ceiling: ~9.5/10. The renderer simulates the *display*, not just its output.

Chosen because the user's stated optimization function is "best end experience, effort doesn't matter." D is the only approach where the renderer itself simulates the vector hardware rather than approximating its output. Every other approach hits a fidelity ceiling imposed by an off-the-shelf bloom shader; D's ceiling is the user's own taste.

Tradeoffs accepted: ~3-4x the code volume of Approach A (~3000 LOC vs ~800), longer time to first playable, real graphics-programming surface (WGSL, render pipelines, ping-pong textures), longer compile times.

### Approaches Considered (rejected)

- **A: Minimal Viable — C99 + raylib.** ~500-800 LOC, one file. Effort S, Risk Low, Fidelity ceiling ~7/10. Capped by raylib's bloom example and lack of custom multi-pass post.
- **B: Balanced — Rust + macroquad + cpal + fundsp.** Effort M, Risk Med, ~7/10 graphics, ~9/10 audio. Audio excellent; rendering fine but not bespoke.
- **C: Lateral — Go + ebitengine.** User's daily language. Effort M, Risk Med, ~6.5/10. Kage shaders more limited than WGSL; vector lines less idiomatic; GC pauses risky for audio.

## Beam Rasterizer Spec

- **Input:** `BeamCommand { start: Vec2, end: Vec2, intensity: f32, dwell_us: f32 }`. `intensity` is normalized [0,1] base brightness. `dwell_us` is a per-segment scalar set explicitly by the emitter (no virtual beam-rate model in v1 — emitter tags each segment).
- **Default dwell rules:** ship outline segments = 30µs each; asteroid hull segments = 25µs; bullet "dot" = 40µs (short bright). Endpoints get a small extra dwell bonus (~10µs) to simulate beam pause at corners. All tunable constants.
- **WGSL approach:** analytic line SDF in a fragment shader — for each pixel covered by a quad bounding the segment, compute distance to the line; brightness = `intensity * dwell_factor * exp(-distance² / sigma²)` where `sigma` is the beam-spot radius (~1.0 physical pixels). Slow beam (high dwell) increases brightness *and* slightly increases sigma (beam spot blooms when stationary). This couples width and brightness as on real hardware, resolving the apparent contradiction between "fixed line width" and "dwell affects brightness."
- **Phosphor accumulator:** `Rgba16Float` ping-pong, formula `out = clamp(in_beam + in_previous * exp(-frame_dt / tau), 0, MAX_LUMA)` where `MAX_LUMA = 8.0` (HDR-like headroom, tone-mapped at composite). Tone map at composite: Reinhard `x / (x + 1)` to [0,1], then gamma 2.2.
- **`tau` default:** 70ms. Exposed as a debug F-key slider so it can be tuned with the renderer running.

## Audio Realtime Contract

- **Backend:** cpal default device, prefer PipeWire on modern Linux (cpal handles selection; verify in capability probe). Fallback ALSA direct.
- **Block size:** request 256 samples @ 48kHz (~5.3ms). Accept whatever the backend gives; design DSP to be block-size-agnostic.
- **Thread priority:** at startup, attempt SCHED_FIFO via rtkit (DBus); on failure, log and continue with default priority (acceptable for a personal-use binary; xruns will be visible).
- **Ringbuffer:** `ringbuf` crate, capacity 1024 messages. Schema: `enum AudioMsg { SetParam(VoiceId, ParamId, f32), Trigger(VoiceId), Release(VoiceId), GameState(GameSnapshot) }` where `GameSnapshot` is a small POD with asteroid count, alive flag, score. Non-blocking try-push from game thread; if full, drop oldest non-critical messages (param updates), preserve triggers.
- **Voice graph rule:** every voice graph is built once at app startup with all DSP nodes pre-allocated. Parameter mutation is via shared `AtomicF32` cells read each block, NOT via fundsp graph rebuild. fundsp's `Var` and `shared` patterns support this — verify in capability probe.
- **Target:** zero xruns under normal play on the user's machine. Acceptable degradation if SCHED_FIFO unavailable.

## Display & Timing Model

- **Display server:** X11 first for predictable vsync and exclusive fullscreen behavior. If user's compositor is Wayland (Hyprland), test borderless fullscreen first; fall back to X11 session for the project if vsync/tearing is unreliable.
- **Window mode:** borderless fullscreen, vsync on, no exclusive fullscreen (modern monitors don't need it for tearing avoidance with vsync).
- **Internal render resolution:** native monitor resolution (likely 2560x1440 or 3840x2160). Phosphor textures match. No supersampling for v1 — bloom already gives the soft edge feel.
- **Simulation step:** fixed-timestep at 1/240s for physics (deterministic, decoupled from render), interpolated to display rate. Audio runs on cpal's callback thread at 48kHz, blocksize 256 samples.
- **HiDPI:** report and respect physical pixel scale; render at physical resolution.

## Input Mapping

- Rotate left: `A` and Left arrow
- Rotate right: `D` and Right arrow
- Thrust: `J`
- Fire: `L`
- Hyperspace: `H` (also `Shift` as alt)
- Pause: `P`
- Quit: `Esc`
- Gamepad: not in v1; defer.
- Lives: 3 starting, +1 every 10,000 points (per original).
- Score persistence: high score saved to `~/.local/share/asteroids/highscore` as a single integer. Nothing else persists.

## Game Constants (from original disassembly)

- Ship max velocity: ~6 units/sec (in original game-world units)
- Ship rotation rate: ~3 radians/sec
- Thrust acceleration: ~0.05 units/frame at 60Hz (scale to 1/240s sim step)
- Hyperspace cooldown: ~1 second between uses (prevents spam); self-destruct chance ~10% on exit
- Lives: 3 starting, +1 every 10,000 points, max display 6 lives onscreen
- Asteroid sizes: large (radius ~30), medium (~15), small (~7); each splits into 2 of the next size down
- Saucer: starts with large UFOs; each saucer spawn reduces the original reload timer, and once the reload byte's high bit clears later spawns can be small UFOs. After that point, small UFOs are guaranteed once score reaches 30,000; large UFOs fire randomly, small UFOs aim toward the ship.
- Attract mode: rolling demo of asteroids drifting + score table after 30s idle on title screen
- Tune all values against Norbert Kehrer's disassembly during step 11

## Open Questions (decided during build)

1. **Aspect ratio strategy** — three live options, decided between steps 5 and 6:
   - (a) Letterbox the original 4:3 playfield with black bars on the sides
   - (b) Stretch the playfield to widescreen (more space, asteroids spread out, slightly easier game)
   - (c) Keep gameplay 4:3 in the center, fill side margins with vector-art bezels (score + lives readouts only for v1; no cabinet art). Most museum-piece feel; modest extra code if scoped.
   - Default lean: (c) scoped to score+lives only.
2. **Color: pure white-on-black, or warm phosphor green/amber tint?** Original arcade was pure white but CRT phosphors had slight warm cast. Tune by feel during step 6.
3. **Persistence-of-vision target** — phosphor decay time-constant (`tau`) in the 50-100ms range. Tune live with a debug slider.
4. **Heartbeat behavior** — original couples tempo to remaining-asteroid count (verified via Norbert Kehrer's Asteroids disassembly / computerarcheology.com listing). Confirm exact tempo curve when implementing step 13.
5. **CRT curvature on a vector tube — authentic, or raster-CRT aesthetic creep?** Vector XY monitors had less geometric distortion than raster CRTs. Heavy barrel distortion may be unfaithful. Suggest: skip curvature for v1, ship without it; add subtly later only if reference photos of original Asteroids cabinets warrant it.
6. **Hyperspace teleport randomness** — original had a small chance of self-destruct on hyperspace exit. Faithful: keep. Annoying: drop. Suggest: keep, it's part of the feel.
7. **Collision model** — circle vs circle using sprite-extent radii (modern, fast, accurate enough). Confirm before step 12.
8. **Reference disassembly** — Norbert Kehrer's Asteroids disassembly and computerarcheology.com's annotated listing are the canonical references for original spawn patterns, UFO AI, and audio timing. Bookmark before step 11.

## Success Criteria

- Spinning wireframe ship on black background with phosphor glow that visibly trails behind motion. Soul of the project visible by milestone 1.
- Thrust sound humming through speakers as you press up — generated by real synthesis, not sample playback.
- Asteroids breaking into smaller asteroids correctly when shot (3 size tiers per the original).
- UFO appears, hunts the player with original-feeling AI, makes the iconic siren sound.
- Heartbeat audio tempo speeds up as round progresses, stays running between deaths.
- Frame rate sits at 144Hz on the user's monitor with no judder during gameplay.
- The user can sit down and play it for 10 minutes straight without the museum-piece illusion breaking. (The real test.)
