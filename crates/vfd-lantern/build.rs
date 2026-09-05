use std::{env, fs, path::PathBuf};

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .join("../..")
        .canonicalize()
        .expect("workspace root must exist");
    let source = env::var_os("VFD_LANTERN_PACKAGED_PROFILES_MANIFEST")
        .map(PathBuf::from)
        .map(|path| {
            if path.is_absolute() {
                path
            } else {
                workspace_root.join(path)
            }
        })
        .unwrap_or_else(|| workspace_root.join("profiles/manifest/profiles-v1.json"));

    let metadata = fs::symlink_metadata(&source).unwrap_or_else(|error| {
        panic!(
            "packaged profile manifest {} is unavailable: {error}",
            source.display()
        )
    });
    assert!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "packaged profile manifest must be a regular non-symlink file: {}",
        source.display()
    );

    let bytes = fs::read(&source).unwrap_or_else(|error| {
        panic!(
            "cannot read packaged profile manifest {}: {error}",
            source.display()
        )
    });
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("profiles-v1.json");
    fs::write(&out, bytes)
        .unwrap_or_else(|error| panic!("cannot stage embedded packaged profile manifest: {error}"));

    println!("cargo:rerun-if-env-changed=VFD_LANTERN_PACKAGED_PROFILES_MANIFEST");
    println!("cargo:rerun-if-changed={}", source.display());
}
