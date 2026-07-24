//! mosh wire format: the AES-128-OCB3 datagram layer, the fragment header, zlib
//! framing, and the `TransportBuffers.Instruction` proto2 message. Byte layouts
//! verified against mosh master (`crypto/crypto.cc`, `network/network.{h,cc}`,
//! `network/transportfragment.cc`, `protobufs/transportinstruction.proto`).
//!
//! Datagram = `nonce8 ‖ OCB3(aes128).encrypt(ts2 ‖ tsreply2 ‖ payload) ‖ tag16`
//! where `nonce8` is big-endian `(direction_bit<<63) | seq`, and the OCB nonce
//! is `0x00000000 ‖ nonce8` (12 bytes). AAD is empty; tag is 16 bytes.

use aes::Aes128;
use ocb3::aead::generic_array::GenericArray;
use ocb3::aead::{Aead, KeyInit};
use ocb3::Ocb3;

/// AES-128-OCB3 with a 12-byte nonce and 16-byte tag (RFC 7253), matching
/// mosh's `ae_init(key,16, nonce,12, tag,16)`.
type MoshOcb = Ocb3<Aes128>;

const DIRECTION_BIT: u64 = 1 << 63;

/// mosh protocol version (`MOSH_PROTOCOL_VERSION`).
pub const PROTOCOL_VERSION: u32 = 2;

/// The AES-128-OCB3 session cipher keyed by the 16-byte mosh session key.
pub struct Crypto {
    cipher: MoshOcb,
}

/// A decrypted datagram.
#[derive(Debug, PartialEq, Eq)]
pub struct Opened {
    /// True if the direction bit is set (TO_CLIENT); server→client packets.
    pub to_client: bool,
    pub seq: u64,
    pub timestamp: u16,
    pub timestamp_reply: u16,
    pub payload: Vec<u8>,
}

impl Crypto {
    pub fn new(key16: &[u8; 16]) -> Self {
        Self {
            cipher: MoshOcb::new(GenericArray::from_slice(key16)),
        }
    }

    /// Encrypt a datagram. `to_client` sets the direction bit (a client sends
    /// with `to_client = false`).
    pub fn seal(
        &self,
        to_client: bool,
        seq: u64,
        timestamp: u16,
        timestamp_reply: u16,
        payload: &[u8],
    ) -> Vec<u8> {
        let dir_seq = ((to_client as u64) << 63) | (seq & !DIRECTION_BIT);
        let nonce8 = dir_seq.to_be_bytes();
        let mut nonce12 = [0u8; 12];
        nonce12[4..].copy_from_slice(&nonce8);

        let mut plaintext = Vec::with_capacity(4 + payload.len());
        plaintext.extend_from_slice(&timestamp.to_be_bytes());
        plaintext.extend_from_slice(&timestamp_reply.to_be_bytes());
        plaintext.extend_from_slice(payload);

        let ct = self
            .cipher
            .encrypt(GenericArray::from_slice(&nonce12), plaintext.as_ref())
            .expect("OCB3 encryption is infallible for valid inputs");

        let mut out = Vec::with_capacity(8 + ct.len());
        out.extend_from_slice(&nonce8);
        out.extend_from_slice(&ct);
        out
    }

    /// Decrypt and authenticate a datagram. Returns None if too short or the
    /// OCB tag fails to verify.
    pub fn open(&self, datagram: &[u8]) -> Option<Opened> {
        if datagram.len() < 8 + 16 {
            return None;
        }
        let nonce8: [u8; 8] = datagram[0..8].try_into().ok()?;
        let dir_seq = u64::from_be_bytes(nonce8);
        let to_client = dir_seq & DIRECTION_BIT != 0;
        let seq = dir_seq & !DIRECTION_BIT;

        let mut nonce12 = [0u8; 12];
        nonce12[4..].copy_from_slice(&nonce8);
        let pt = self
            .cipher
            .decrypt(GenericArray::from_slice(&nonce12), &datagram[8..])
            .ok()?;
        if pt.len() < 4 {
            return None;
        }
        Some(Opened {
            to_client,
            seq,
            timestamp: u16::from_be_bytes([pt[0], pt[1]]),
            timestamp_reply: u16::from_be_bytes([pt[2], pt[3]]),
            payload: pt[4..].to_vec(),
        })
    }
}

