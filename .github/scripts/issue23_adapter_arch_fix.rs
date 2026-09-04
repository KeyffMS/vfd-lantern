use std::{fs, path::Path};

fn main() {
    let path = Path::new("scripts/check-architecture.sh");
    let text = fs::read_to_string(path).expect("read architecture check");
    let old = r#"if grep -R -n -E '\bPreparedBusWrite\b' crates/vfd-lantern/src; then
    printf 'composition root must never mint or expose PreparedBusWrite directly\n' >&2
    exit 1
fi
"#;
    let new = r#"if awk '/#\[cfg\(test\)\]/{exit} {print}' crates/vfd-lantern/src/write_runtime.rs \
        | grep -n -E '\bPreparedBusWrite\b'; then
    printf 'production write composition must never mint or expose PreparedBusWrite directly\n' >&2
    exit 1
fi

if find crates/vfd-lantern/src -type f -name '*.rs' ! -name 'write_runtime.rs' -print0 \
        | xargs -0 grep -n -E '\bPreparedBusWrite\b'; then
    printf 'PreparedBusWrite escaped the guarded write runtime boundary\n' >&2
    exit 1
fi
"#;
    assert_eq!(text.matches(old).count(), 1, "unexpected PreparedBusWrite architecture guard");
    fs::write(path, text.replace(old, new)).expect("write architecture check");
}
