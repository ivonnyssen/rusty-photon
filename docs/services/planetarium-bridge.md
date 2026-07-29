# planetarium-bridge

**Status: P3a verification spike COMPLETE — findings below.** This
document records the P3a milestone of
[planetarium-target-import.md](../plans/planetarium-target-import.md)
(Decision 8): a sanctioned throwaway spike — exempt from the
design-first/BDD-first order, per the ADR-005 precedent — that observed a
real SkySafari install against a virtual Alpaca Telescope. The full
service design document (development-workflow Phase 1) replaces the spike
sections here when P3 proper begins. The spike crate
`spikes/planetarium-bridge-p3a` is deleted at that point; its findings
survive here.

Session: 2026-07-29 (UTC), SkySafari on iPad driving the spike over
Wi-Fi, operator-driven per the session script, ~20 minutes of traffic.
The full JSONL wire log is the raw evidence (kept off-repo with the
operator).

## The P3a questions — answered

| # | Question | Verdict | Finding |
|---|----------|---------|---------|
| 1 | Discovery and connection lifecycle | Answered | Alpaca UDP discovery is **subnet-local** (broadcast; does not cross routed segments — the iPad and spike host were on different subnets and no discovery datagram ever arrived). Manual IP:port entry works across subnets, confirming the plan's documented-default posture. Lifecycle detail below. |
| 2 | Which slew/sync verbs | Answered | GoTo = **`SlewToCoordinatesAsync`, exclusively** (never the blocking variant, never `SetTarget*`+`SlewToTarget`). Align = **`SyncToCoordinates`** (never `SyncToTarget`). The verbs are cleanly distinct — Decision 2's sync-is-not-intent rule is safe. Numbers arrive in **scientific notation** (`RightAscension=1.341988e+01`); parsers must accept it (our `ascom-alpaca` fork does). |
| 3 | J2000 honored, or JNow? | Answered | **J2000 honored.** Five object GoTos (Spica, M 92, M 13, Draco Dwarf, HD 142596) all matched the target's J2000 position to arcseconds; the automated probe verdict on M 13 read 0.04′ (J2000 frame) vs 12.13′ (JNow frame). `EquatorialSystem` is read **once, at connect** — changing the declaration requires a reconnect. No epoch setting exists anywhere in SkySafari's scope UI, so this reads as client behavior (honor the device declaration), not per-install configuration; the planned `assume_epoch` override stays as a safety valve. |
| 4 | Position-report cadence | Answered | A steady **1 Hz cycle** of `Slewing` → `Tracking` → `RightAscension` → `Declination` (four GETs ~10 ms apart, every ~1.0 s), identical while idle and while slewing. The spike's 3 s simulated convergence satisfied it: SkySafari showed the slew as arrived when `Slewing` flipped false. No connection timeouts observed. |
| 5 | **Go/no-go: arbitrary-point GoTo** | **GO, with caveats** | Tapping empty sky offers **no** GoTo — an object must be selected. But **coordinate entry (Search → coordinates) exists and GoTos arbitrary points**. Caveat: the entry form has **no epoch choice and interprets input as equinox-of-date (JNow)**, converting to J2000 on the wire — proven by a round-number probe: entered 14h00m00s / −40°00′00″ arrived as 13h58m23s / −39°52′11″, which is exactly the J2000 equivalent of the entered values read as JNow (the RA −1m37s / Dec +7.7′ shifts match 26.6 years of precession at that position to the second). A second practical path: SkySafari's selectable catalog reaches faint HD/Tycho stars and obscure PGC galaxies — a star within arcminutes of any intended frame center almost always exists and GoTos with exact J2000 coordinates. |

## Connection lifecycle detail (Q1)

- **Preset editor probe** (before any connect): `apiversions` →
  `configureddevices` → `apiversions` → `alignmentmode` →
  `canslewasync` → `canslewaltazasync`, under throwaway `ClientID`s.
- **Connect**: `PUT Connected=true`, then a property battery —
  `EquatorialSystem` (once), site latitude/longitude, `UTCDate`, and a
  capability sweep (`CanSync`, `CanSyncAltAz`, `CanSetTracking`,
  `CanPark`, `CanMoveAxis`, `CanSlewAsync`, `CanSlewAltAzAsync`) — then
  the 1 Hz poll loop.
- **SkySafari pushes site and time to the device**: `PUT SiteLatitude` /
  `PUT SiteLongitude` (its own GPS-derived values) and `PUT UTCDate`.
  The spike accepted the site writes; `SetUTCDate` answered
  `NOT_IMPLEMENTED` (1024) — SkySafari retried once ~23 s later and
  carried on unaffected. The bridge should accept these writes or
  reject them benignly; neither breaks the client.
- **Disconnect sends nothing**: polling simply stops — no
  `Connected=false` was observed. The bridge must not depend on an
  explicit disconnect signal (idle-timeout thinking only).
- `ClientID` is **not stable** across app contexts: the main scope
  panel used one value, other gestures another. Treat it as
  diagnostic, not identity.
- SkySafari's **Center** button is display-only (no wire traffic) —
  only **GoTo** and **Align** reach the device.

