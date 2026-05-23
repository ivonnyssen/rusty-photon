//! Configuration types.
//!
//! See [`docs/services/star-adventurer-gti.md`](../../../docs/services/star-adventurer-gti.md)
//! §"Configuration" for the canonical schema and field meanings.

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Top-level configuration deserialised from the JSON config file.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    pub transport: TransportConfig,
    pub server: ServerConfig,
    pub mount: MountConfig,
}

/// Transport block — `usb` (serial) or `udp` (WiFi).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TransportConfig {
    Usb(UsbConfig),
    Udp(UdpConfig),
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self::Usb(UsbConfig::default())
    }
}

impl TransportConfig {
    /// Human-readable label for the configured transport target. Used by
    /// the connect handshake's wrong-device diagnostic (see
    /// [`crate::error::StarAdvError::WrongDevice`] and issue #254) to
    /// quote the configured port in operator-facing error messages.
    ///
    /// * USB → the configured serial-port path verbatim (e.g.
    ///   `/dev/serial/by-id/...` or `/dev/ttyACM0`).
    /// * UDP → [`SocketAddr`]'s `Display` form, which brackets IPv6
    ///   correctly (`[fe80::1]:11880`) and leaves IPv4 unbracketed
    ///   (`192.168.4.1:11880`). Matches the canonical operator-typing
    ///   form a tool like `nc -u` accepts.
    ///
    /// [`SocketAddr`]: std::net::SocketAddr
    pub fn port_label(&self) -> String {
        match self {
            Self::Usb(u) => u.port.clone(),
            Self::Udp(u) => std::net::SocketAddr::new(u.address, u.port).to_string(),
        }
    }
}

/// USB-CDC serial transport config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsbConfig {
    pub port: String,
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    #[serde(default = "default_command_timeout", with = "humantime_serde")]
    pub command_timeout: Duration,
    #[serde(default = "default_polling_interval", with = "humantime_serde")]
    pub polling_interval: Duration,
}

/// UDP/WiFi transport config.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UdpConfig {
    pub address: IpAddr,
    #[serde(default = "default_udp_port")]
    pub port: u16,
    /// Mandatory on UDP — must be a 192.168.4.0/24 host IP when the mount is
    /// in AP mode.
    pub bind_address: IpAddr,
    #[serde(default = "default_command_timeout", with = "humantime_serde")]
    pub command_timeout: Duration,
    #[serde(default = "default_polling_interval", with = "humantime_serde")]
    pub polling_interval: Duration,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    #[serde(default = "default_discovery_port")]
    pub discovery_port: Option<u16>,
    #[serde(default)]
    pub tls: Option<rp_tls::config::TlsConfig>,
    #[serde(default)]
    pub auth: Option<rp_auth::config::AuthConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountConfig {
    pub name: String,
    pub unique_id: String,
    pub description: String,
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// WGS84 degrees, +N. ASCOM convention.
    pub site_latitude_deg: f64,
    /// WGS84 degrees, +E. ASCOM convention.
    pub site_longitude_deg: f64,
    #[serde(default)]
    pub site_elevation_m: f64,

    #[serde(default = "default_settle_after_slew", with = "humantime_serde")]
    pub settle_after_slew: Duration,

    /// Reserved for future expansion. Sidereal only in MVP.
    #[serde(default)]
    pub tracking_rate: TrackingRateName,

    /// CW exclusion zone in encoder `mech_HA` (signed
    /// hours, folded to `[−12, +12)`). Slews / syncs whose chosen
    /// pier side's `mech_HA` falls inside
    /// `(binding_zone_min_hours, binding_zone_max_hours)` are
    /// rejected with `INVALID_VALUE` and never reach the wire. Flip
    /// slews additionally check that the *path swept* by the polar
    /// axis stays clear of the zone — see [`flip_slew_ra_delta`].
    ///
    /// The physical constraint on the GTi is "the counterweights
    /// must not rise more than 0.95 h (≈ 14°) above horizontal at
    /// any point." This carves a one-sided arc on the positive
    /// `mech_HA` side: the CW shaft crosses horizontal at
    /// `mech_HA = +0.95`, peaks at `mech_HA = +6`, and crosses
    /// horizontal again at `mech_HA = +11.05`. The negative-mech_HA
    /// mirror is *not* a CW exclusion zone — the CW swings below horizontal
    /// into the ground beneath the pier rather than above the mount
    /// head.
    ///
    /// Defaults are `(0.95, 11.05)` — the wide-zone reading of the
    /// "0.95 h above horizontal" rule, hardware-verified at lat
    /// 32.7°N. The narrower `(6.95, 11.05)` zone the driver used
    /// previously captured only the *outer* portion (CW descending
    /// past the pier from above), missing the inner ascent through
    /// which the OTA contacted the tripod during the 2026-05-17
    /// session.
    ///
    /// Set `binding_zone_min_hours = binding_zone_max_hours` (or any
    /// non-overlapping pair) to disable the check entirely; only do
    /// this for tests or for mounts whose CW exclusion arc is known to
    /// be elsewhere. See the design doc's
    /// [§Per-pier safety envelopes](../../../docs/services/star-adventurer-gti.md#per-pier-safety-envelopes).
    #[serde(default = "default_binding_zone_min_hours")]
    pub binding_zone_min_hours: f64,
    #[serde(default = "default_binding_zone_max_hours")]
    pub binding_zone_max_hours: f64,

    /// Slack added on both edges of the CW exclusion zone for the
    /// **tracking-time safety guard** (see the design doc's
    /// [§"Tracking-time safety guard"](../../../docs/services/star-adventurer-gti.md#tracking-time-safety-guard)).
    ///
    /// While `Tracking = true`, a background watcher stops the mount
    /// (`:K1`) once the live encoder `mech_HA` enters
    /// `(binding_zone_min_hours − margin, binding_zone_max_hours + margin)`,
    /// before tracking drift can carry the counterweights into the
    /// zone. The margin lets cautious operators stop early; `0.0`
    /// stops exactly at zone entry. Defaults to `0.05` h (≈ 45 s of
    /// sidereal drift).
    ///
    /// Independent of [`FlipPolicy::enabled`]: the guard is the safety
    /// floor and runs whenever the zone is active
    /// (`binding_zone_min_hours < binding_zone_max_hours`), regardless
    /// of meridian-flip support. Validated on load: a non-finite,
    /// negative, or over-cap value (> [`MAX_TRACKING_GUARD_MARGIN_HOURS`])
    /// fails config loading. The guard additionally treats a non-finite or
    /// negative value as `0.0` as defense-in-depth for construction paths
    /// that bypass validation.
    #[serde(default = "default_tracking_guard_margin_hours")]
    pub tracking_guard_margin_hours: f64,

    /// Safe Dec mechanical-degree envelope. Same enforcement and
    /// rationale as the RA limits. Defaults `[-90.0, +90.0]` — the
    /// observable hemisphere, plus the convention that
    /// "encoder = 0" is OTA on the meridian.
    #[serde(default = "default_dec_min_degrees")]
    pub dec_min_degrees: f64,
    #[serde(default = "default_dec_max_degrees")]
    pub dec_max_degrees: f64,

    /// Persisted park-target encoder positions, written by `SetPark`
    /// and read on every connect. When `None` (default on first run),
    /// the driver falls back to the encoder positions captured during
    /// the init handshake (`:j1` / `:j2`). See the design doc's
    /// [§"Park lifecycle"](../../../docs/services/star-adventurer-gti.md#park-lifecycle)
    /// and [§"Park persistence"](../../../docs/services/star-adventurer-gti.md#park-persistence).
    #[serde(default)]
    pub park_ra_ticks: Option<i32>,
    #[serde(default)]
    pub park_dec_ticks: Option<i32>,

    /// Meridian-flip policy. See the design doc's
    /// [§"Meridian flip"](../../../docs/services/star-adventurer-gti.md#meridian-flip).
    /// Defaults to `enabled = false` so the driver behaves identically
    /// to pre-Phase-6 builds until an operator opts in on a
    /// hardware-validated mount.
    #[serde(default)]
    pub flip_policy: FlipPolicy,

    /// Physical pose the mount is in at power-up.
    ///
    /// When `Some(ApPark*)`, the driver seeds the firmware encoder on
    /// connect (no motion, just `:E1` / `:E2`) so the codebase's
    /// celestial-coordinate math matches the operator's physical
    /// pose. When `None` (the default), the driver does no seeding
    /// and trusts the firmware encoder as-is — the codebase's
    /// historical pre-Phase-6 behaviour. See [`HomePose`] for the
    /// supported AP park positions and the wiring in
    /// `MountDevice::seed_home_pose_after_connect`.
    #[serde(default)]
    pub home_pose: Option<HomePose>,
}

