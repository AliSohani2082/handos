use crate::{FLAG_WORLD, MSG_FRAME, PROTOCOL, ProtoError};

const COUNT: usize = 21;
const HEADER: usize = 25;
const HAND: usize = 268;
const WORLD: usize = 252;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handedness {
    Left = 0,
    Right = 1,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Hand {
    pub track_id: u32,
    pub handedness: Handedness,
    pub handedness_score: f32,
    pub presence: f32,
    pub landmarks: [[f32; 3]; COUNT],
    pub world: Option<[[f32; 3]; COUNT]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandFrame {
    pub flags: u8,
    pub seq: u32,
    pub t_capture_us: u64,
    pub t_emit_us: u64,
    pub hands: Vec<Hand>,
}

fn hand_size(flags: u8, count: usize) -> Result<usize, ProtoError> {
    if count > 4 {
        return Err(ProtoError::HandCount(count));
    }
    match flags {
        0 => Ok(HAND),
        FLAG_WORLD => Ok(HAND + WORLD),
        _ => Err(ProtoError::Flags(flags)),
    }
}

fn floats(hand: &Hand) -> impl Iterator<Item = f32> + '_ {
    [hand.handedness_score, hand.presence]
        .into_iter()
        .chain(hand.landmarks.iter().flatten().copied())
        .chain(hand.world.iter().flatten().flatten().copied())
}

/// Encodes the full sequenced packet, including the leading message type.
pub fn encode_frame(frame: &HandFrame, out: &mut Vec<u8>) -> Result<(), ProtoError> {
    let size = hand_size(frame.flags, frame.hands.len())?;
    for hand in &frame.hands {
        if hand.world.is_some() != (frame.flags & FLAG_WORLD != 0) {
            return Err(ProtoError::World);
        }
        if !floats(hand).all(f32::is_finite) {
            return Err(ProtoError::NonFinite);
        }
    }

    out.clear();
    out.reserve(HEADER + size * frame.hands.len());
    out.extend([MSG_FRAME, PROTOCOL, frame.flags, frame.hands.len() as u8, 0]);
    out.extend(frame.seq.to_le_bytes());
    out.extend(frame.t_capture_us.to_le_bytes());
    out.extend(frame.t_emit_us.to_le_bytes());
    for hand in &frame.hands {
        out.extend(hand.track_id.to_le_bytes());
        out.extend([hand.handedness as u8, 0, 0, 0]);
        for value in floats(hand) {
            out.extend(value.to_le_bytes());
        }
    }
    Ok(())
}

fn read<const N: usize>(bytes: &[u8], offset: usize) -> [u8; N] {
    // The caller checks the size of the complete header or hand first.
    std::array::from_fn(|i| bytes[offset + i])
}

fn points(bytes: &[u8], offset: usize) -> [[f32; 3]; COUNT] {
    std::array::from_fn(|i| {
        std::array::from_fn(|axis| f32::from_le_bytes(read(bytes, offset + 12 * i + 4 * axis)))
    })
}

pub fn decode_frame(packet: &[u8]) -> Result<HandFrame, ProtoError> {
    if packet.len() < HEADER {
        return Err(ProtoError::Length);
    }
    if packet[0] != MSG_FRAME {
        return Err(ProtoError::MessageType);
    }
    if packet[1] != PROTOCOL {
        return Err(ProtoError::Version(packet[1]));
    }
    let flags = packet[2];
    let count = usize::from(packet[3]);
    let size = hand_size(flags, count)?;
    if packet.len() != HEADER + size * count {
        return Err(ProtoError::Length);
    }
    let mut hands = Vec::with_capacity(count);
    for bytes in packet[HEADER..].chunks_exact(size) {
        let hand = Hand {
            track_id: u32::from_le_bytes(read(bytes, 0)),
            handedness: match bytes[4] {
                0 => Handedness::Left,
                1 => Handedness::Right,
                value => return Err(ProtoError::Handedness(value)),
            },
            handedness_score: f32::from_le_bytes(read(bytes, 8)),
            presence: f32::from_le_bytes(read(bytes, 12)),
            landmarks: points(bytes, 16),
            world: (flags & FLAG_WORLD != 0).then(|| points(bytes, HAND)),
        };
        if !floats(&hand).all(f32::is_finite) {
            return Err(ProtoError::NonFinite);
        }
        hands.push(hand);
    }
    Ok(HandFrame {
        flags,
        seq: u32::from_le_bytes(read(packet, 5)),
        t_capture_us: u64::from_le_bytes(read(packet, 9)),
        t_emit_us: u64::from_le_bytes(read(packet, 17)),
        hands,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn frame(count: usize, world: bool, val: f32) -> HandFrame {
        HandFrame {
            flags: if world { FLAG_WORLD } else { 0 },
            seq: 24,
            t_capture_us: 23_000,
            t_emit_us: 24_321,
            hands: vec![
                Hand {
                    track_id: 5,
                    handedness: Handedness::Right,
                    handedness_score: val,
                    presence: val,
                    landmarks: [[val; 3]; COUNT],
                    world: world.then_some([[val; 3]; COUNT]),
                };
                count
            ],
        }
    }

    #[test]
    fn packet_layout_and_counts() {
        for count in [0, 1, 2, 4] {
            for world in [false, true] {
                let original = frame(count, world, 0.75);
                let mut packet = Vec::new();
                encode_frame(&original, &mut packet).unwrap();
                assert_eq!(packet.len(), 25 + count * if world { 520 } else { 268 });
                assert_eq!(decode_frame(&packet).unwrap(), original);
            }
        }
    }

    #[test]
    fn rejects_malformed_packets() {
        let mut packet = Vec::new();
        encode_frame(&frame(1, false, 0.5), &mut packet).unwrap();
        for (offset, value) in [(0, 2), (1, 2), (2, 2), (3, 5), (29, 2)] {
            let mut corrupt = packet.clone();
            corrupt[offset] = value;
            assert!(decode_frame(&corrupt).is_err(), "byte {offset}");
        }
        assert!(matches!(
            decode_frame(&packet[..packet.len() - 1]),
            Err(ProtoError::Length)
        ));
        packet[33..37].copy_from_slice(&f32::INFINITY.to_le_bytes());
        assert!(matches!(decode_frame(&packet), Err(ProtoError::NonFinite)));
        assert!(matches!(
            encode_frame(&frame(1, true, f32::NAN), &mut packet),
            Err(ProtoError::NonFinite)
        ));
    }

    proptest! {
        #[test]
        fn arbitrary_finite_coordinates(bits in any::<u32>(), count in 0..=4_usize, world in any::<bool>()) {
            let value = f32::from_bits(bits);
            prop_assume!(value.is_finite());
            let original = frame(count, world, value);
            let mut packet = Vec::new();
            encode_frame(&original, &mut packet).unwrap();
            prop_assert_eq!(decode_frame(&packet).unwrap(), original);
        }
    }
}
