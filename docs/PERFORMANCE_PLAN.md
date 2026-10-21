# Performance and Release Measurement Plan

**Status:** Measurement protocol; no measurements recorded  
**Research checked:** 2026-10-21

## Targets versus evidence

The approximately 100–130 MB Windows installer is a planning goal only. No package-size, startup, memory, CPU, throughput, or recovery benchmark has been run for this repository. Do not present estimates as results. The only current state is an empty workspace with no buildable application.

## Reproducible baseline

For every release candidate, record commit, build profile, Rust/Node/Tauri versions, lockfile hashes, Windows build, CPU/RAM/storage, power mode, WebView2 version/runtime distribution mode, installer type, signing state, antivirus state, and whether the machine is cold or warm. Use a clean Windows 10/11 x64 VM with fixed resources; repeat each timed case at least 10 times, report median and p90, and retain raw output. Record tool versions and exact commands/scripts in `benchmarks/` when implementation exists.

| Metric | Procedure | Report |
|---|---|---|
| Installer size | Build the selected signed/unsigned installer from a clean release build; measure bytes on disk; repeat for runtime bundling choices | Exact bytes, format, WebView2 mode, artifacts |
| Installed footprint | Install in a clean VM, measure app files and separately account for shared WebView2/runtime files | App bytes and total incremental bytes |
| Cold start | Reboot/clear file cache per documented protocol; launch to first usable window; instrument native start and first rendered-ready marker | Median/p90; exclude first-time runtime download separately |
| Idle memory | At 1, 5, and 15 minutes after ready with no jobs; record process private working set and commit | Median/p90, background processes noted |
| Active memory | Download fixed large fixture at 1, 4, and planned maximum concurrent jobs; sample at 1-second interval | Peak and steady-state private bytes; fixture size and duration |
| CPU | Same concurrency and throughput scenarios; capture total process CPU and normalized CPU time | Average and p95 over steady state |
| Throughput/correctness | Loopback controlled server and a separate controlled LAN server; fixed file sizes, latency, bandwidth, and concurrency; compare output hash/size | MiB/s, CPU, bytes, hash, errors; do not use public servers as baseline |
| Progress overhead | Run identical transfer with progress disabled/minimal and configured events; measure CPU, wall time, number of emitted events, queue depth | Overhead %, event rate, max queue size |
| Failure/recovery | Inject cancel at phases, reset connections, malformed/truncated body, full disk, app termination; for later recovery test relaunch | Time to terminal outcome, cleanup state, data integrity, recovered status |

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
- **Unmeasured:** every performance and packaging metric above.

## References

- Tauri Windows installer and WebView2 options: https://v2.tauri.app/distribute/windows-installer/ (checked 2026-10-21)
- Tokio filesystem implementation notes: https://docs.rs/tokio/latest/tokio/fs/index.html (checked 2026-10-21)
