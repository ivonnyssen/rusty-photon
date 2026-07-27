# Hardware validation records

Successful **real-hardware ConformU runs**, one directory per run. Where the
per-service design docs (`docs/services/<service>.md`, "Real-hardware
validation") narrate *what was learned*, this directory preserves *the
evidence*: which commit was tested, on what platform, against which physical
device, and the unmodified ConformU output.

## Runs

| Date | Service | Device | Platform | Commit | ConformU | Result | Record |
|------|---------|--------|----------|--------|----------|--------|--------|
| 2026-07-26 | svbony-camera | SVBONY SV605CC | Windows 11 (25H2) x64 | [`ef03a1cd`](https://github.com/ivonnyssen/rusty-photon/commit/ef03a1cd7b9e0831e731d0ed9d37df7661fe5edd) | 4.4.0 | `alpacaprotocol` + `conformance` clean | [record](2026-07-26-svbony-camera-sv605cc-windows/README.md) |

## Adding a run

Each run gets a directory named `<YYYY-MM-DD>-<service>-<device>-<platform>/`
containing:

- `README.md` — the run record: the exact commit tested (`git rev-parse HEAD`
  of the built tree), platform and environment details, how the binary was
  built (features, SDK provenance and version), the device identity
  (model + serial as minted into the ASCOM `UniqueID`), the verdicts, and
  anything platform-specific the run taught us.
- The unmodified ConformU output. Ask ConformU to write its own artifacts
  rather than scraping the console:

  ```sh
  conformu alpacaprotocol <device-url> -n alpacaprotocol.log
  conformu conformance    <device-url> -n conformance.log -r conformance-results.json
  ```

- `conformance-results.json` — ConformU's machine-readable verdict
  (`ErrorCount` / `IssueCount` / `ConfigurationAlertCount` /
  `TimingIssuesCount` must all be 0 for a run to be recorded here).

Only **successful** runs are recorded — this directory is the proof trail
that a given commit passed on real hardware, not a debugging journal.
Failures belong in issues. Before committing logs, check they carry no
private network addresses or local usernames (loopback URLs are fine).

Finally, add the run to the table above (newest first) and, when the run is
a service's first on a platform, link the record from the service design
doc's "Real-hardware validation" section.