/// Master switch + parameters for driver-planned meridian flips.
///
/// `enabled = false` (the shipped default) disables every flip code
/// path: `CanSetPierSide` reports `false`, `SetSideOfPier` returns
/// `NOT_IMPLEMENTED`, `DestinationSideOfPier` always returns the
/// current side, and slews use the pre-flip coordinate pipeline only.
///
/// With `enabled = true`, the driver may pick the flipped pier side
/// for a slew (or honour an explicit `SetSideOfPier` call) when the
/// target's hour angle falls inside the meridian window of width
/// `2 × flip_range_hours`. See the design doc for the full decision
/// tree and the per-side safety envelopes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FlipPolicy {
    /// Master switch. Defaults `false` until the first real-hardware
    /// meridian flip on a GTi has been verified.
    #[serde(default = "default_flip_policy_enabled")]
    pub enabled: bool,

    /// Half-width of the target-HA window around the meridian where
    /// the flipped state is mechanically reachable. Targets with
    /// `|target_HA| > flip_range_hours` are unflippable: the slew
    /// planner uses normal pointing only and `DestinationSideOfPier`
    /// returns the current side. Valid range `(0, 0.95]`; the upper
    /// bound matches the Phase 1.1 hardware-verified headroom past
    /// counterweight-horizontal on the pre-flip side.
    #[serde(default = "default_flip_range_hours")]
    pub flip_range_hours: f64,
}

impl Default for FlipPolicy {
    fn default() -> Self {
        Self {
            enabled: default_flip_policy_enabled(),
            flip_range_hours: default_flip_range_hours(),
        }
    }
}

/// Outer bound on `FlipPolicy::flip_range_hours`. Larger values would
/// push the post-flip mechanical hour angle into the unverified
/// mirror of the Phase 4 counterweight-up CW exclusion zone. See the plan
/// `docs/plans/star-adventurer-gti-meridian-flip.md` §2.6.
pub const MAX_FLIP_RANGE_HOURS: f64 = 0.95;

impl FlipPolicy {
    /// Validate the policy against the constraints documented in the
    /// design doc and §2.6 of the plan. Returns `Err(message)` on a
    /// bad value; the caller surfaces it as a startup error.
    pub fn validate(&self) -> std::result::Result<(), String> {
        if !self.flip_range_hours.is_finite() {
            return Err(format!(
                "flip_policy.flip_range_hours must be finite, got {}",
                self.flip_range_hours
            ));
        }
        if self.flip_range_hours <= 0.0 || self.flip_range_hours > MAX_FLIP_RANGE_HOURS {
            return Err(format!(
                "flip_policy.flip_range_hours must be in (0, {MAX_FLIP_RANGE_HOURS}] hours, got {}",
                self.flip_range_hours
            ));
        }
        Ok(())
    }
}

/// Outer bound on [`MountConfig::tracking_guard_margin_hours`]. The
/// margin is operator-preference slack before the CW exclusion zone,
/// not a mechanical limit; this cap only catches a misconfiguration
/// (e.g. a value entered in degrees, or a unit mix-up) at startup. The
/// shipped default is `0.05` h.
pub const MAX_TRACKING_GUARD_MARGIN_HOURS: f64 = 1.0;

impl MountConfig {
    /// Validate the mount configuration after load. Returns
    /// `Err(message)` on a bad value; [`load_config`] surfaces it as a
    /// startup error so an out-of-range config fails fast instead of
    /// silently driving the mount with a bad parameter.
    ///
    /// These fields are bare `f64` with no newtype guarantees, so this
    /// method is the single place their documented ranges are enforced.
    pub fn validate(&self) -> std::result::Result<(), String> {
        self.flip_policy.validate()?;

        // CW exclusion zone. `min >= max` is the documented "disabled"
        // sentinel (used by tests and by operators whose geometry
        // differs — see the field docs), accepted as "no zone". An
        // *active* zone (`min < max`) must live in the folded
        // mechanical-HA domain `[-12, +12)` that the guard and slew
        // checks assume (neither does 24 h wrap handling).
        if !self.binding_zone_min_hours.is_finite() || !self.binding_zone_max_hours.is_finite() {
            return Err(format!(
                "binding_zone_min_hours / binding_zone_max_hours must be finite, got ({}, {})",
                self.binding_zone_min_hours, self.binding_zone_max_hours
            ));
        }
        if self.binding_zone_min_hours < self.binding_zone_max_hours
            && (self.binding_zone_min_hours < -12.0 || self.binding_zone_max_hours > 12.0)
        {
            return Err(format!(
                "an active CW exclusion zone must satisfy \
                 -12 <= binding_zone_min_hours < binding_zone_max_hours <= 12 (folded mech_HA), \
                 got ({}, {})",
                self.binding_zone_min_hours, self.binding_zone_max_hours
            ));
        }

        // Tracking-guard margin: finite, non-negative, capped to catch a
        // misconfiguration. The guard also sanitises bad values at
        // runtime, but a startup error is clearer to the operator.
        if !self.tracking_guard_margin_hours.is_finite()
            || self.tracking_guard_margin_hours < 0.0
            || self.tracking_guard_margin_hours > MAX_TRACKING_GUARD_MARGIN_HOURS
        {
            return Err(format!(
                "tracking_guard_margin_hours must be in [0, {MAX_TRACKING_GUARD_MARGIN_HOURS}] hours, got {}",
                self.tracking_guard_margin_hours
            ));
        }

        // Dec clip range: a real celestial-Dec interval within the
        // observable hemisphere.
        if !self.dec_min_degrees.is_finite() || !self.dec_max_degrees.is_finite() {
            return Err(format!(
                "dec_min_degrees / dec_max_degrees must be finite, got ({}, {})",
                self.dec_min_degrees, self.dec_max_degrees
            ));
        }
        if self.dec_min_degrees < -90.0
            || self.dec_max_degrees > 90.0
            || self.dec_min_degrees >= self.dec_max_degrees
        {
            return Err(format!(
                "dec_min_degrees / dec_max_degrees must satisfy -90 <= min < max <= 90, got ({}, {})",
                self.dec_min_degrees, self.dec_max_degrees
            ));
        }

        Ok(())
    }
}

