#![allow(dead_code)]

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    },
};

use bytemuck::{Pod, Zeroable};
use fundsp::prelude::Shared;
use ringbuf::{
    Cons, HeapProd, HeapRb,
    traits::{Consumer, Observer, Producer, Split},
};

pub const AUDIO_MSG_CAPACITY: usize = 1024;
type AudioRing = Arc<HeapRb<AudioMsg>>;

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct VoiceId(pub u16);

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Pod, Zeroable)]
pub struct ParamId(pub u16);

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct GameSnapshot {
    pub asteroid_count: u32,
    pub alive: u32,
    pub score: u32,
}

impl GameSnapshot {
    pub fn new(asteroid_count: u32, alive: bool, score: u32) -> Self {
        Self {
            asteroid_count,
            alive: u32::from(alive),
            score,
        }
    }

    pub fn is_alive(self) -> bool {
        self.alive != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioMsg {
    SetParam(VoiceId, ParamId, f32),
    Trigger(VoiceId),
    Release(VoiceId),
    GameState(GameSnapshot),
}

impl AudioMsg {
    fn is_set_param(self) -> bool {
        matches!(self, Self::SetParam(_, _, _))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PushOutcome {
    Enqueued,
    DroppedOldestSetParam(AudioMsg),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PushError {
    Full(AudioMsg),
}

pub struct AudioMsgSender {
    producer: HeapProd<AudioMsg>,
    ring: AudioRing,
    dropped_set_params: AtomicU64,
}

impl AudioMsgSender {
    pub fn try_push(&mut self, msg: AudioMsg) -> Result<PushOutcome, PushError> {
        match self.producer.try_push(msg) {
            Ok(()) => Ok(PushOutcome::Enqueued),
            Err(msg) => self.push_after_dropping_oldest_set_param(msg),
        }
    }

    pub fn dropped_set_params(&self) -> u64 {
        self.dropped_set_params.load(Ordering::Relaxed)
    }

    fn push_after_dropping_oldest_set_param(
        &mut self,
        msg: AudioMsg,
    ) -> Result<PushOutcome, PushError> {
        if let Some(dropped) = self.drop_front_set_param() {
            self.dropped_set_params.fetch_add(1, Ordering::Relaxed);
            self.producer
                .try_push(msg)
                .map(|()| PushOutcome::DroppedOldestSetParam(dropped))
                .map_err(PushError::Full)
        } else if let Some(dropped) = self.replace_oldest_set_param(msg) {
            self.dropped_set_params.fetch_add(1, Ordering::Relaxed);
            Ok(PushOutcome::DroppedOldestSetParam(dropped))
        } else {
            Err(PushError::Full(msg))
        }
    }

    fn drop_front_set_param(&self) -> Option<AudioMsg> {
        let msg = self.front_msg()?;
        if !msg.is_set_param() {
            return None;
        }

        // AudioMsg is Copy and has no destructor; advancing read skips this queued parameter.
        unsafe {
            self.ring.advance_read_index(1);
        }
        Some(msg)
    }

    fn front_msg(&self) -> Option<AudioMsg> {
        let (left, right) = self.ring.occupied_slices();
        left.first()
            .or_else(|| right.first())
            .map(|slot| unsafe { *slot.assume_init_ref() })
    }

    fn replace_oldest_set_param(&self, replacement: AudioMsg) -> Option<AudioMsg> {
        let read = self.ring.read_index();
        let write = self.ring.write_index();
        let (left, right) = unsafe { self.ring.unsafe_slices_mut(read, write) };

        for slot in left.iter_mut().chain(right.iter_mut()) {
            let msg = unsafe { *slot.assume_init_ref() };
            if msg.is_set_param() {
                slot.write(replacement);
                return Some(msg);
            }
        }

        None
    }
}

pub struct AudioMsgReceiver {
    consumer: Cons<AudioRing>,
}

impl AudioMsgReceiver {
    pub fn try_pop(&mut self) -> Option<AudioMsg> {
        self.consumer.try_pop()
    }

    pub fn drain_into(&mut self, out: &mut Vec<AudioMsg>) {
        while let Some(msg) = self.try_pop() {
            out.push(msg);
        }
    }
}

pub fn audio_msg_channel() -> (AudioMsgSender, AudioMsgReceiver) {
    let ring = Arc::new(HeapRb::<AudioMsg>::new(AUDIO_MSG_CAPACITY));
    let (producer, _) = ring.clone().split();
    let consumer = Cons::new(ring.clone());
    (
        AudioMsgSender {
            producer,
            ring,
            dropped_set_params: AtomicU64::new(0),
        },
        AudioMsgReceiver { consumer },
    )
}

pub struct AtomicF32 {
    bits: AtomicU32,
}

impl AtomicF32 {
    pub fn new(value: f32) -> Self {
        Self {
            bits: AtomicU32::new(value.to_bits()),
        }
    }

    pub fn load(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    pub fn store(&self, value: f32) {
        self.bits.store(value.to_bits(), Ordering::Relaxed);
    }
}

impl Default for AtomicF32 {
    fn default() -> Self {
        Self::new(0.0)
    }
}

impl fmt::Debug for AtomicF32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AtomicF32").field(&self.load()).finish()
    }
}

pub trait Voice {
    fn id(&self) -> VoiceId;
    fn trigger(&self);
    fn release(&self);
    fn set_param(&self, param: ParamId, value: f32) -> bool;
    fn sync_controls_for_audio_block(&self);
}

pub struct PreallocatedVoice {
    id: VoiceId,
    params: Vec<AtomicF32>,
    fundsp_controls: Vec<Shared>,
    gate: AtomicBool,
}

impl PreallocatedVoice {
    pub fn new(id: VoiceId, param_defaults: &[f32]) -> Self {
        Self {
            id,
            params: param_defaults.iter().copied().map(AtomicF32::new).collect(),
            fundsp_controls: param_defaults.iter().copied().map(Shared::new).collect(),
            gate: AtomicBool::new(false),
        }
    }

    pub fn param(&self, param: ParamId) -> Option<&AtomicF32> {
        self.params.get(usize::from(param.0))
    }

    pub fn is_triggered(&self) -> bool {
        self.gate.load(Ordering::Relaxed)
    }
}

impl Voice for PreallocatedVoice {
    fn id(&self) -> VoiceId {
        self.id
    }

    fn trigger(&self) {
        self.gate.store(true, Ordering::Relaxed);
    }

    fn release(&self) {
        self.gate.store(false, Ordering::Relaxed);
    }

    fn set_param(&self, param: ParamId, value: f32) -> bool {
        let Some(cell) = self.param(param) else {
            return false;
        };
        cell.store(value);
        true
    }

    fn sync_controls_for_audio_block(&self) {
        for (cell, control) in self.params.iter().zip(self.fundsp_controls.iter()) {
            control.set(cell.load());
        }
    }
}

pub struct VoiceBank {
    voices: Vec<PreallocatedVoice>,
}

impl VoiceBank {
    pub fn single_player_default() -> Self {
        Self {
            voices: vec![
                PreallocatedVoice::new(VOICE_THRUST, &[0.0, 0.0, 0.0, 0.0]),
                PreallocatedVoice::new(VOICE_FIRE, &[0.0, 0.0, 0.0]),
                PreallocatedVoice::new(VOICE_EXPLOSION, &[0.0, 0.0, 0.0, 0.0]),
                PreallocatedVoice::new(VOICE_UFO, &[0.0, 0.0, 0.0]),
                PreallocatedVoice::new(VOICE_HEARTBEAT, &[0.0, 0.0]),
            ],
        }
    }

    pub fn len(&self) -> usize {
        self.voices.len()
    }

    pub fn get(&self, id: VoiceId) -> Option<&PreallocatedVoice> {
        self.voices.iter().find(|voice| voice.id() == id)
    }
}

pub const VOICE_THRUST: VoiceId = VoiceId(0);
pub const VOICE_FIRE: VoiceId = VoiceId(1);
pub const VOICE_EXPLOSION: VoiceId = VoiceId(2);
pub const VOICE_UFO: VoiceId = VoiceId(3);
pub const VOICE_HEARTBEAT: VoiceId = VoiceId(4);

pub struct AudioScaffold {
    sender: AudioMsgSender,
    receiver: AudioMsgReceiver,
    voices: VoiceBank,
}

impl AudioScaffold {
    pub fn new() -> Self {
        let (sender, receiver) = audio_msg_channel();
        Self {
            sender,
            receiver,
            voices: VoiceBank::single_player_default(),
        }
    }

    pub fn sender(&mut self) -> &mut AudioMsgSender {
        &mut self.sender
    }

    pub fn receiver(&mut self) -> &mut AudioMsgReceiver {
        &mut self.receiver
    }

    pub fn voices(&self) -> &VoiceBank {
        &self.voices
    }
}

#[cfg(feature = "audio-scaffolding")]
#[derive(Debug, Clone, Copy)]
pub struct AudioStreamNotWired;

#[cfg(feature = "audio-scaffolding")]
impl fmt::Display for AudioStreamNotWired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cpal stream creation is reserved for the audio-scaffolding task")
    }
}

#[cfg(feature = "audio-scaffolding")]
impl std::error::Error for AudioStreamNotWired {}

#[cfg(feature = "audio-scaffolding")]
pub fn create_cpal_stream_placeholder() -> Result<cpal::Stream, AudioStreamNotWired> {
    Err(AudioStreamNotWired)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_ring_drops_oldest_set_param_and_enqueues_latest() {
        let (mut sender, mut receiver) = audio_msg_channel();

        for value in 0..AUDIO_MSG_CAPACITY {
            assert_eq!(
                sender.try_push(AudioMsg::SetParam(VOICE_THRUST, ParamId(0), value as f32)),
                Ok(PushOutcome::Enqueued)
            );
        }

        assert_eq!(
            sender.try_push(AudioMsg::SetParam(VOICE_THRUST, ParamId(0), 2048.0)),
            Ok(PushOutcome::DroppedOldestSetParam(AudioMsg::SetParam(
                VOICE_THRUST,
                ParamId(0),
                0.0
            )))
        );
        assert_eq!(sender.dropped_set_params(), 1);

        let mut drained = Vec::new();
        receiver.drain_into(&mut drained);
        assert_eq!(drained.len(), AUDIO_MSG_CAPACITY);
        assert!(matches!(
            drained.first(),
            Some(AudioMsg::SetParam(VOICE_THRUST, ParamId(0), 1.0))
        ));
        assert!(matches!(
            drained.last(),
            Some(AudioMsg::SetParam(VOICE_THRUST, ParamId(0), 2048.0))
        ));
    }

    #[test]
    fn full_ring_preserves_existing_and_incoming_critical_messages() {
        let (mut sender, mut receiver) = audio_msg_channel();

        assert_eq!(
            sender.try_push(AudioMsg::Trigger(VOICE_THRUST)),
            Ok(PushOutcome::Enqueued)
        );
        for value in 1..AUDIO_MSG_CAPACITY {
            assert_eq!(
                sender.try_push(AudioMsg::SetParam(VOICE_THRUST, ParamId(0), value as f32)),
                Ok(PushOutcome::Enqueued)
            );
        }

        assert_eq!(
            sender.try_push(AudioMsg::Release(VOICE_THRUST)),
            Ok(PushOutcome::DroppedOldestSetParam(AudioMsg::SetParam(
                VOICE_THRUST,
                ParamId(0),
                1.0
            )))
        );

        let mut drained = Vec::new();
        receiver.drain_into(&mut drained);
        assert_eq!(drained.len(), AUDIO_MSG_CAPACITY);
        assert!(matches!(
            drained.first(),
            Some(AudioMsg::Trigger(VOICE_THRUST))
        ));
        assert!(
            drained
                .iter()
                .any(|msg| matches!(msg, AudioMsg::Release(VOICE_THRUST)))
        );
        assert!(
            !drained
                .iter()
                .any(|msg| matches!(msg, AudioMsg::SetParam(VOICE_THRUST, ParamId(0), 1.0)))
        );
    }

    #[test]
    fn game_snapshot_is_pod_with_integer_alive_flag() {
        let snapshot = GameSnapshot::new(7, true, 10_000);
        assert_eq!(
            bytemuck::bytes_of(&snapshot).len(),
            size_of::<GameSnapshot>()
        );
        assert!(snapshot.is_alive());
    }
}