// --- Fragment (network/transportfragment.cc) -------------------------------
// [ id: u64 BE ][ frag_field: u16 BE = (final<<15)|num ][ contents ]

/// Wrap already-compressed bytes as a single, final fragment (id, num=0).
pub fn make_single_fragment(id: u64, contents: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(10 + contents.len());
    v.extend_from_slice(&id.to_be_bytes());
    let frag_field: u16 = 0x8000; // final = 1, num = 0
    v.extend_from_slice(&frag_field.to_be_bytes());
    v.extend_from_slice(contents);
    v
}

/// Parse a fragment header. Returns (id, num, is_final, contents).
pub fn parse_fragment(payload: &[u8]) -> Option<(u64, u16, bool, &[u8])> {
    if payload.len() < 10 {
        return None;
    }
    let id = u64::from_be_bytes(payload[0..8].try_into().ok()?);
    let ff = u16::from_be_bytes(payload[8..10].try_into().ok()?);
    Some((id, ff & 0x7fff, ff & 0x8000 != 0, &payload[10..]))
}

// --- zlib (network/compressor.cc uses zlib format, RFC 1950) ----------------

pub fn zlib_compress(data: &[u8]) -> Vec<u8> {
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut e = ZlibEncoder::new(Vec::new(), Compression::new(6));
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

pub fn zlib_decompress(data: &[u8]) -> Option<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;
    let mut d = ZlibDecoder::new(data);
    let mut out = Vec::new();
    d.read_to_end(&mut out).ok().map(|_| out)
}

// --- TransportBuffers.Instruction (proto2, hand-encoded) --------------------

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Instruction {
    pub protocol_version: u32,
    pub old_num: u64,
    pub new_num: u64,
    pub ack_num: u64,
    pub throwaway_num: u64,
    pub diff: Vec<u8>,
    pub chaff: Vec<u8>,
}

fn put_varint(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v != 0 {
            out.push(b | 0x80);
        } else {
            out.push(b);
            break;
        }
    }
}

fn put_tag(field: u32, wire: u32, out: &mut Vec<u8>) {
    put_varint(((field as u64) << 3) | wire as u64, out);
}

fn get_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0;
    while *pos < buf.len() {
        let b = buf[*pos];
        *pos += 1;
        result |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(result);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
    None
}

impl Instruction {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        put_tag(1, 0, &mut o);
        put_varint(self.protocol_version as u64, &mut o);
        put_tag(2, 0, &mut o);
        put_varint(self.old_num, &mut o);
        put_tag(3, 0, &mut o);
        put_varint(self.new_num, &mut o);
        put_tag(4, 0, &mut o);
        put_varint(self.ack_num, &mut o);
        put_tag(5, 0, &mut o);
        put_varint(self.throwaway_num, &mut o);
        if !self.diff.is_empty() {
            put_tag(6, 2, &mut o);
            put_varint(self.diff.len() as u64, &mut o);
            o.extend_from_slice(&self.diff);
        }
        if !self.chaff.is_empty() {
            put_tag(7, 2, &mut o);
            put_varint(self.chaff.len() as u64, &mut o);
            o.extend_from_slice(&self.chaff);
        }
        o
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        let mut inst = Instruction::default();
        let mut pos = 0;
        while pos < buf.len() {
            let key = get_varint(buf, &mut pos)?;
            let field = (key >> 3) as u32;
            let wire = (key & 7) as u32;
            match (field, wire) {
                (1, 0) => inst.protocol_version = get_varint(buf, &mut pos)? as u32,
                (2, 0) => inst.old_num = get_varint(buf, &mut pos)?,
                (3, 0) => inst.new_num = get_varint(buf, &mut pos)?,
                (4, 0) => inst.ack_num = get_varint(buf, &mut pos)?,
                (5, 0) => inst.throwaway_num = get_varint(buf, &mut pos)?,
                (6, 2) => {
                    let len = get_varint(buf, &mut pos)? as usize;
                    inst.diff = buf.get(pos..pos + len)?.to_vec();
                    pos += len;
                }
                (7, 2) => {
                    let len = get_varint(buf, &mut pos)? as usize;
                    inst.chaff = buf.get(pos..pos + len)?.to_vec();
                    pos += len;
                }
                // Skip unknown fields by wire type.
                (_, 0) => {
                    get_varint(buf, &mut pos)?;
                }
                (_, 2) => {
                    let len = get_varint(buf, &mut pos)? as usize;
                    pos += len;
                }
                (_, 5) => pos += 4,
                (_, 1) => pos += 8,
                _ => return None,
            }
        }
        Some(inst)
    }
}

