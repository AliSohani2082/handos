use std::io::{ErrorKind, Read, Write};

use crate::{MSG_CONTROL, MSG_FRAME, ProtoError, control::MAX_PACKET};

const PREFIX: &[u8; 8] = b"HTRC\x01\0\0\0";

pub struct TraceWriter<W> {
    out: W,
}

impl<W: Write> TraceWriter<W> {
    pub fn new(mut out: W, t0_us: u64) -> Result<Self, ProtoError> {
        out.write_all(PREFIX)?;
        out.write_all(&t0_us.to_le_bytes())?;
        Ok(Self { out })
    }

    pub fn write_packet(&mut self, packet: &[u8]) -> Result<(), ProtoError> {
        check_len(packet.len())?;
        check_type(packet)?;
        self.out.write_all(&(packet.len() as u32).to_le_bytes())?;
        self.out.write_all(packet)?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), ProtoError> {
        Ok(self.out.flush()?)
    }
}

pub struct TraceReader<R> {
    input: R,
    pub t0_us: u64,
}

impl<R: Read> TraceReader<R> {
    pub fn new(mut input: R) -> Result<Self, ProtoError> {
        let mut prefix = [0; 8];
        read_exact(&mut input, &mut prefix)?;
        if &prefix != PREFIX {
            return Err(ProtoError::TraceHeader);
        }
        let mut time = [0; 8];
        read_exact(&mut input, &mut time)?;
        Ok(Self {
            input,
            t0_us: u64::from_le_bytes(time),
        })
    }

    /// Fills the caller's reusable buffer. Returns false at a record boundary at EOF.
    pub fn read_packet(&mut self, packet: &mut Vec<u8>) -> Result<bool, ProtoError> {
        let mut len = [0; 4];
        loop {
            match self.input.read(&mut len[..1]) {
                Ok(0) => return Ok(false),
                Ok(_) => break,
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                Err(err) => return Err(err.into()),
            }
        }
        read_exact(&mut self.input, &mut len[1..])?;
        let len = u32::from_le_bytes(len) as usize;
        check_len(len)?;
        packet.resize(len, 0);
        read_exact(&mut self.input, packet)?;
        check_type(packet)?;
        Ok(true)
    }
}

fn read_exact(input: &mut impl Read, buf: &mut [u8]) -> Result<(), ProtoError> {
    input.read_exact(buf).map_err(|err| match err.kind() {
        ErrorKind::UnexpectedEof => ProtoError::Truncated,
        _ => err.into(),
    })
}

fn check_len(len: usize) -> Result<(), ProtoError> {
    if !(1..=MAX_PACKET).contains(&len) {
        return Err(ProtoError::Length);
    }
    Ok(())
}

fn check_type(packet: &[u8]) -> Result<(), ProtoError> {
    match packet.first().copied() {
        Some(MSG_FRAME | MSG_CONTROL) => Ok(()),
        _ => Err(ProtoError::MessageType),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_truncation() {
        let mut trace = Vec::new();
        let mut writer = TraceWriter::new(&mut trace, 1234).unwrap();
        let hello = b"\x02{\"type\":\"shutdown\"}";
        writer.write_packet(hello).unwrap();
        writer.write_packet(&[MSG_FRAME, 1]).unwrap();

        let mut reader = TraceReader::new(trace.as_slice()).unwrap();
        let mut packet = Vec::new();
        assert_eq!(reader.t0_us, 1234);
        assert!(reader.read_packet(&mut packet).unwrap());
        assert_eq!(packet, hello);
        assert!(reader.read_packet(&mut packet).unwrap());
        assert_eq!(packet, [MSG_FRAME, 1]);
        assert!(!reader.read_packet(&mut packet).unwrap());

        for end in [1, 15] {
            assert!(matches!(
                TraceReader::new(&trace[..end]),
                Err(ProtoError::Truncated)
            ));
        }
        let mut reader = TraceReader::new(&trace[..trace.len() - 1]).unwrap();
        assert!(reader.read_packet(&mut packet).unwrap());
        assert!(matches!(
            reader.read_packet(&mut packet),
            Err(ProtoError::Truncated)
        ));
    }

    #[test]
    fn rejects_oversize_record_before_allocating() {
        let mut trace = Vec::new();
        TraceWriter::new(&mut trace, 0).unwrap();
        trace.extend(u32::MAX.to_le_bytes());
        let mut reader = TraceReader::new(trace.as_slice()).unwrap();
        let mut packet = Vec::new();
        assert!(matches!(
            reader.read_packet(&mut packet),
            Err(ProtoError::Length)
        ));
        assert_eq!(packet.capacity(), 0);
    }
}
