//! Chrome native messaging uses a four-byte, native-endian length prefix
//! followed by one UTF-8 JSON value. The host and the service reuse the same
//! framing on their local socket.

use std::io::{self, Read, Write};

use anyhow::{Context, Result, bail};
use brauser_protocol::{ErrorCode, ErrorResponse, PROTOCOL_VERSION, Request, Response};

pub const MAX_INBOUND_BYTES: usize = 4 * 1024 * 1024;
// Chrome limits host-to-extension messages to 1 MiB. Leave room for the
// framing prefix and future envelope fields. Note limits (`note.rs`) are
// derived from this so a saved note can always be loaded again.
pub const MAX_OUTBOUND_BYTES: usize = 900 * 1024;

#[derive(Debug)]
pub enum FrameError {
    TooLarge,
    InvalidLength,
    Io(io::Error),
}

pub fn read_frame<R: Read>(input: &mut R) -> std::result::Result<Option<Vec<u8>>, FrameError> {
    let mut prefix = [0u8; 4];
    let received = input.read(&mut prefix[..1]).map_err(FrameError::Io)?;
    if received == 0 {
        return Ok(None);
    }
    input.read_exact(&mut prefix[1..]).map_err(FrameError::Io)?;
    let size = u32::from_ne_bytes(prefix) as usize;
    if size == 0 {
        return Err(FrameError::InvalidLength);
    }
    if size > MAX_INBOUND_BYTES {
        return Err(FrameError::TooLarge);
    }
    let mut body = vec![0u8; size];
    input.read_exact(&mut body).map_err(FrameError::Io)?;
    Ok(Some(body))
}

pub fn write_frame<W: Write>(output: &mut W, response: &Response) -> Result<()> {
    let mut body = serde_json::to_vec(response).context("serializing native response")?;
    if body.len() > MAX_OUTBOUND_BYTES {
        // One oversized answer must not end the connection: that would drop
        // every other in-flight request and show the extension only a
        // disconnect. Answer the same request with a small error instead.
        let fallback = Response::Error(ErrorResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id: response.request_id().to_owned(),
            code: ErrorCode::MessageTooLarge,
            message: "response exceeds the host-to-extension size limit".to_owned(),
        });
        body = serde_json::to_vec(&fallback).context("serializing native response")?;
    }
    write_body(output, &body)
}

/// Frame a request for the service. The service reads it with `read_frame`,
/// so a request over `MAX_INBOUND_BYTES` would be refused there; refuse it
/// here instead, before writing anything.
pub fn write_request<W: Write>(output: &mut W, request: &Request) -> Result<()> {
    let body = serde_json::to_vec(request).context("serializing service request")?;
    if body.len() > MAX_INBOUND_BYTES {
        bail!("request exceeds the {MAX_INBOUND_BYTES}-byte frame limit");
    }
    write_body(output, &body)
}