## Below-horizon behavior (feeds P3/P4 design)

- **GoTo is horizon-gated client-side.** A below-horizon target's GoTo
  is refused by SkySafari — sometimes silently (nothing on the wire, no
  dialog), sometimes with a warning dialog. Couch-planning an object
  that has not yet risen therefore **cannot** be imported by object-GoTo
  at that moment; the operator must import while the target is up, or
  use coordinate entry (not horizon-gated in the observed case, which
  reached a 17°-altitude point).
- **Align/Sync is NOT horizon-gated** — a sync to below-horizon M 31
  went through.
- **Wedge hazard**: after that sync put the virtual scope's reported
  position below the horizon, SkySafari refused *every* subsequent GoTo
  ("stuck") until the reported position returned above the horizon
  (fixed server-side with a corrective sync). Design consequence for
  the real bridge: the virtual device's reported position must never
  linger below the horizon — this can happen with no sync at all, since
  a tracked position imported at dusk sets hours later. An idle
  reposition policy (e.g. drift the virtual pointing back to a
  meridian/equator idle point after convergence reporting completes) is
  a P3 design decision to make deliberately.

## Bridge design implications collected

1. Manual IP:port is the primary connection story (discovery is
   broadcast-scoped to the subnet; the fleet's opt-in single-responder
   convention stands — see plan Decision 1).
2. The GoTo/Sync verb split is exactly as Decision 2 assumes; sync can
   be ignored with zero risk of losing intent.
3. The wire is J2000 end-to-end for this client; `assume_epoch` remains
   a cheap insurance config.
4. Accept-or-benignly-reject `SetSiteLatitude`/`SetSiteLongitude`/
   `SetUTCDate`; expect scientific-notation floats.
5. Reported-position policy must keep the virtual scope above the
   horizon (wedge hazard above).
6. Operator docs must state that SkySafari's coordinate-entry box is
   JNow (no epoch choice); coordinates copied from J2000 sources land
   ~20′ off if typed there. Framing via a nearby faint catalog star
   avoids the pitfall entirely.
7. No explicit disconnect arrives; connection state is inferred, not
   signaled.

## Running the spike (kept for re-runs until the crate is deleted)

```sh
cargo run -p planetarium-bridge-p3a -- \
  --latitude <site-lat> --longitude <site-lon> \
  --log-file ~/p3a-wire.jsonl
```

City-level site coordinates are enough — they only drive the alt-az and
sidereal-time numbers the device reports. Defaults serve the Alpaca API
on `0.0.0.0:11126` and answer Alpaca UDP discovery on `32227`
(`--no-discovery` turns the responder off). On a firewalled host
(Fedora): `sudo firewall-cmd --add-port=11126/tcp --add-port=32227/udp`
(non-permanent).

Console output is the narrative (connects, GoTos with automatic
J2000-vs-JNow verdicts, syncs, discovery packets); the JSONL wire log is
the evidence. `RUST_LOG=debug` additionally narrates each poll. Useful
flags: `--slew-secs <f64>` (convergence window, default 3 s),
`--equatorial-system {j2000,topocentric,other}` (re-run the epoch
experiment with a different declaration), `--probe "<name>"` (extra
epoch-inference probe objects; repeatable).

On every GoTo the spike compares received coordinates against a probe
list of bright catalog objects (M 31, M 42, M 13, M 51, M 81, M 101,
M 104, M 8, M 27, M 57, NGC 7000, NGC 253) in both the J2000 and
apparent-of-date frames (ERFA `Atci13`; ~10–20′ apart in 2026) and
prints a verdict when the received value lands within 2′ of one frame
and beyond 10′ of the other.

### Analyzing a wire log

```sh
# Full timeline
jq -r 'select(.kind=="http") | "\(.t) \(.method) \(.path)"' p3a-wire.jsonl

# Member histogram
jq -r 'select(.kind=="http") | .path' p3a-wire.jsonl | sort | uniq -c | sort -rn

# Verbs answered NOT_IMPLEMENTED (ASCOM error 1024)
jq -r 'select(.response | test("\"ErrorNumber\":1024")) | .path' p3a-wire.jsonl | sort -u
```

## What the spike device implements

One Alpaca `Telescope` (device 0) via the `ascom-alpaca` server feature:
`EquatorialSystem` = J2000 (flag-overridable), `CanSlew`/`CanSlewAsync`/
`CanSync` = true, park/home/pulse-guide/alt-az capabilities = false.
GoTos run a simulated slew (`Slewing` true for the convergence window,
position interpolating toward the target). Sync is accepted and
reflected — the real bridge will *ignore* sync per plan Decision 2; the
spike reflects it to keep the client's pointing model happy and observe
its follow-up. Alt-az and sidereal time are computed from the configured
site via `rp-ephemeris`. Everything else stays at the crate's
`NOT_IMPLEMENTED` defaults so the wire log records what the client
actually wanted. The device name states loudly that it is a virtual
target-entry device, not a mount; it never touches hardware or rp
(tenet 3 trivially satisfied).
