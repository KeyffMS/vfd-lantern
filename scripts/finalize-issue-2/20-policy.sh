#!/usr/bin/env bash

write_deny_policy() {
    cp scripts/finalize-issue-2/deny.toml.template deny.toml
}

patch_ci_workflow() {
    awk '
        {
            print
        }
        $0 == "            git \\" {
            print "            libssl-dev \\\"
        }
        $0 == "          cargo --version" {
            print ""
            print "      - name: Install pinned supply-chain tools"
            print "        env:"
            print "          CARGO_INSTALL_ROOT: ${{ runner.temp }}/vfd-lantern-cargo-tools"
            print "        run: sh scripts/install-cargo-tools.sh supply-chain"
        }
    ' .github/workflows/ci.yml > .github/workflows/ci.yml.tmp
    mv .github/workflows/ci.yml.tmp .github/workflows/ci.yml

    sed -i \
        -e 's/cargo build --workspace --locked/cargo build --workspace --all-features --locked/' \
        -e 's/cargo doc --workspace --no-deps --locked/cargo doc --workspace --all-features --no-deps --locked/' \
        .github/workflows/ci.yml

    awk '
        $0 == "      - name: Build documentation" {
            print
            print "        env:"
            print "          RUSTDOCFLAGS: -D warnings"
            next
        }
        $0 == "      - name: Check supply-chain baseline" {
            print "      - name: Check supply chain"
            print "        env:"
            print "          CARGO_VET_REPORT: target/supply-chain/cargo-vet-summary-${{ matrix.arch }}.json"
            print "        run: sh scripts/check-supply-chain.sh"
            print ""
            print "      - name: Upload supply-chain report"
            print "        if: always()"
            print "        uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"
            print "        with:"
            print "          name: supply-chain-${{ matrix.arch }}"
            print "          path: target/supply-chain"
            print "          if-no-files-found: warn"
            print "          retention-days: 14"
            skip = 1
            next
        }
        skip == 1 {
            skip = 0
            next
        }
        {
            print
        }
    ' .github/workflows/ci.yml > .github/workflows/ci.yml.tmp
    mv .github/workflows/ci.yml.tmp .github/workflows/ci.yml
}