/// Whether `write_request` would accept `request`, measured without
/// buffering it, so a caller can refuse an oversize request before it
/// connects to the service. A request that cannot be serialized counts as
/// fitting: `write_request` reports that failure itself.
pub fn request_fits(request: &Request) -> bool {
    struct Count(usize);
    impl Write for Count {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count(0);
    serde_json::to_writer(&mut count, request).map_or(true, |()| count.0 <= MAX_INBOUND_BYTES)
}

fn write_body<W: Write>(output: &mut W, body: &[u8]) -> Result<()> {
    let size = u32::try_from(body.len()).context("native message length overflow")?;
    output.write_all(&size.to_ne_bytes())?;
    output.write_all(body)?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use brauser_protocol::{
        ErrorCode, HelloRequest, NoteLoaded, PROTOCOL_VERSION, Request, Response,
    };
    use std::io::Cursor;

    fn hello() -> Request {
        Request::Hello(HelloRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "hello-1".into(),
        })
    }

    #[test]
    fn request_and_response_round_trip_native_endian() {
        let request = hello();
        let mut wire = Vec::new();
        write_request(&mut wire, &request).unwrap();
        let json = serde_json::to_vec(&request).unwrap();
        assert_eq!(&wire[..4], &(json.len() as u32).to_ne_bytes());
        let mut reader = Cursor::new(wire);
        let body = read_frame(&mut reader).unwrap().unwrap();
        assert_eq!(serde_json::from_slice::<Request>(&body).unwrap(), request);
        assert!(read_frame(&mut reader).unwrap().is_none());

        let response = Response::Error(brauser_protocol::ErrorResponse {
            protocol_version: PROTOCOL_VERSION,
            request_id: "hello-1".into(),
            code: ErrorCode::Internal,
            message: "x".into(),
        });
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        let json = serde_json::to_vec(&response).unwrap();
        assert_eq!(&wire[..4], &(json.len() as u32).to_ne_bytes());
        let body = read_frame(&mut Cursor::new(wire)).unwrap().unwrap();
        assert_eq!(serde_json::from_slice::<Response>(&body).unwrap(), response);
    }

    #[test]
    fn oversized_response_becomes_message_too_large() {
        let response = Response::NoteLoaded(NoteLoaded {
            protocol_version: PROTOCOL_VERSION,
            request_id: "load-big".into(),
            exists: true,
            revision: "missing".into(),
            title: String::new(),
            body: "x".repeat(MAX_OUTBOUND_BYTES + 1),
            key: crate::vault::url_record_key("https://example.com/a").unwrap(),
            older_notes: Vec::new(),
        });
        let mut wire = Vec::new();
        write_frame(&mut wire, &response).unwrap();
        assert!(wire.len() <= MAX_OUTBOUND_BYTES + 4);
        let body = read_frame(&mut Cursor::new(wire)).unwrap().unwrap();
        match serde_json::from_slice::<Response>(&body).unwrap() {
            Response::Error(value) => {
                assert_eq!(value.code, ErrorCode::MessageTooLarge);
                assert_eq!(value.request_id, "load-big");
            }
            other => panic!("expected message_too_large, got {other:?}"),
        }
    }

    #[test]
    fn oversized_request_is_an_error_and_writes_nothing() {
        let request = Request::Hello(HelloRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "r".repeat(MAX_INBOUND_BYTES + 1),
        });
        let mut wire = Vec::new();
        assert!(write_request(&mut wire, &request).is_err());
        assert!(wire.is_empty());
    }

    /// request_fits agrees with write_request on both sides of the cap.
    #[test]
    fn request_fits_matches_write_request_at_the_cap() {
        let sized = |len: usize| {
            let mut value = HelloRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: String::new(),
            };
            let base = serde_json::to_vec(&Request::Hello(value.clone()))
                .unwrap()
                .len();
            value.request_id = "r".repeat(len - base);
            Request::Hello(value)
        };
        let at_cap = sized(MAX_INBOUND_BYTES);
        assert_eq!(
            serde_json::to_vec(&at_cap).unwrap().len(),
            MAX_INBOUND_BYTES
        );
        assert!(request_fits(&at_cap));
        assert!(write_request(&mut Vec::new(), &at_cap).is_ok());
        let over = sized(MAX_INBOUND_BYTES + 1);
        assert!(!request_fits(&over));
        assert!(write_request(&mut Vec::new(), &over).is_err());
        assert!(request_fits(&hello()));
    }

    #[test]
    fn zero_length_frame_is_invalid() {
        let wire = 0u32.to_ne_bytes();
        assert!(matches!(
            read_frame(&mut Cursor::new(wire)),
            Err(FrameError::InvalidLength)
        ));
    }

    #[test]
    fn oversized_frame_is_too_large_before_reading_its_body() {
        let wire = ((MAX_INBOUND_BYTES + 1) as u32).to_ne_bytes();
        assert!(matches!(
            read_frame(&mut Cursor::new(wire)),
            Err(FrameError::TooLarge)
        ));
    }
}
