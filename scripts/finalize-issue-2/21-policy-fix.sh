#!/usr/bin/env bash

insert_after_line() {
    file=$1
    line_number=$2
    block=$3
    temporary=$(mktemp)
    head -n "$line_number" "$file" > "$temporary"
    cat "$block" >> "$temporary"
    tail -n "+$((line_number + 1))" "$file" >> "$temporary"
    mv "$temporary" "$file"
}

patch_ci_workflow() {
    file=.github/workflows/ci.yml

    git_line=$(grep -n '^[[:space:]]*git \\$' "$file" | head -n 1 | cut -d: -f1)
    git_block=$(mktemp)
    printf '%s\n' '            libssl-dev \' > "$git_block"
    insert_after_line "$file" "$git_line" "$git_block"

    version_line=$(grep -n '^          cargo --version$' "$file" | head -n 1 | cut -d: -f1)
    tools_block=$(mktemp)
    cat > "$tools_block" <<'EOF'

      - name: Install pinned supply-chain tools
        env:
          CARGO_INSTALL_ROOT: ${{ runner.temp }}/vfd-lantern-cargo-tools
        run: sh scripts/install-cargo-tools.sh supply-chain
EOF
    insert_after_line "$file" "$version_line" "$tools_block"

    sed -i \
        -e 's/cargo build --workspace --locked/cargo build --workspace --all-features --locked/' \
        -e 's/cargo doc --workspace --no-deps --locked/cargo doc --workspace --all-features --no-deps --locked/' \
        "$file"

    docs_line=$(grep -n '^      - name: Build documentation$' "$file" | head -n 1 | cut -d: -f1)
    docs_block=$(mktemp)
    cat > "$docs_block" <<'EOF'
        env:
          RUSTDOCFLAGS: -D warnings
EOF
    insert_after_line "$file" "$docs_line" "$docs_block"

    baseline_line=$(grep -n '^      - name: Check supply-chain baseline$' "$file" | head -n 1 | cut -d: -f1)
    temporary=$(mktemp)
    head -n "$((baseline_line - 1))" "$file" > "$temporary"
    cat >> "$temporary" <<'EOF'
      - name: Check supply chain
        env:
          CARGO_VET_REPORT: target/supply-chain/cargo-vet-summary-${{ matrix.arch }}.json
        run: sh scripts/check-supply-chain.sh

      - name: Upload supply-chain report
        if: always()
        uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02
        with:
          name: supply-chain-${{ matrix.arch }}
          path: target/supply-chain
          if-no-files-found: warn
          retention-days: 14
EOF
    mv "$temporary" "$file"
}