/// Physical pose the operator powers the mount up in, expressed as
/// one of the Astro-Physics
/// ["Park Positions Defined"](https://astro-physics.info/tech_support/mounts/park-positions-defined.pdf)
/// positions.
///
/// The Sky-Watcher firmware resets its encoder counter to `(0, 0)`
/// every power-up. When `home_pose: Some(ApPark*)`, the driver
/// seeds the firmware encoder on connect so the codebase's coordinate
/// math interprets that zero against the operator's physical pose.
///
/// Each AP pose is mirror-symmetric between the Northern and Southern
/// Hemispheres around the observer's local meridian — the
/// counterweight-shaft direction inverts and the celestial-Dec sign of
/// the target inverts, so the codebase reading for each pose accounts
/// for the observer's hemisphere from `site_latitude_deg`. The
/// `codebase_*` accessor helpers do the hemisphere case-split
/// internally; callers pass `site_latitude_deg` unchanged and get back
/// a single signed encoder-reading value.
///
/// AP Park 4 and Park 5 are "east-side" (post-meridian-flip) poses;
/// the codebase reads them with `mech_HA` at the encoder wrap (`±12 h`)
/// for Park 4 (target on the meridian, anti-pole side) and at
/// `mech_HA = 0` for Park 5 (target on the anti-meridian, pole side
/// horizon), and the Dec encoder is past the celestial pole.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HomePose {
    /// AP Park 1. "RA horizontal" (Dec axis east-west horizontal,
    /// saddle on the *west* end, counterweight on the east end). OTA
    /// tube level, pointing at the polar-side horizon — north horizon
    /// for north observers, south horizon for south observers.
    ///
    /// AP table: `Dec = (90 − Latitude)` for north,
    /// `(−90 − Latitude)` for south. Codebase reading:
    /// `mech_HA = 0`, `dec_encoder = ±(90 − |latitude|)` (sign matches
    /// the hemisphere).
    #[serde(rename = "ap_park_1")]
    ApPark1,
    /// AP Park 2. "RA axis vertical, Dec = 0". OTA level facing the
    /// east horizon, counterweight shaft pointing down. The "RA
    /// axis vertical" description is the visual look near the equator;
    /// at any latitude the codebase reading is `mech_HA = −6 h`
    /// (target on the east-rising celestial equator), `dec_encoder
    /// = 0`. Hemisphere-independent.
    #[serde(rename = "ap_park_2")]
    ApPark2,
    /// AP Park 3 (also Sky-Watcher's power-on home). OTA along the
    /// polar axis pointing at the visible celestial pole — Polaris
    /// for north observers, SCP for south observers. Counterweight
    /// shaft along the anti-pole half of the polar axis.
    ///
    /// AP table: `Dec = 90` (visible pole). Codebase reading:
    /// `mech_HA = 0`, `dec_encoder = +90°` for north / `−90°` for
    /// south.
    #[serde(rename = "ap_park_3")]
    ApPark3,
    /// AP Park 4. East-side / post-meridian-flip equivalent of Park 1:
    /// saddle on the *east* end of the Dec axis, counterweight shaft
    /// horizontal pointing due west. OTA level facing the *anti-polar*
    /// horizon — south horizon for north observers, north horizon for
    /// south observers.
    ///
    /// AP table: `Dec = (−90 + Latitude)` for north,
    /// `(90 + Latitude)` for south. The target's celestial dec lands
    /// at `±(90 − |latitude|)` (sign anti-hemisphere), and the
    /// post-flip Dec encoder = `sign(dec) · (180 − |dec|) =
    /// ∓(90 + |latitude|)`. Codebase reading: `mech_HA = −12 h`
    /// (= `+12 h` via the encoder wrap),
    /// `dec_encoder = ∓(90 + |latitude|)`.
    #[serde(rename = "ap_park_4")]
    ApPark4,
    /// AP Park 5. East-side / post-meridian-flip equivalent of Park 1
    /// (mirror of Park 4 across the polar-axis plane): saddle on the
    /// *east* end, counterweight shaft horizontal pointing due west.
    /// OTA level facing the *polar-side* horizon — north horizon for
    /// north observers, south horizon for south observers.
    ///
    /// AP table: `Dec = (90 − Latitude)` for north,
    /// `(−90 − Latitude)` for south. The celestial dec is at the same
    /// magnitude as Park 1 but reached from the post-flip side, so
    /// the post-flip Dec encoder = `±(90 + |latitude|)` (sign matches
    /// the hemisphere). Codebase reading: `mech_HA = 0` (target on
    /// the anti-meridian, post-flip wrap brings mech_HA back to 0),
    /// `dec_encoder = ±(90 + |latitude|)`.
    ///
    /// (`Note: Park 5 shown below is only available in APCC and the
    /// AP V2 driver.` per the AP doc — it's an APCC extension, not on
    /// the keypad.)
    #[serde(rename = "ap_park_5")]
    ApPark5,
}

