use std::io::{self, Read, Write};

/// Messages sent from server to client.
pub enum ServerMsg {
    /// PTY output bytes (also used for initial screen dump)
    Output(Vec<u8>),
    /// PTY size changed (sent on connect + when other clients cause resize)
    Resized { cols: u16, rows: u16 },
    /// Child exited with code
    Exited(i32),
}

/// Messages sent from client to server.
pub enum ClientMsg {
    /// Raw bytes for PTY
    Input(Vec<u8>),
    /// Client terminal size (inner_rows already minus 1 for header)
    Resize { cols: u16, rows: u16 },
    /// Kill child process
    Kill,
}

// Wire format: [u8 tag][u32 BE payload_len][payload]
//
// Server→Client tags:
//   0x01 = Output(payload)
//   0x02 = Resized(cols: u16 BE, rows: u16 BE)
//   0x03 = Exited(code: i32 BE)
//
// Client→Server tags:
//   0x11 = Input(payload)
//   0x12 = Resize(cols: u16 BE, rows: u16 BE)
//   0x13 = Kill (no payload)

fn write_frame(w: &mut impl Write, tag: u8, payload: &[u8]) -> io::Result<()> {
    w.write_all(&[tag])?;
    w.write_all(&(payload.len() as u32).to_be_bytes())?;
    if !payload.is_empty() {
        w.write_all(payload)?;
    }
    w.flush()
}

/// Maximum payload size (16 MB) to prevent OOM on corrupted frames.
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

fn read_frame(r: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut tag_buf = [0u8; 1];
    r.read_exact(&mut tag_buf)?;
    let tag = tag_buf[0];

    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("payload too large: {} bytes (max {})", len, MAX_PAYLOAD),
        ));
    }

    let mut payload = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut payload)?;
    }

    Ok((tag, payload))
}

pub fn write_server_msg(w: &mut impl Write, msg: &ServerMsg) -> io::Result<()> {
    match msg {
        ServerMsg::Output(data) => write_frame(w, 0x01, data),
        ServerMsg::Resized { cols, rows } => {
            let mut buf = [0u8; 4];
            buf[0..2].copy_from_slice(&cols.to_be_bytes());
            buf[2..4].copy_from_slice(&rows.to_be_bytes());
            write_frame(w, 0x02, &buf)
        }
        ServerMsg::Exited(code) => write_frame(w, 0x03, &code.to_be_bytes()),
    }
}

pub fn read_server_msg(r: &mut impl Read) -> io::Result<ServerMsg> {
    let (tag, payload) = read_frame(r)?;
    match tag {
        0x01 => Ok(ServerMsg::Output(payload)),
        0x02 => {
            if payload.len() < 4 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short Resized"));
            }
            let cols = u16::from_be_bytes([payload[0], payload[1]]);
            let rows = u16::from_be_bytes([payload[2], payload[3]]);
            Ok(ServerMsg::Resized { cols, rows })
        }
        0x03 => {
            if payload.len() < 4 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short Exited"));
            }
            let code = i32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
            Ok(ServerMsg::Exited(code))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown server tag: 0x{:02x}", tag),
        )),
    }
}

pub fn write_client_msg(w: &mut impl Write, msg: &ClientMsg) -> io::Result<()> {
    match msg {
        ClientMsg::Input(data) => write_frame(w, 0x11, data),
        ClientMsg::Resize { cols, rows } => {
            let mut buf = [0u8; 4];
            buf[0..2].copy_from_slice(&cols.to_be_bytes());
            buf[2..4].copy_from_slice(&rows.to_be_bytes());
            write_frame(w, 0x12, &buf)
        }
        ClientMsg::Kill => write_frame(w, 0x13, &[]),
    }
}

/// Serialize a ServerMsg into a self-contained byte buffer (tag + length + payload).
/// Used for pre-serializing messages before sending to per-client writer threads.
pub fn serialize_server_msg(msg: &ServerMsg) -> Vec<u8> {
    let mut buf = Vec::new();
    write_server_msg(&mut buf, msg).unwrap(); // Vec<u8> write never fails
    buf
}

/// Serialize an Output message from a borrowed slice, avoiding a clone of the
/// underlying data.  Used to send the history buffer without copying it.
pub fn serialize_output_slice(data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(5 + data.len());
    write_frame(&mut buf, 0x01, data).unwrap();
    buf
}