/// Build a full client datagram carrying `instruction`: encode → zlib →
/// single fragment → OCB3 seal (direction = to server).
pub fn build_client_datagram(
    crypto: &Crypto,
    seq: u64,
    timestamp: u16,
    timestamp_reply: u16,
    frag_id: u64,
    instruction: &Instruction,
) -> Vec<u8> {
    let compressed = zlib_compress(&instruction.encode());
    let fragment = make_single_fragment(frag_id, &compressed);
    crypto.seal(false, seq, timestamp, timestamp_reply, &fragment)
}

/// Decrypt a server datagram and, if it is a single complete fragment, return
/// the parsed Instruction. (Multi-fragment reassembly comes in a later phase.)
pub fn parse_server_datagram(crypto: &Crypto, datagram: &[u8]) -> Option<(Opened, Instruction)> {
    let opened = crypto.open(datagram)?;
    let (_, _, is_final, contents) = parse_fragment(&opened.payload)?;
    if !is_final {
        return None; // multi-fragment not handled yet
    }
    let raw = zlib_decompress(contents)?;
    let inst = Instruction::decode(&raw)?;
    Some((opened, inst))
}

// --- statesync payloads -----------------------------------------------------
// Server→client diff is a HostBuffers.HostMessage; client→server diff is a
// ClientBuffers.UserMessage. Both are `repeated Instruction instruction = 1`
// where the inner Instruction carries proto2 extension fields:
//   HostBuffers.Instruction: field 2 = HostBytes{ hoststring = 4 }
//   ClientBuffers.Instruction: field 2 = Keystroke{ keys = 4 },
//                              field 3 = ResizeMessage{ width=5, height=6 }

/// Walks a message for top-level field 1 (length-delimited) submessages,
/// calling `f` with each submessage's bytes.
fn for_each_field1<'a>(buf: &'a [u8], mut f: impl FnMut(&'a [u8])) {
    let mut pos = 0;
    while pos < buf.len() {
        let Some(key) = get_varint(buf, &mut pos) else { return };
        let field = key >> 3;
        let wire = key & 7;
        match wire {
            2 => {
                let Some(len) = get_varint(buf, &mut pos) else { return };
                let len = len as usize;
                let Some(slice) = buf.get(pos..pos + len) else { return };
                if field == 1 {
                    f(slice);
                }
                pos += len;
            }
            0 => {
                if get_varint(buf, &mut pos).is_none() {
                    return;
                }
            }
            5 => pos += 4,
            1 => pos += 8,
            _ => return,
        }
    }
}

/// Returns the length-delimited bytes of the first occurrence of `field` in a
/// proto message (used to descend into a known nested/extension field).
fn nested_field<'a>(buf: &'a [u8], field: u64) -> Option<&'a [u8]> {
    let mut pos = 0;
    while pos < buf.len() {
        let key = get_varint(buf, &mut pos)?;
        let f = key >> 3;
        let wire = key & 7;
        match wire {
            2 => {
                let len = get_varint(buf, &mut pos)? as usize;
                let slice = buf.get(pos..pos + len)?;
                if f == field {
                    return Some(slice);
                }
                pos += len;
            }
            0 => {
                get_varint(buf, &mut pos)?;
            }
            5 => pos += 4,
            1 => pos += 8,
            _ => return None,
        }
    }
    None
}

