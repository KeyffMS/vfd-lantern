use std::{fs, path::PathBuf};

use lantern_release::{BuildManifestV1, CandidateGateReportV1, CandidateManifestV1};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: release_schemas OUTPUT_DIR")?;
    fs::create_dir_all(&directory)?;
    for (name, schema) in [
        (
            "candidate-manifest-v1.schema.json",
            schemars::schema_for!(CandidateManifestV1),
        ),
        (
            "gate-report-v1.schema.json",
            schemars::schema_for!(CandidateGateReportV1),
        ),
        (
            "build-manifest-v1.schema.json",
            schemars::schema_for!(BuildManifestV1),
        ),
    ] {
        fs::write(directory.join(name), serde_json::to_vec_pretty(&schema)?)?;
    }
    Ok(())
}
