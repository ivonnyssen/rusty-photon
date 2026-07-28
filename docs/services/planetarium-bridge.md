# planetarium-bridge

**Status: P3a verification spike.** This document currently records the
P3a milestone of
[planetarium-target-import.md](../plans/planetarium-target-import.md)
(Decision 8): a sanctioned throwaway spike — exempt from the
design-first/BDD-first order, per the ADR-005 precedent — that observes a
real SkySafari install against a virtual Alpaca Telescope. The full
service design document (development-workflow Phase 1) replaces the spike
sections here once the findings below are in and the go/no-go question is
answered. The spike crate `spikes/planetarium-bridge-p3a` is deleted at
that point; its findings survive here.

## The P3a questions

| # | Question | Status | Finding |
|---|----------|--------|---------|
| 1 | Discovery and connection lifecycle: does SkySafari use Alpaca UDP discovery, what does it read on connect, how does it disconnect? | Pending | |
| 2 | Which slew/sync verbs does SkySafari send? (`SlewToCoordinates` vs `...Async` vs `SetTarget*`+`SlewToTarget`; `SyncToCoordinates` vs `SyncToTarget`) | Pending | |
| 3 | Does SkySafari honor the device-declared J2000 `EquatorialSystem`, or send JNow — and is that a version constant or per-install configuration? | Pending | |
| 4 | What position-report cadence does SkySafari need to consider a slew complete and stay connected? | Pending | |
| 5 | **Go/no-go:** can the SkySafari UI GoTo an arbitrary point (tapped empty sky / entered coordinates), or only cataloged objects? | Pending | |

Session metadata to record with the findings: SkySafari edition and
version (Plus/Pro, 7.x/8.x), iOS or Android, and the state of any
epoch/equinox setting found in SkySafari's scope-setup UI.

## Running the spike

```sh
cargo run -p planetarium-bridge-p3a -- \
  --latitude <site-lat> --longitude <site-lon> \
  --log-file ~/p3a-wire.jsonl
```

City-level site coordinates are enough — they only drive the alt-az and
sidereal-time numbers the device reports, so SkySafari's horizon display
matches its own sky model. Defaults serve the Alpaca API on
`0.0.0.0:11126` and answer Alpaca UDP discovery on `32227` (the spike is
deliberately discovery-ON, unlike the fleet's opt-in convention — question
1 needs it; `--no-discovery` turns it off).

On a firewalled host (Fedora), open the ports for the session:

```sh
sudo firewall-cmd --add-port=11126/tcp --add-port=32227/udp
```

(non-`--permanent`, so the next reload closes them again).

Console output is the narrative: connects, GoTos with an automatic
J2000-vs-JNow verdict, syncs, discovery packets. The JSONL wire log is
the evidence: every HTTP request/response with millisecond receipt
timestamps, and every discovery datagram. `RUST_LOG=debug` additionally
narrates each poll on the console.

Useful flags: `--slew-secs <f64>` (simulated convergence window, default
3 s), `--equatorial-system {j2000,topocentric,other}` (re-run the epoch
experiment with a different declaration), `--probe "<name>"`
(add epoch-inference probe objects; repeatable).

### Epoch verdicts

On every GoTo the spike compares the received coordinates against a probe
list of bright catalog objects (M 31, M 42, M 13, M 51, M 81, M 101,
M 104, M 8, M 27, M 57, NGC 7000, NGC 253) in two frames: their J2000
positions and their apparent-of-date positions (ERFA `Atci13`). In 2026
the frames differ by ~20′, so a GoTo that lands within 2′ of one frame
and >10′ from the other is an unambiguous verdict, printed on the
console. GoTos to stars, planets, or arbitrary points get no verdict —
use a probe object for the epoch answer.

## SkySafari session script

Each step names the question it answers.

1. **(Q1)** In SkySafari: Settings → Telescope → Setup; scope type
   *ASCOM Alpaca*. Try auto-discovery first and watch the spike console
   for `DISCOVERY` lines. Record whether the spike's device is offered.
2. **(Q1)** Whether or not discovery worked, note the manual path: enter
   the spike machine's LAN IP and port `11126`, device 0. Tap **Connect**.
   The console shows the management reads and `CLIENT CONNECTED`; the wire
   log shows every property SkySafari reads on connect.
3. **(Q2, Q3)** Select **M 31** in SkySafari and tap **GoTo**. The console
   prints the verb used and the epoch verdict. Repeat for one southern
   probe (e.g. M 8) for confidence.
4. **(Q4)** Let the connection idle for ~2 minutes. The wire-log
   timestamps give the poll cadence (see analysis below). Watch whether
   SkySafari shows the slew as complete when `Slewing` flips false.
5. **(Q5 — the go/no-go)** Try every way to GoTo a *non-object* point:
   tap an empty patch of sky and check whether the popup offers GoTo;
   search/enter raw coordinates if the UI has such an entry; drag the sky
   so the crosshair sits on empty field and look for a "slew here"
   affordance. Record each attempt — the wire log shows whether *any*
   path produces a slew to non-catalog coordinates.
6. **(Q2)** With the scope "pointing" at the last GoTo, select a nearby
   *different* object and use SkySafari's **Align** gesture. The console
   prints the sync verb. This confirms the Sync/GoTo distinction the
   bridge design relies on (plan Decision 2).
7. **(Q3, only if time)** Restart the spike with
   `--equatorial-system topocentric` and repeat step 3: does SkySafari
   change what it sends, and did its scope-setup UI expose any
   epoch/equinox setting?
8. **(Q1)** Disconnect from SkySafari's UI; watch whether a
   `Connected=false` PUT arrives or the TCP connections just stop.
   Reconnect once to confirm the lifecycle repeats cleanly.

Afterwards stop the spike (Ctrl-C), keep the console output and the
JSONL, and fill in the findings table above.

## Analyzing the wire log

```sh
# Full timeline
jq -r 'select(.kind=="http") | "\(.t) \(.method) \(.path)"' p3a-wire.jsonl

# Which members does the client use, how often?
jq -r 'select(.kind=="http") | .path' p3a-wire.jsonl | sort | uniq -c | sort -rn

# Verbs the spike answered NOT_IMPLEMENTED (ASCOM error 1024) — members
# SkySafari wanted that the plan's device sketch must consider
jq -r 'select(.response | test("\"ErrorNumber\":1024")) | .path' p3a-wire.jsonl | sort -u
```

Poll cadence is the gap between successive `GET` timestamps per member;
the `t` field is millisecond-precision RFC 3339.

## What the spike device implements

One Alpaca `Telescope` (device 0) via the `ascom-alpaca` server feature:
`EquatorialSystem` = J2000 (flag-overridable), `CanSlew`/`CanSlewAsync`/
`CanSync` = true, park/home/pulse-guide/alt-az capabilities = false. GoTos
run a simulated slew: `Slewing` reports true for the convergence window
while the reported position interpolates toward the target. Sync is
accepted and reflected (the real bridge will *ignore* sync per plan
Decision 2; the spike reflects it to keep the client's pointing model
happy and observe its follow-up). Alt-az and sidereal time are computed
from the configured site via `rp-ephemeris`, so the crosshair SkySafari
draws sits where SkySafari expects it. Everything else stays at the
crate's `NOT_IMPLEMENTED` defaults — deliberately, so the wire log
records what the client actually wanted. The device name states loudly
that it is a virtual target-entry device, not a mount; it never touches
hardware or rp (tenet 3 trivially satisfied).
