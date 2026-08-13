#!/usr/bin/env bash
set -euo pipefail

temporary_main=$(mktemp)
awk '
    $0 == "source scripts/finalize-issue-2/20-policy.sh" {
        print "source scripts/finalize-issue-2/21-policy-fix.sh"
        print "write_deny_policy() { cp scripts/finalize-issue-2/deny.toml.template deny.toml; }"
        next
    }
    {
        print
    }
' scripts/finalize-issue-2/00-main.sh > "$temporary_main"
exec bash "$temporary_main"
