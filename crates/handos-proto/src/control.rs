use crate::{MSG_CONTROL, ProtoError};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub const MAX_PACKET: usize = 65_536;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CameraInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TrackerMsg {
    Hello {
        protocol: u8,
        name: String,
        max_hands: u8,
        world: bool,
        camera: CameraInfo,
    },
    Status {
        fps: f32,
        infer_ms: f32,
        dropped: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackerMode {
    Active,
    Idle,
    Off,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonMsg {
    Set {
        mode: TrackerMode,
        #[serde(skip_serializing_if = "Option::is_none")]
        fps: Option<f32>,
    },
    Shutdown,
}

pub fn encode_control(message: &impl Serialize) -> Result<Vec<u8>, ProtoError> {
    let mut packet = vec![MSG_CONTROL];
    serde_json::to_writer(&mut packet, message)?;
    if packet.len() > MAX_PACKET {
        return Err(ProtoError::Length);
    }
    Ok(packet)
}

pub fn decode_control<T: DeserializeOwned>(packet: &[u8]) -> Result<T, ProtoError> {
    if packet.len() > MAX_PACKET {
        return Err(ProtoError::Length);
    }
    match packet.split_first() {
        Some((&MSG_CONTROL, json)) => Ok(serde_json::from_slice(json)?),
        _ => Err(ProtoError::MessageType),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_message_omits_unspecified_rate() {
        let msg = DaemonMsg::Set {
            mode: TrackerMode::Active,
            fps: None,
        };
        let bytes = encode_control(&msg).unwrap();
        assert_eq!(bytes, b"\x02{\"type\":\"set\",\"mode\":\"active\"}");
        assert_eq!(decode_control::<DaemonMsg>(&bytes).unwrap(), msg);
        let with_rate = b"\x02{\"type\":\"set\",\"mode\":\"idle\",\"fps\":5}";
        assert_eq!(
            decode_control::<DaemonMsg>(with_rate).unwrap(),
            DaemonMsg::Set {
                mode: TrackerMode::Idle,
                fps: Some(5.0)
            }
        );
    }
}