impl HomePose {
    /// Codebase-convention `mech_HA` (signed hours, `[−12, +12)`)
    /// corresponding to firmware encoder `(0, 0)` at this home pose
    /// for the configured latitude.
    ///
    /// AP defines each pose by the OTA pointing direction and which
    /// side of the mount the OTA tube is on (East or West, mechanical).
    /// The pier-side mechanical designation is the same for both
    /// hemispheres, but the driver's natural-vs-flipped pier convention
    /// flips between hemispheres (N natural = pierWest, S natural =
    /// pierEast — see `pre_flip_side` in `MountDevice`). So a pose like
    /// Park 1 ("OTA on west side") is the **natural side** in the
    /// North and the **flipped side** in the South — and its encoder
    /// representation differs accordingly.
    ///
    /// - **Natural side** (mech_HA = celestial HA): used when the
    ///   pose's mechanical pier matches the hemisphere's natural pier.
    /// - **Flipped side** (mech_HA = celestial HA + 12, folded): used
    ///   when the pose is on the opposite mechanical pier.
    ///
    /// | Pose | Celestial HA | Mech. pier | N: nat / flip | S: nat / flip |
    /// |------|--------------|-----------|---------------|----------------|
    /// | 1    | ±12          | West      | natural       | flipped        |
    /// | 2    | −6           | —         | natural       | natural        |
    /// | 3    | (pole)       | —         | natural       | natural        |
    /// | 4    | 0            | East      | flipped       | natural        |
    /// | 5    | ±12          | East      | flipped       | natural        |
    ///
    /// Concretely (Northern Hemisphere; Southern mirrors via the
    /// hemisphere case-split below):
    /// - Park 1 N: saddle west → `mech_HA = 0` (saddle in west half
    ///   of polar plane; OTA reaches north horizon via past-pole dec
    ///   rotation).
    /// - Park 4 N: saddle east → `mech_HA = −12` (encoder wrap;
    ///   saddle in east half; OTA reaches south horizon via past-
    ///   pole dec rotation).
    /// - Park 5 N: saddle east → `mech_HA = −12` (encoder wrap;
    ///   OTA reaches north horizon via natural-side dec rotation).
    /// - Parks 2 and 3 are hemisphere-neutral for mech_HA (`−6`,
    ///   saddle in the south-up direction, neither east nor west).
    pub fn codebase_mech_ha_hours(&self, _latitude_deg: f64) -> f64 {
        // We don't case-split on hemisphere for mech_HA: the saddle
        // east/west position is a function of the encoder alone,
        // which is hemisphere-independent. The dec encoder *is*
        // hemisphere-dependent — see [`codebase_dec_encoder_degrees`].
        match self {
            // Park 1: OTA west of mount → saddle west → mech_HA in
            // the (−6, +6) range. `mech_HA = 0` is the canonical
            // saddle-west position (dec axis east-west horizontal).
            Self::ApPark1 => 0.0,
            // Park 2 and Park 3 share the same RA position ("RA axis
            // vertical" per the AP doc); only the dec rotation differs.
            // Both put the dec axis south-up out of east-west horizontal
            // (`mech_HA = −6`).
            Self::ApPark2 => -6.0,
            Self::ApPark3 => -6.0,
            // Park 4: OTA east of mount → saddle east → `mech_HA` in
            // the wrap region. `mech_HA = −12` is the canonical
            // saddle-east position.
            Self::ApPark4 => -12.0,
            // Park 5: OTA east of mount (same saddle side as Park 4,
            // different dec rotation) → `mech_HA = −12`.
            Self::ApPark5 => -12.0,
        }
    }

