#![no_main]
use libfuzzer_sys::fuzz_target;
fuzz_target!(|data: &[u8]| {
    let _ = lantern_storage::decode_backup(data);
    if let Ok(envelope) = serde_json::from_slice::<lantern_storage::BackupEnvelopeV1>(data) {
        let canonical = serde_jcs::to_vec(&envelope).unwrap();
        let roundtrip: lantern_storage::BackupEnvelopeV1 =
            serde_json::from_slice(&canonical).unwrap();
        assert_eq!(envelope, roundtrip);
    }
});
