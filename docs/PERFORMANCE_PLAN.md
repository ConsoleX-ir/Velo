# Performance and Release Measurement Plan

**Status:** Measurement protocol plus one reproducible local engine run; release and UI metrics remain unmeasured
**Research checked:** 2026-10-21

## Phase 1E local engine run

An opt-in deterministic benchmark/correctness harness is in `src/tests.rs` as the ignored test `local_performance_scenarios`. It runs only against a loopback server and verifies every output byte against a deterministic `(offset % 251)` pattern, exact length, terminal success, monotonic progress snapshots, active-request ceiling, queueing, and absence of leftover temporary files. It reports elapsed wall time and aggregate effective MiB/s. It exercises 1 MiB and 64 MiB single transfers, four concurrent 16 MiB transfers, eight 8 MiB jobs under a four-job active cap, and ten sequential 1 MiB cycles reusing one manager/engine/directory and acknowledging each terminal result. Existing non-benchmark tests cover empty bodies, throttled steady bodies, stalled bodies/timeouts, cancellation, and bounded Range continuation.

Reproduce with:

```powershell
cargo test --release local_performance_scenarios -- --ignored --nocapture
```

Observed on 2026-10-22, before any production-code changes for Phase 1E:

| Scenario | Jobs × bytes/job | Elapsed | Aggregate throughput | Correctness / concurrency |
|---|---:|---:|---:|---|
| Single fast small | 1 × 1 MiB | 6 ms | 147.11 MiB/s | exact bytes, 1 active, 1 success |
| Single fast large | 1 × 64 MiB | 190 ms | 335.42 MiB/s | exact bytes, 1 active, 1 success |
| Four throttled | 4 × 16 MiB | 3,651 ms | 17.53 MiB/s | exact bytes, peak 4 active, 4 successes |
| Eight queued, four active | 8 × 8 MiB | 3,627 ms | 17.65 MiB/s | exact bytes, peak 4 active, 4 queued at admission, 8 successes |
| Ten repeated small transfers | 10 sequential × 1 MiB, same manager/engine/directory | 5.47–7.91 ms; median 6.10 ms, nearest-rank p90 7.10 ms | median of per-cycle rates 164.05 MiB/s | all exact; terminal results acknowledged; no temp leftovers |

Environment: Windows NT 10.0.26300.0, Rust 1.96.0, Cargo 1.96.0, 12 logical processors, release profile, repository baseline commit `a65050a`. Windows CIM access was denied, so CPU model and RAM could not be recorded. Storage type, power mode, antivirus state, and thermal state were not controlled. Timings are one exploratory run, not release targets or before/after evidence. The four/eight-job cases intentionally use a mock-server delay of 1 ms per 64 KiB write; those rates characterize the paced harness and should not be interpreted as unthrottled engine throughput. The generated pattern is checked byte-for-byte; no cryptographic digest is used. No CPU or memory measurements were collected.

No production optimization was justified by this single controlled run. The high single-transfer rates and lower explicitly paced rates do not identify an engine bottleneck. Repeat on a controlled reference machine before setting budgets or making performance claims.

## Targets versus evidence

The approximately 100–130 MB Windows installer is a planning goal only. The Rust core is buildable and the local engine measurements below have been run, but no packaged desktop application exists. Installer size, installed footprint, startup, process memory/CPU, and controlled LAN results are not available. Do not present estimates as results.

## Reproducible baseline

For every release candidate, record commit, build profile, Rust/Node/Tauri versions, lockfile hashes, Windows build, CPU/RAM/storage, power mode, WebView2 version/runtime distribution mode, installer type, signing state, antivirus state, and whether the machine is cold or warm. Use a clean Windows 10/11 x64 VM with fixed resources; repeat each timed case at least 10 times, report median and p90, and retain raw output. Record tool versions and exact commands/scripts in `benchmarks/` when implementation exists.

| Metric | Procedure | Report |
|---|---|---|
| Installer size | Build the selected signed/unsigned installer from a clean release build; measure bytes on disk; repeat for runtime bundling choices | Exact bytes, format, WebView2 mode, artifacts |
| Installed footprint | Install in a clean VM, measure app files and separately account for shared WebView2/runtime files | App bytes and total incremental bytes |
| Cold start | Reboot/clear file cache per documented protocol; launch to first usable window; instrument native start and first rendered-ready marker | Median/p90; exclude first-time runtime download separately |
| Idle memory | At 1, 5, and 15 minutes after ready with no jobs; record process private working set and commit | Median/p90, background processes noted |
| Active memory | Download fixed large fixture at 1 and the provisional maximum of 4 concurrent jobs; sample at 1-second interval | Peak and steady-state private bytes; fixture size and duration |
| CPU | Same 1-job and 4-job concurrency and throughput scenarios; capture total process CPU and normalized CPU time | Average and p95 over steady state |
| Throughput/correctness | Loopback controlled server and a separate controlled LAN server; fixed file sizes, latency, bandwidth, and concurrency; compare output hash/size | MiB/s, CPU, bytes, hash, errors; do not use public servers as baseline |
| Progress overhead | Run identical transfer with progress disabled/minimal and configured events; measure CPU, wall time, number of emitted events, queue depth | Overhead %, event rate, max queue size |
| Failure/cleanup | Inject cancel at phases, reset connections, malformed/truncated body, full disk, app termination | Time to terminal outcome, cleanup state, data integrity, orphan temp-file behavior; startup recovery is not claimed in Phase 1 |

## Acceptance thresholds

Set numeric targets only after Phase 1 produces a baseline on representative hardware. Initial correctness gates are exact bytes for fixtures, bounded progress events/queues, no memory proportional to object size, and deterministic terminal behavior. Product owner should then approve performance budgets for startup, active memory, CPU, throughput regression, and installer size.

## Controls and interpretation

- Run correctness first; performance runs must verify a cryptographic hash after each transfer.
- Separate local disk/network effects; use the loopback server for repeatability and LAN server for realistic network stack overhead.
- Warm/cold cache and Defender/antivirus state materially affect results; report both where practical, never silently mix.
- Keep profiler overhead out of final timing runs or disclose it.
- Compare same machine/image and settings across revisions; retain failed runs and exclusions with reasons.
- Installer compression can trade build time/install time/CPU for download size; test choices rather than infer impact.

## Current verified facts and assumptions

- **Verified official documentation:** Tauri offers MSI and NSIS Windows packaging paths; WebView2 distribution is configurable. Tokio's ordinary file operations currently use a blocking pool. These facts do not establish the resulting product size or throughput.
- **Assumption:** 100–130 MB installer is an initial planning range.
- **Unmeasured:** installer and installed size, startup, process CPU/memory, controlled LAN throughput, and release-target distributions. The local results above are narrow engine observations only.

## References

- Tauri Windows installer and WebView2 options: https://v2.tauri.app/distribute/windows-installer/ (checked 2026-10-21)
- Tokio filesystem implementation notes: https://docs.rs/tokio/latest/tokio/fs/index.html (checked 2026-10-21)
