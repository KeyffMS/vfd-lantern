#![no_main]
use lantern_domain::{ByteOrder, RegisterCodec, RegisterEncoding, WordOrder};
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    let words: Vec<u16> = data
        .chunks_exact(2)
        .take(4)
        .map(|x| u16::from_be_bytes([x[0], x[1]]))
        .collect();
    for encoding in [
        RegisterEncoding::Unsigned16,
        RegisterEncoding::Signed16,
        RegisterEncoding::Unsigned32,
        RegisterEncoding::Signed32,
        RegisterEncoding::Unsigned64,
        RegisterEncoding::Signed64,
        RegisterEncoding::Float32,
        RegisterEncoding::Float64,
        RegisterEncoding::Bcd16,
        RegisterEncoding::Bcd32,
        RegisterEncoding::Enum16,
        RegisterEncoding::Bitfield64,
    ] {
        for byte in [ByteOrder::BigEndian, ByteOrder::LittleEndian] {
            for word in [
                WordOrder::MostSignificantFirst,
                WordOrder::LeastSignificantFirst,
            ] {
                let codec = RegisterCodec::new(encoding, byte, word, None).unwrap();
                if let Ok(value) = codec.decode(&words) {
                    assert_eq!(codec.encode(&value).unwrap().as_slice(), words.as_slice());
                }
            }
        }
    }
});