    /// Codebase-convention Dec encoder reading (degrees, signed,
    /// `[−180, +180)`) corresponding to firmware encoder `(0, 0)` at
    /// this home pose, given the observer's latitude.
    ///
    /// Hemisphere-dependent because the OTA points at the visible
    /// pole / horizon, which has opposite celestial-Dec signs between
    /// hemispheres. The codebase dec_enc value is the rotation angle
    /// around the dec axis from the dec=0 reference (which itself is
    /// hemisphere-dependent at fixed mech_HA).
    pub fn codebase_dec_encoder_degrees(&self, latitude_deg: f64) -> f64 {
        let northern = latitude_deg >= 0.0;
        let lat_abs = latitude_deg.abs();
        // Magnitude common to Parks 1, 4, 5 — the celestial Dec at the
        // polar-side / anti-polar horizon at this latitude.
        let horizon_dec_mag = 90.0 - lat_abs;
        match self {
            // Park 1: saddle west (mech_HA=0). OTA at polar-side
            // horizon — north horizon for N (alt=0, az=0), south
            // horizon for S. From the saddle-west position at
            // mech_HA=0 the OTA reaches the polar-side horizon via
            // a past-pole rotation: `dec_enc ≈ ±(90 + |lat|)` (sign
            // matches hemisphere; magnitude > 90 indicates the
            // past-pole encoding).
            Self::ApPark1 => {
                if northern {
                    90.0 + lat_abs
                } else {
                    -(90.0 + lat_abs)
                }
            }
            Self::ApPark2 => 0.0,
            Self::ApPark3 => {
                // OTA at the visible celestial pole. Both hemispheres
                // encode this as `dec_enc = ±90°` (sign matches
                // hemisphere — celestial pole at +Dec in N, −Dec
                // in S).
                if northern {
                    90.0
                } else {
                    -90.0
                }
            }
            // Park 4: saddle east (mech_HA=-12). OTA at the anti-
            // polar horizon — south for N (alt=0, az=180), north
            // for S. Past-pole rotation gives `dec_enc = ∓(90+|lat|)`
            // (sign opposite hemisphere).
            Self::ApPark4 => {
                if northern {
                    -(90.0 + lat_abs)
                } else {
                    90.0 + lat_abs
                }
            }
            // Park 5: saddle east (mech_HA=-12, same as Park 4) but
            // OTA at the polar-side horizon (180° around dec axis
            // from Park 4). `dec_enc = ±(90 − |lat|)` (natural-side
            // encoding; |dec_enc| < 90).
            Self::ApPark5 => {
                if northern {
                    horizon_dec_mag
                } else {
                    -horizon_dec_mag
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TrackingRateName {
    #[default]
    Sidereal,
}

fn default_baud_rate() -> u32 {
    115_200
}
fn default_command_timeout() -> Duration {
    Duration::from_secs(2)
}
fn default_polling_interval() -> Duration {
    Duration::from_millis(200)
}
fn default_udp_port() -> u16 {
    11_880
}
fn default_discovery_port() -> Option<u16> {
    Some(ascom_alpaca::discovery::DEFAULT_DISCOVERY_PORT)
}
fn default_settle_after_slew() -> Duration {
    Duration::from_secs(2)
}
fn default_binding_zone_min_hours() -> f64 {
    0.95
}
fn default_binding_zone_max_hours() -> f64 {
    11.05
}
fn default_tracking_guard_margin_hours() -> f64 {
    0.05
}
fn default_dec_min_degrees() -> f64 {
    -90.0
}
fn default_dec_max_degrees() -> f64 {
    90.0
}
fn default_flip_policy_enabled() -> bool {
    false
}
fn default_flip_range_hours() -> f64 {
    0.5
}
fn default_true() -> bool {
    true
}

impl Default for UsbConfig {
    fn default() -> Self {
        Self {
            port: "/dev/ttyACM0".to_string(),
            baud_rate: default_baud_rate(),
            command_timeout: default_command_timeout(),
            polling_interval: default_polling_interval(),
        }
    }
}

impl Default for UdpConfig {
    fn default() -> Self {
        Self {
            // GTi AP-mode address (192.168.4.1) and a typical bind
            // address on the same /24. Constructed via `Ipv4Addr::new`
            // rather than parsing a string so `Default::default()`
            // can't panic on a typo — the compiler validates the
            // octet literals.
            address: IpAddr::V4(Ipv4Addr::new(192, 168, 4, 1)),
            port: default_udp_port(),
            bind_address: IpAddr::V4(Ipv4Addr::new(192, 168, 4, 2)),
            command_timeout: default_command_timeout(),
            polling_interval: default_polling_interval(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 11_117,
            discovery_port: default_discovery_port(),
            tls: None,
            auth: None,
        }
    }
}

impl Default for MountConfig {
    fn default() -> Self {
        Self {
            name: "Star Adventurer GTi".to_string(),
            unique_id: "skywatcher-sa-gti-001".to_string(),
            description: "Sky-Watcher Star Adventurer GTi German Equatorial Mount".to_string(),
            enabled: true,
            site_latitude_deg: 0.0,
            site_longitude_deg: 0.0,
            site_elevation_m: 0.0,
            settle_after_slew: default_settle_after_slew(),
            tracking_rate: TrackingRateName::Sidereal,
            binding_zone_min_hours: default_binding_zone_min_hours(),
            binding_zone_max_hours: default_binding_zone_max_hours(),
            tracking_guard_margin_hours: default_tracking_guard_margin_hours(),
            dec_min_degrees: default_dec_min_degrees(),
            dec_max_degrees: default_dec_max_degrees(),
            park_ra_ticks: None,
            park_dec_ticks: None,
            flip_policy: FlipPolicy::default(),
            home_pose: None,
        }
    }
}

/// Load a [`Config`] from a JSON file.
pub fn load_config(path: &Path) -> std::result::Result<Config, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = serde_json::from_str(&content)?;
    config
        .mount
        .validate()
        .map_err(|msg| std::io::Error::new(std::io::ErrorKind::InvalidData, msg))?;
    Ok(config)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::unreachable)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_match_the_design_doc() {
        let cfg = Config::default();
        assert_eq!(cfg.server.port, 11_117);
        assert_eq!(cfg.mount.name, "Star Adventurer GTi");
        assert_eq!(cfg.mount.unique_id, "skywatcher-sa-gti-001");
        assert!(cfg.mount.enabled);
        assert_eq!(cfg.mount.tracking_rate, TrackingRateName::Sidereal);
        assert_eq!(cfg.mount.settle_after_slew, Duration::from_secs(2));
        assert!(matches!(cfg.transport, TransportConfig::Usb(_)));
    }

    #[test]
    fn usb_transport_default_uses_recommended_baud_rate() {
        let usb = UsbConfig::default();
        // Spec says 9600; in practice GTi USB also accepts 115200, which we
        // recommend (matches EQMOD docs).
        assert_eq!(usb.baud_rate, 115_200);
        assert_eq!(usb.command_timeout, Duration::from_secs(2));
        assert_eq!(usb.polling_interval, Duration::from_millis(200));
    }

    #[test]
    fn udp_transport_default_targets_the_ap_mode_address() {
        let udp = UdpConfig::default();
        assert_eq!(udp.address.to_string(), "192.168.4.1");
        assert_eq!(udp.port, 11_880);
        // bind_address is mandatory for UDP and must be on the 192.168.4.0/24
        // subnet when the mount is in AP mode.
        assert!(udp.bind_address.to_string().starts_with("192.168.4."));
    }

    #[test]
    fn port_label_usb_returns_serial_path_verbatim() {
        let cfg = TransportConfig::Usb(UsbConfig {
            port: "/dev/serial/by-id/usb-Some_Device-port0".into(),
            ..UsbConfig::default()
        });
        assert_eq!(cfg.port_label(), "/dev/serial/by-id/usb-Some_Device-port0");
    }

    #[test]
    fn port_label_udp_formats_address_and_port() {
        let cfg = TransportConfig::Udp(UdpConfig::default());
        // UdpConfig::default() targets the GTi's AP-mode address on the
        // documented port.
        assert_eq!(cfg.port_label(), "192.168.4.1:11880");
    }

    #[test]
    fn port_label_udp_brackets_ipv6_addresses() {
        // `SocketAddr`'s Display brackets v6 so the resulting label is
        // unambiguous (without brackets `fe80::1:11880` could read as
        // address `fe80::1` port `11880` *or* address `fe80::1:11880`
        // with no port). Matches the canonical operator-typing form.
        let cfg = TransportConfig::Udp(UdpConfig {
            address: "fe80::1".parse().expect("parse v6"),
            port: 11880,
            ..UdpConfig::default()
        });
        assert_eq!(cfg.port_label(), "[fe80::1]:11880");
    }

    #[test]
    fn config_round_trips_through_json() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).expect("serialise");
        let back: Config = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.server.port, cfg.server.port);
        assert_eq!(back.mount.name, cfg.mount.name);
    }

    #[test]
    fn transport_config_deserialises_usb_variant() {
        let json = r#"{
            "kind": "usb",
            "port": "/dev/ttyACM0",
            "baud_rate": 9600,
            "command_timeout": "1s",
            "polling_interval": "100ms"
        }"#;
        let t: TransportConfig = serde_json::from_str(json).expect("usb variant");
        match t {
            TransportConfig::Usb(usb) => {
                assert_eq!(usb.port, "/dev/ttyACM0");
                assert_eq!(usb.baud_rate, 9600);
                assert_eq!(usb.command_timeout, Duration::from_secs(1));
                assert_eq!(usb.polling_interval, Duration::from_millis(100));
            }
            other => panic!("expected Usb, got {other:?}"),
        }
    }

    #[test]
    fn mount_config_park_ticks_default_to_none() {
        let cfg = MountConfig::default();
        assert_eq!(cfg.park_ra_ticks, None);
        assert_eq!(cfg.park_dec_ticks, None);
    }