pub fn read_client_msg(r: &mut impl Read) -> io::Result<ClientMsg> {
    let (tag, payload) = read_frame(r)?;
    match tag {
        0x11 => Ok(ClientMsg::Input(payload)),
        0x12 => {
            if payload.len() < 4 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "short Resize"));
            }
            let cols = u16::from_be_bytes([payload[0], payload[1]]);
            let rows = u16::from_be_bytes([payload[2], payload[3]]);
            Ok(ClientMsg::Resize { cols, rows })
        }
        0x13 => Ok(ClientMsg::Kill),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown client tag: 0x{:02x}", tag),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_server_output_roundtrip() {
        let msg = ServerMsg::Output(b"hello world".to_vec());
        let mut buf = Vec::new();
        write_server_msg(&mut buf, &msg).unwrap();
        let decoded = read_server_msg(&mut &buf[..]).unwrap();
        match decoded {
            ServerMsg::Output(data) => assert_eq!(data, b"hello world"),
            _ => panic!("expected Output"),
        }
    }

    #[test]
    fn test_server_resized_roundtrip() {
        let msg = ServerMsg::Resized {
            cols: 120,
            rows: 40,
        };
        let mut buf = Vec::new();
        write_server_msg(&mut buf, &msg).unwrap();
        let decoded = read_server_msg(&mut &buf[..]).unwrap();
        match decoded {
            ServerMsg::Resized { cols, rows } => {
                assert_eq!(cols, 120);
                assert_eq!(rows, 40);
            }
            _ => panic!("expected Resized"),
        }
    }

    #[test]
    fn test_server_exited_roundtrip() {
        let msg = ServerMsg::Exited(42);
        let mut buf = Vec::new();
        write_server_msg(&mut buf, &msg).unwrap();
        let decoded = read_server_msg(&mut &buf[..]).unwrap();
        match decoded {
            ServerMsg::Exited(code) => assert_eq!(code, 42),
            _ => panic!("expected Exited"),
        }
    }

    #[test]
    fn test_client_input_roundtrip() {
        let msg = ClientMsg::Input(b"keystrokes".to_vec());
        let mut buf = Vec::new();
        write_client_msg(&mut buf, &msg).unwrap();
        let decoded = read_client_msg(&mut &buf[..]).unwrap();
        match decoded {
            ClientMsg::Input(data) => assert_eq!(data, b"keystrokes"),
            _ => panic!("expected Input"),
        }
    }

    #[test]
    fn test_client_resize_roundtrip() {
        let msg = ClientMsg::Resize { cols: 80, rows: 24 };
        let mut buf = Vec::new();
        write_client_msg(&mut buf, &msg).unwrap();
        let decoded = read_client_msg(&mut &buf[..]).unwrap();
        match decoded {
            ClientMsg::Resize { cols, rows } => {
                assert_eq!(cols, 80);
                assert_eq!(rows, 24);
            }
            _ => panic!("expected Resize"),
        }
    }

    #[test]
    fn test_client_kill_roundtrip() {
        let msg = ClientMsg::Kill;
        let mut buf = Vec::new();
        write_client_msg(&mut buf, &msg).unwrap();
        let decoded = read_client_msg(&mut &buf[..]).unwrap();
        assert!(matches!(decoded, ClientMsg::Kill));
    }

    #[test]
    fn test_server_output_empty() {
        let msg = ServerMsg::Output(vec![]);
        let mut buf = Vec::new();
        write_server_msg(&mut buf, &msg).unwrap();
        let decoded = read_server_msg(&mut &buf[..]).unwrap();
        match decoded {
            ServerMsg::Output(data) => assert!(data.is_empty()),
            _ => panic!("expected Output"),
        }
    }

    #[test]
    fn test_server_exited_negative() {
        let msg = ServerMsg::Exited(-1);
        let mut buf = Vec::new();
        write_server_msg(&mut buf, &msg).unwrap();
        let decoded = read_server_msg(&mut &buf[..]).unwrap();
        match decoded {
            ServerMsg::Exited(code) => assert_eq!(code, -1),
            _ => panic!("expected Exited"),
        }
    }

    #[test]
    fn test_unknown_server_tag() {
        let buf = vec![0xFF, 0, 0, 0, 0]; // unknown tag, 0-length payload
        let result = read_server_msg(&mut &buf[..]);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_client_tag() {
        let buf = vec![0xFF, 0, 0, 0, 0];
        let result = read_client_msg(&mut &buf[..]);
        assert!(result.is_err());
    }
}
