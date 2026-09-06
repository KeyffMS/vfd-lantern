# Platform policy — Debian 13 amd64 first

Decision: 2026-09-06. The active development, acceptance, packaging and candidate-release target is **Debian 13 (Trixie), amd64 (x86_64)**.

## Current environment and acceptance

Use the existing self-hosted runner `vfd-lantern-podman-01` with labels
`[self-hosted, linux, x64, podman, vfd-lantern]`. Check Linux/X64, `uname -m=x86_64`,
Debian and VERSION_ID 13 before executing the gate. Package installation tests use
the pinned Trixie image through Podman. Ubuntu-hosted builds do not replace this acceptance.

All current feature, CI/conformance, documentation and packaging work is evaluated
on this platform, including issues #21, #27, #24 and the amd64 candidate in #25.
Missing arm64 runners, builds, artifacts or reports do not block this scope.
Keep all functional, security, profile qualification, audit/trust/restore,
reproducibility, performance and hardware acceptance requirements on amd64.
A green general CI run does not replace package install/uninstall or reproducibility evidence.

## End-of-queue platform work

Finish and qualify Debian 13 amd64 first. Debian arm64 porting and native acceptance
come at the end of the queue. Other operating systems are deferred for a separate
scope decision after the Debian amd64 work; this policy does not declare them supported
or add previously excluded systems to 1.0.

Do not schedule arm64 jobs, require arm64 assets during assembly/finalization, or claim
multi-platform support in current release notes. Historical arm64 test results remain
historical evidence; they do not qualify the current product.

Reactivating another platform requires an explicit roadmap update, a working native
environment, its own package/conformance/performance/soak/HIL evidence and comparison
of deterministic traces where applicable. Never copy amd64 evidence into an arm64 report.

## Remaining roadmap

- #24 is in progress on `agent/issue-24`; #17 is the latest completed feature.
- Complete #21 CI/long-run infrastructure and #27 conformance on Debian 13 amd64;
  these remain dependencies of #24.
- Complete #24 package, documentation and disposable draft/finalizer/publish acceptance
  on amd64. This stage does not publish the real 1.0 release.
- #25 qualifies exact amd64 candidate assets and promotes the same verified draft
  only after its required gates pass.
- Platform expansion follows at the end of the queue.

The operational roadmap is [#26](https://github.com/KeyffMS/vfd-lantern/issues/26).
This policy supersedes earlier simultaneous amd64/arm64 acceptance requirements;
it does not mark unfinished work or deferred platforms as completed.