    #[test]
    fn mount_config_deserialises_missing_park_ticks_as_none() {
        // Existing config files written before the SetPark feature
        // landed do not carry `park_ra_ticks` / `park_dec_ticks`; the
        // driver must read them as `None` rather than failing.
        let json = r#"{
            "name": "T",
            "unique_id": "t-001",
            "description": "T",
            "site_latitude_deg": 0.0,
            "site_longitude_deg": 0.0
        }"#;
        let m: MountConfig = serde_json::from_str(json).expect("deserialise");
        assert_eq!(m.park_ra_ticks, None);
        assert_eq!(m.park_dec_ticks, None);
    }

    #[test]
    fn mount_config_round_trips_park_ticks_through_json() {
        let cfg = MountConfig {
            park_ra_ticks: Some(8000),
            park_dec_ticks: Some(-3000),
            ..MountConfig::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialise");
        let back: MountConfig = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.park_ra_ticks, Some(8000));
        assert_eq!(back.park_dec_ticks, Some(-3000));
    }

    #[test]
    fn home_pose_default_is_none_for_backward_compat() {
        // `home_pose: None` means "no encoder seeding on connect" —
        // the codebase's pre-Phase-6 behaviour. Operators powering up
        // at an AP park position opt in by setting `home_pose:
        // "ap_park_<n>"`.
        let cfg = MountConfig::default();
        assert_eq!(cfg.home_pose, None);
    }

    #[test]
    fn home_pose_deserialises_from_snake_case() {
        for (json, expected) in [
            (r#""ap_park_1""#, HomePose::ApPark1),
            (r#""ap_park_2""#, HomePose::ApPark2),
            (r#""ap_park_3""#, HomePose::ApPark3),
            (r#""ap_park_4""#, HomePose::ApPark4),
            (r#""ap_park_5""#, HomePose::ApPark5),
        ] {
            let got: HomePose = serde_json::from_str(json).expect(json);
            assert_eq!(got, expected, "json input {json}");
        }
    }

    #[test]
    fn home_pose_ap_park_1_matches_ap_table_both_hemispheres() {
        // AP Park 1: OTA on west side of mount, level, facing the
        // polar-side horizon (N: north horizon, S: south horizon).
        // Saddle is on the west end of the dec axis → mech_HA = 0
        // (canonical saddle-west position; dec axis east-west
        // horizontal). To reach the polar-side horizon from this
        // saddle position, the dec axis must rotate past the pole:
        // `dec_enc = ±(90 + |lat|)` (sign matches hemisphere).
        // Verified on hardware at lat 32.7°N (2026-05-17): the
        // encoder pose previously labelled Park 1 N (mech_HA=−12,
        // dec_enc=+57.3) physically corresponds to Park 5 N (saddle
        // east) — see commit fixing the swap.
        let n = 32.7_f64;
        let s = -33.0_f64;
        assert!(
            (HomePose::ApPark1.codebase_dec_encoder_degrees(n) - 122.7).abs() < 1e-9,
            "Park 1 N dec_enc at 32.7°: got {}",
            HomePose::ApPark1.codebase_dec_encoder_degrees(n)
        );
        assert!(
            (HomePose::ApPark1.codebase_dec_encoder_degrees(s) - (-123.0)).abs() < 1e-9,
            "Park 1 S dec_enc at −33°: got {}",
            HomePose::ApPark1.codebase_dec_encoder_degrees(s)
        );
        assert_eq!(HomePose::ApPark1.codebase_mech_ha_hours(n), 0.0);
        assert_eq!(HomePose::ApPark1.codebase_mech_ha_hours(s), 0.0);
    }

    #[test]
    fn home_pose_ap_park_2_is_hemisphere_independent() {
        // Park 2: "RA axis vertical, Dec = 0", both hemispheres.
        // OTA at east-rising celestial equator → celestial HA = −6 h,
        // celestial dec = 0. Both hemispheres land on the natural side
        // (|dec_enc| = 0 ≤ 90), so encoder = celestial dec for both.
        for lat in [-89.0, -33.0, 0.0, 32.7, 89.0] {
            assert_eq!(
                HomePose::ApPark2.codebase_mech_ha_hours(lat),
                -6.0,
                "Park 2 mech_HA at lat {lat}"
            );
            assert_eq!(
                HomePose::ApPark2.codebase_dec_encoder_degrees(lat),
                0.0,
                "Park 2 dec at lat {lat}"
            );
        }
    }

    #[test]
    fn home_pose_ap_park_3_visible_pole_inverts_with_hemisphere() {
        // Park 3 / Sky-Watcher home: OTA along polar axis at the
        // visible pole. Celestial dec = +90 (N) / −90 (S). Natural
        // side for both hemispheres → encoder = celestial dec.
        // Verified on hardware at lat 32.7°N (2026-05-15): mech_HA =
        // −6 h and dec_enc = +90 leaves the OTA pointing at the NCP.
        assert_eq!(HomePose::ApPark3.codebase_dec_encoder_degrees(32.7), 90.0);
        assert_eq!(HomePose::ApPark3.codebase_dec_encoder_degrees(-33.0), -90.0);
        // Boundary: lat = 0 falls into the "north" arm via `>= 0`.
        assert_eq!(HomePose::ApPark3.codebase_dec_encoder_degrees(0.0), 90.0);
        assert_eq!(HomePose::ApPark3.codebase_mech_ha_hours(32.7), -6.0);
        assert_eq!(HomePose::ApPark3.codebase_mech_ha_hours(-33.0), -6.0);
    }

    #[test]
    fn home_pose_ap_park_4_dec_at_lat_32() {
        // AP Park 4: OTA on east side of mount, level, facing the
        // anti-polar horizon (N: south horizon, S: north horizon).
        // Saddle east → mech_HA = −12. AP celestial dec:
        //   N = (−90 + Lat) = −(90 − |lat|),
        //   S = (+90 + Lat) = +(90 − |lat|).
        // Past-pole encoding (180° around dec axis from natural-side
        // reference at this mech_HA): `dec_enc = ∓(90 + |lat|)`
        // (sign opposite hemisphere).
        assert!(
            (HomePose::ApPark4.codebase_dec_encoder_degrees(32.7) - (-122.7)).abs() < 1e-9,
            "Park 4 N at 32.7°: got {}",
            HomePose::ApPark4.codebase_dec_encoder_degrees(32.7)
        );
        assert!(
            (HomePose::ApPark4.codebase_dec_encoder_degrees(-33.0) - 123.0).abs() < 1e-9,
            "Park 4 S at −33°: got {}",
            HomePose::ApPark4.codebase_dec_encoder_degrees(-33.0)
        );
        assert_eq!(HomePose::ApPark4.codebase_mech_ha_hours(32.7), -12.0);
        assert_eq!(HomePose::ApPark4.codebase_mech_ha_hours(-33.0), -12.0);
    }

    #[test]
    fn home_pose_ap_park_5_matches_ap_table_both_hemispheres() {
        // AP Park 5 (APCC / AP V2 driver only): OTA on east side of
        // mount, level, facing the polar-side horizon (N: north,
        // S: south). Saddle east → mech_HA = −12. Natural-side dec
        // rotation around the dec axis at this mech_HA reaches the
        // polar-side horizon: `dec_enc = ±(90 − |lat|)` (sign
        // matches hemisphere).
        // Hardware-verified at lat 32.7°N (2026-05-17): slewing to
        // celestial (LST+12, +57.28) lands at encoder (−cpr/2,
        // +461,815), saddle east, OTA at north horizon — matches AP
        // Park 5 N visually.
        assert!(
            (HomePose::ApPark5.codebase_dec_encoder_degrees(32.7) - 57.3).abs() < 1e-9,
            "Park 5 N at 32.7°: got {}",
            HomePose::ApPark5.codebase_dec_encoder_degrees(32.7)
        );
        assert!(
            (HomePose::ApPark5.codebase_dec_encoder_degrees(-33.0) - (-57.0)).abs() < 1e-9,
            "Park 5 S at −33°: got {}",
            HomePose::ApPark5.codebase_dec_encoder_degrees(-33.0)
        );
        assert_eq!(HomePose::ApPark5.codebase_mech_ha_hours(32.7), -12.0);
        assert_eq!(HomePose::ApPark5.codebase_mech_ha_hours(-33.0), -12.0);
    }

    #[test]
    fn home_pose_park1_park5_share_celestial_target_opposite_pier() {
        // Park 1 (saddle west) and Park 5 (saddle east) point at the
        // same celestial coordinates (polar-side horizon) but on
        // opposite mechanical pier sides. The dec-encoder magnitudes
        // therefore differ by the "past the pole" offset:
        // `|natural| + |flipped| = |dec| + (180 − |dec|) = 180°`.
        for lat in [-45.0, -33.0, 32.7, 45.0] {
            let p1 = HomePose::ApPark1.codebase_dec_encoder_degrees(lat).abs();
            let p5 = HomePose::ApPark5.codebase_dec_encoder_degrees(lat).abs();
            assert!(
                (p1 + p5 - 180.0).abs() < 1e-9,
                "lat {lat}: |p1| {p1}, |p5| {p5}, sum {} (expected 180)",
                p1 + p5
            );
        }
    }

    #[test]
    fn home_pose_park4_and_park5_share_pier_opposite_celestial_dec() {
        // Park 4 (OTA facing anti-polar horizon) and Park 5 (OTA
        // facing polar-side horizon) are on the same mechanical pier
        // (East, `mech_HA = −12`), and their celestial Decs are equal
        // in magnitude but opposite in sign. The Park 4 encoder uses
        // past-pole encoding (|dec_enc| > 90); Park 5 uses
        // natural-side encoding (|dec_enc| < 90). The magnitudes
        // therefore add to 180°, and the signs differ — i.e. their
        // encoder values are equal in magnitude with opposite signs
        // iff one is past-pole and the other is not.
        for lat in [-45.0, -33.0, 32.7, 45.0] {
            assert_eq!(HomePose::ApPark4.codebase_mech_ha_hours(lat), -12.0);
            assert_eq!(HomePose::ApPark5.codebase_mech_ha_hours(lat), -12.0);
            let p4_abs = HomePose::ApPark4.codebase_dec_encoder_degrees(lat).abs();
            let p5_abs = HomePose::ApPark5.codebase_dec_encoder_degrees(lat).abs();
            assert!(
                (p4_abs + p5_abs - 180.0).abs() < 1e-9,
                "lat {lat}: |p4| {p4_abs}, |p5| {p5_abs}"
            );
        }
    }

    #[test]
    fn home_pose_round_trips_through_mount_config_json() {
        // None round-trips as the missing/null field.
        let cfg = MountConfig {
            home_pose: None,
            ..MountConfig::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialise");
        let back: MountConfig = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.home_pose, None, "None round trip");

        // Every AP park variant round-trips through Some(...).
        for pose in [
            HomePose::ApPark1,
            HomePose::ApPark2,
            HomePose::ApPark3,
            HomePose::ApPark4,
            HomePose::ApPark5,
        ] {
            let cfg = MountConfig {
                home_pose: Some(pose),
                ..MountConfig::default()
            };
            let json = serde_json::to_string(&cfg).expect("serialise");
            let back: MountConfig = serde_json::from_str(&json).expect("deserialise");
            assert_eq!(back.home_pose, Some(pose), "round trip for {pose:?}");
        }
    }

    #[test]
    fn mount_config_deserialises_missing_home_pose_as_none() {
        let json = r#"{
            "name": "T",
            "unique_id": "t-001",
            "description": "T",
            "site_latitude_deg": 0.0,
            "site_longitude_deg": 0.0
        }"#;
        let m: MountConfig = serde_json::from_str(json).expect("deserialise");
        assert_eq!(m.home_pose, None);
    }

    #[test]
    fn flip_policy_default_is_disabled_with_half_hour_range() {
        // The shipped default disables every flip code path — a fresh
        // install must behave identically to pre-Phase-6 builds until
        // the operator explicitly opts in on a hardware-validated mount.
        let p = FlipPolicy::default();
        assert!(!p.enabled);
        assert!(
            (p.flip_range_hours - 0.5).abs() < f64::EPSILON,
            "got {}",
            p.flip_range_hours
        );
    }

    #[test]
    fn mount_config_default_includes_disabled_flip_policy() {
        let cfg = MountConfig::default();
        assert!(!cfg.flip_policy.enabled);
    }

    #[test]
    fn mount_config_deserialises_missing_flip_policy_as_default() {
        // Existing config files written before Phase 6 do not carry
        // `flip_policy`; the driver must read them as
        // `FlipPolicy::default()` rather than failing.
        let json = r#"{
            "name": "T",
            "unique_id": "t-001",
            "description": "T",
            "site_latitude_deg": 0.0,
            "site_longitude_deg": 0.0
        }"#;
        let m: MountConfig = serde_json::from_str(json).expect("deserialise");
        assert!(!m.flip_policy.enabled);
        assert!((m.flip_policy.flip_range_hours - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn mount_config_default_tracking_guard_margin_is_45s_of_drift() {
        // 0.05 h ≈ 45 s of sidereal drift — the shipped default the
        // design doc's §"Tracking-time safety guard" documents.
        let cfg = MountConfig::default();
        assert!(
            (cfg.tracking_guard_margin_hours - 0.05).abs() < f64::EPSILON,
            "got {}",
            cfg.tracking_guard_margin_hours
        );
    }

    #[test]
    fn mount_config_deserialises_missing_tracking_guard_margin_as_default() {
        // Configs written before the tracking-time safety guard landed
        // do not carry `tracking_guard_margin_hours`; the driver must
        // read them as the 0.05 h default rather than failing.
        let json = r#"{
            "name": "T",
            "unique_id": "t-001",
            "description": "T",
            "site_latitude_deg": 0.0,
            "site_longitude_deg": 0.0
        }"#;
        let m: MountConfig = serde_json::from_str(json).expect("deserialise");
        assert!((m.tracking_guard_margin_hours - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn mount_config_validate_accepts_defaults() {
        MountConfig::default().validate().unwrap();
    }

    #[test]
    fn mount_config_validate_accepts_disabled_binding_zone() {
        // `min >= max` is the documented disable sentinel — must pass.
        MountConfig {
            binding_zone_min_hours: 24.0,
            binding_zone_max_hours: 0.0,
            ..Default::default()
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn mount_config_validate_delegates_to_flip_policy() {
        // Previously dead: an out-of-range flip_range_hours is now
        // rejected because `load_config` calls `validate`.
        let m = MountConfig {
            flip_policy: FlipPolicy {
                flip_range_hours: 5.0, // > MAX_FLIP_RANGE_HOURS
                ..Default::default()
            },
            ..Default::default()
        };
        let err = m.validate().unwrap_err();
        assert!(err.contains("flip_range_hours"), "got {err}");
    }

    #[test]
    fn mount_config_validate_rejects_margin_above_cap() {
        let m = MountConfig {
            tracking_guard_margin_hours: MAX_TRACKING_GUARD_MARGIN_HOURS + 0.1,
            ..Default::default()
        };
        let err = m.validate().unwrap_err();
        assert!(err.contains("tracking_guard_margin_hours"), "got {err}");
    }

    #[test]
    fn mount_config_validate_rejects_negative_margin() {
        MountConfig {
            tracking_guard_margin_hours: -0.01,
            ..Default::default()
        }
        .validate()
        .unwrap_err();
    }

    #[test]
    fn mount_config_validate_rejects_non_finite_margin() {
        MountConfig {
            tracking_guard_margin_hours: f64::NAN,
            ..Default::default()
        }
        .validate()
        .unwrap_err();
    }

    #[test]
    fn mount_config_validate_rejects_non_finite_binding_zone() {
        MountConfig {
            binding_zone_min_hours: f64::INFINITY,
            ..Default::default()
        }
        .validate()
        .unwrap_err();
    }

    #[test]
    fn mount_config_validate_rejects_active_zone_outside_folded_range() {
        // An active zone (min < max) must stay within folded mech_HA
        // [-12, +12); a max beyond +12 is out of the domain the guard
        // and slew checks assume.
        let m = MountConfig {
            binding_zone_min_hours: 0.95,
            binding_zone_max_hours: 20.0,
            ..Default::default()
        };
        let err = m.validate().unwrap_err();
        assert!(err.contains("active CW exclusion zone"), "got {err}");
    }

    #[test]
    fn mount_config_validate_rejects_inverted_dec_range() {
        let m = MountConfig {
            dec_min_degrees: 10.0,
            dec_max_degrees: -10.0,
            ..Default::default()
        };
        let err = m.validate().unwrap_err();
        assert!(err.contains("dec_"), "got {err}");
    }

    #[test]
    fn mount_config_validate_rejects_out_of_range_dec() {
        MountConfig {
            dec_min_degrees: -100.0,
            ..Default::default()
        }
        .validate()
        .unwrap_err();
    }

    #[test]
    fn load_config_rejects_an_out_of_range_value() {
        // End-to-end: validation is wired into `load_config`, so a bad
        // value in the file fails the load rather than being silently
        // accepted. flip_range_hours = 5.0 is out of (0, 0.95].
        use std::io::Write;
        let cfg = Config {
            mount: MountConfig {
                flip_policy: FlipPolicy {
                    flip_range_hours: 5.0,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Config::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialise config");
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(json.as_bytes()).expect("write temp config");
        let err = load_config(f.path()).unwrap_err();
        assert!(
            err.to_string().contains("flip_range_hours"),
            "expected a flip_range_hours validation error, got: {err}"
        );
    }

    #[test]
    fn mount_config_round_trips_enabled_flip_policy_through_json() {
        let cfg = MountConfig {
            flip_policy: FlipPolicy {
                enabled: true,
                flip_range_hours: 0.7,
            },
            ..MountConfig::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialise");
        let back: MountConfig = serde_json::from_str(&json).expect("deserialise");
        assert!(back.flip_policy.enabled);
        assert!((back.flip_policy.flip_range_hours - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn flip_policy_deserialises_with_partial_fields() {
        // serde_default on each field means a partial block — only one
        // key present — fills the other in from the default.
        let json = r#"{"enabled": true}"#;
        let p: FlipPolicy = serde_json::from_str(json).expect("deserialise");
        assert!(p.enabled);
        assert!((p.flip_range_hours - 0.5).abs() < f64::EPSILON);

        let json = r#"{"flip_range_hours": 0.25}"#;
        let p: FlipPolicy = serde_json::from_str(json).expect("deserialise");
        assert!(!p.enabled);
        assert!((p.flip_range_hours - 0.25).abs() < f64::EPSILON);
    }

    #[test]
    fn flip_policy_validate_accepts_defaults() {
        FlipPolicy::default().validate().unwrap();
    }

    #[test]
    fn flip_policy_validate_accepts_upper_bound() {
        let p = FlipPolicy {
            enabled: true,
            flip_range_hours: MAX_FLIP_RANGE_HOURS,
        };
        p.validate().unwrap();
    }

    #[test]
    fn flip_policy_validate_rejects_zero() {
        let p = FlipPolicy {
            enabled: true,
            flip_range_hours: 0.0,
        };
        let err = p.validate().unwrap_err();
        assert!(err.contains("flip_range_hours"), "got: {err}");
    }

    #[test]
    fn flip_policy_validate_rejects_negative() {
        let p = FlipPolicy {
            enabled: true,
            flip_range_hours: -0.1,
        };
        p.validate().unwrap_err();
    }

    #[test]
    fn flip_policy_validate_rejects_above_upper_bound() {
        let p = FlipPolicy {
            enabled: true,
            flip_range_hours: MAX_FLIP_RANGE_HOURS + 1e-6,
        };
        let err = p.validate().unwrap_err();
        assert!(err.contains("flip_range_hours"), "got: {err}");
    }

    #[test]
    fn flip_policy_validate_rejects_non_finite() {
        let p = FlipPolicy {
            enabled: true,
            flip_range_hours: f64::INFINITY,
        };
        p.validate().unwrap_err();
        let p = FlipPolicy {
            enabled: true,
            flip_range_hours: f64::NAN,
        };
        p.validate().unwrap_err();
    }

    #[test]
    fn transport_config_deserialises_udp_variant_with_bind_address() {
        let json = r#"{
            "kind": "udp",
            "address": "192.168.4.1",
            "port": 11880,
            "bind_address": "192.168.4.7",
            "command_timeout": "2s",
            "polling_interval": "200ms"
        }"#;
        let t: TransportConfig = serde_json::from_str(json).expect("udp variant");
        match t {
            TransportConfig::Udp(udp) => {
                assert_eq!(udp.address.to_string(), "192.168.4.1");
                assert_eq!(udp.bind_address.to_string(), "192.168.4.7");
            }
            other => panic!("expected Udp, got {other:?}"),
        }
    }
}