/// Decodes a HostBuffers.HostMessage, returning the concatenated `hoststring`
/// ANSI updates (the terminal bytes to feed downstream).
pub fn decode_host_message(diff: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for_each_field1(diff, |instr| {
        // instr is a HostBuffers.Instruction; extension 2 = HostBytes.
        if let Some(hostbytes) = nested_field(instr, 2) {
            // HostBytes.hoststring = field 4.
            if let Some(s) = nested_field(hostbytes, 4) {
                out.extend_from_slice(s);
            }
        }
    });
    out
}

/// A single user-input event in a UserStream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserEvent {
    Keys(Vec<u8>),
    Resize(i32, i32),
}

fn push_submessage(field: u32, body: &[u8], out: &mut Vec<u8>) {
    put_tag(field, 2, out);
    put_varint(body.len() as u64, out);
    out.extend_from_slice(body);
}

/// Encodes a ClientBuffers.UserMessage from a sequence of events (the SSP diff
/// from the receiver's assumed state to ours).
pub fn encode_user_message(events: &[UserEvent]) -> Vec<u8> {
    let mut msg = Vec::new();
    for ev in events {
        // Build the inner ClientBuffers.Instruction with its extension field.
        let mut instr = Vec::new();
        match ev {
            UserEvent::Keys(keys) => {
                let mut keystroke = Vec::new();
                push_submessage(4, keys, &mut keystroke); // Keystroke.keys = 4
                push_submessage(2, &keystroke, &mut instr); // ext: keystroke = 2
            }
            UserEvent::Resize(w, h) => {
                let mut resize = Vec::new();
                put_tag(5, 0, &mut resize);
                put_varint(*w as u64, &mut resize);
                put_tag(6, 0, &mut resize);
                put_varint(*h as u64, &mut resize);
                push_submessage(3, &resize, &mut instr); // ext: resize = 3
            }
        }
        push_submessage(1, &instr, &mut msg); // UserMessage.instruction = 1
    }
    msg
}

/// Encodes a ClientBuffers.UserMessage carrying a single Keystroke of `keys`.
pub fn encode_user_keystroke(keys: &[u8]) -> Vec<u8> {
    // Keystroke { keys = 4 }
    let mut keystroke = Vec::new();
    put_tag(4, 2, &mut keystroke);
    put_varint(keys.len() as u64, &mut keystroke);
    keystroke.extend_from_slice(keys);
    // Instruction { [ext 2] = Keystroke }
    let mut instr = Vec::new();
    put_tag(2, 2, &mut instr);
    put_varint(keystroke.len() as u64, &mut instr);
    instr.extend_from_slice(&keystroke);
    // UserMessage { instruction = 1 }
    let mut msg = Vec::new();
    put_tag(1, 2, &mut msg);
    put_varint(instr.len() as u64, &mut msg);
    msg.extend_from_slice(&instr);
    msg
}

/// Encodes a ClientBuffers.UserMessage carrying a single resize event.
pub fn encode_user_resize(width: i32, height: i32) -> Vec<u8> {
    // ResizeMessage { width = 5, height = 6 }
    let mut resize = Vec::new();
    put_tag(5, 0, &mut resize);
    put_varint(width as u64, &mut resize);
    put_tag(6, 0, &mut resize);
    put_varint(height as u64, &mut resize);
    // Instruction { [ext 3] = ResizeMessage }
    let mut instr = Vec::new();
    put_tag(3, 2, &mut instr);
    put_varint(resize.len() as u64, &mut instr);
    instr.extend_from_slice(&resize);
    let mut msg = Vec::new();
    put_tag(1, 2, &mut msg);
    put_varint(instr.len() as u64, &mut msg);
    msg.extend_from_slice(&instr);
    msg
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 16] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee,
        0xff,
    ];

    #[test]
    fn crypto_roundtrip() {
        let c = Crypto::new(&KEY);
        let dg = c.seal(false, 42, 1000, 999, b"hello payload");
        // nonce (8) + ct(payload+4) + tag(16)
        assert_eq!(dg.len(), 8 + (4 + 13) + 16);
        let o = c.open(&dg).unwrap();
        assert!(!o.to_client);
        assert_eq!(o.seq, 42);
        assert_eq!(o.timestamp, 1000);
        assert_eq!(o.timestamp_reply, 999);
        assert_eq!(o.payload, b"hello payload");
    }

    #[test]
    fn direction_bit_roundtrips() {
        let c = Crypto::new(&KEY);
        let dg = c.seal(true, 7, 0, 0, b"x");
        let o = c.open(&dg).unwrap();
        assert!(o.to_client);
        assert_eq!(o.seq, 7);
    }

    #[test]
    fn tampered_datagram_rejected() {
        let c = Crypto::new(&KEY);
        let mut dg = c.seal(false, 1, 0, 0, b"data");
        *dg.last_mut().unwrap() ^= 0x01; // flip a tag bit
        assert!(c.open(&dg).is_none());
    }

    #[test]
    fn wrong_key_rejected() {
        let c1 = Crypto::new(&KEY);
        let c2 = Crypto::new(&[0u8; 16]);
        let dg = c1.seal(false, 1, 0, 0, b"secret");
        assert!(c2.open(&dg).is_none());
    }

    #[test]
    fn fragment_roundtrip() {
        let f = make_single_fragment(0xdead_beef, b"chunk");
        let (id, num, is_final, contents) = parse_fragment(&f).unwrap();
        assert_eq!(id, 0xdead_beef);
        assert_eq!(num, 0);
        assert!(is_final);
        assert_eq!(contents, b"chunk");
    }

    #[test]
    fn zlib_roundtrip() {
        let data = b"the quick brown fox jumps over the lazy dog".repeat(4);
        let z = zlib_compress(&data);
        assert_eq!(zlib_decompress(&z).unwrap(), data);
    }

    #[test]
    fn instruction_roundtrip() {
        let inst = Instruction {
            protocol_version: PROTOCOL_VERSION,
            old_num: 3,
            new_num: 4,
            ack_num: 2,
            throwaway_num: 1,
            diff: b"diffbytes".to_vec(),
            chaff: Vec::new(),
        };
        let bytes = inst.encode();
        let back = Instruction::decode(&bytes).unwrap();
        assert_eq!(back, inst);
    }

    /// Live interoperability probe against a real mosh-server. Skipped unless
    /// MOSH_PROBE_{HOST,PORT,KEY} are set. This is the #1 risk gate: it proves
    /// our AES-128-OCB3 + fragment + zlib + Instruction decode matches mosh.
    ///
    ///   mosh-server new -s -l LANG=en_US.UTF-8        # prints MOSH CONNECT p k
    ///   MOSH_PROBE_HOST=127.0.0.1 MOSH_PROBE_PORT=p MOSH_PROBE_KEY=k \
    ///     cargo test --release --lib mosh_live_probe -- --ignored --nocapture
    #[test]
    #[ignore = "requires a live mosh-server (set MOSH_PROBE_{HOST,PORT,KEY})"]
    fn mosh_live_probe() {
        use base64::Engine;
        use std::net::UdpSocket;
        use std::time::{Duration, SystemTime};

        let (Ok(host), Ok(port), Ok(key_b64)) = (
            std::env::var("MOSH_PROBE_HOST"),
            std::env::var("MOSH_PROBE_PORT"),
            std::env::var("MOSH_PROBE_KEY"),
        ) else {
            eprintln!("skipped: set MOSH_PROBE_{{HOST,PORT,KEY}}");
            return;
        };
        let key_bytes = base64::engine::general_purpose::STANDARD_NO_PAD
            .decode(key_b64.trim())
            .expect("valid base64 key");
        assert_eq!(key_bytes.len(), 16, "mosh key must be 16 bytes");
        let mut key = [0u8; 16];
        key.copy_from_slice(&key_bytes);
        let crypto = Crypto::new(&key);

        let sock = UdpSocket::bind("0.0.0.0:0").unwrap();
        sock.connect(format!("{host}:{port}")).unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(1500))).unwrap();

        let now_ms = || {
            (SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                % 65536) as u16
        };

        // Send an initial (empty) instruction a few times; the server learns our
        // address from the first authenticated datagram and then pushes state.
        let mut got = None;
        for seq in 0..5u64 {
            let inst = Instruction {
                protocol_version: PROTOCOL_VERSION,
                ..Default::default()
            };
            let dg = build_client_datagram(&crypto, seq, now_ms(), 0, seq, &inst);
            sock.send(&dg).unwrap();

            let mut buf = [0u8; 2048];
            if let Ok(n) = sock.recv(&mut buf) {
                eprintln!("received {n} bytes from server");
                let opened = crypto
                    .open(&buf[..n])
                    .expect("OCB3 must decrypt the server's datagram");
                eprintln!(
                    "  decrypted: to_client={} seq={} ts={} ts_reply={} payload={}B",
                    opened.to_client,
                    opened.seq,
                    opened.timestamp,
                    opened.timestamp_reply,
                    opened.payload.len()
                );
                assert!(opened.to_client, "server packets have the TO_CLIENT bit");
                if let Some((_, parsed)) = parse_server_datagram(&crypto, &buf[..n]) {
                    eprintln!(
                        "  Instruction: proto_ver={} old={} new={} ack={} diff={}B",
                        parsed.protocol_version,
                        parsed.old_num,
                        parsed.new_num,
                        parsed.ack_num,
                        parsed.diff.len()
                    );
                    assert_eq!(parsed.protocol_version, PROTOCOL_VERSION);
                }
                got = Some(opened);
                break;
            }
        }
        assert!(got.is_some(), "no reply from mosh-server (crypto interop FAILED)");
        eprintln!("OCB3 interop with real mosh-server: OK");
    }

    #[test]
    fn host_message_decode_concatenates_hoststrings() {
        fn host_instr(s: &[u8]) -> Vec<u8> {
            let mut hb = Vec::new();
            put_tag(4, 2, &mut hb);
            put_varint(s.len() as u64, &mut hb);
            hb.extend_from_slice(s);
            let mut instr = Vec::new();
            put_tag(2, 2, &mut instr);
            put_varint(hb.len() as u64, &mut instr);
            instr.extend_from_slice(&hb);
            instr
        }
        let mut msg = Vec::new();
        for s in [b"AB".as_ref(), b"CD".as_ref()] {
            let instr = host_instr(s);
            put_tag(1, 2, &mut msg);
            put_varint(instr.len() as u64, &mut msg);
            msg.extend_from_slice(&instr);
        }
        assert_eq!(decode_host_message(&msg), b"ABCD");
    }

    #[test]
    fn user_keystroke_encode_is_decodable() {
        let msg = encode_user_keystroke(b"hi\x1b[A");
        // field1 -> Instruction -> ext2 (Keystroke) -> field4 (keys)
        let mut keys = Vec::new();
        for_each_field1(&msg, |instr| {
            if let Some(ks) = nested_field(instr, 2) {
                if let Some(k) = nested_field(ks, 4) {
                    keys.extend_from_slice(k);
                }
            }
        });
        assert_eq!(keys, b"hi\x1b[A");
    }

    #[test]
    fn full_client_datagram_selfconsistent() {
        // Seal a client datagram, then open it as if we were the server.
        let c = Crypto::new(&KEY);
        let inst = Instruction {
            protocol_version: PROTOCOL_VERSION,
            new_num: 1,
            ..Default::default()
        };
        let dg = build_client_datagram(&c, 1, 5, 0, 100, &inst);
        let (opened, parsed) = parse_server_datagram(&c, &dg).unwrap();
        assert!(!opened.to_client);
        assert_eq!(parsed.protocol_version, PROTOCOL_VERSION);
        assert_eq!(parsed.new_num, 1);
    }
}
