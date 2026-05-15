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

    /// Safe RA mechanical-hour-angle envelope. Slews / syncs whose
    /// target falls outside `[ra_min_hours, ra_max_hours]` are
    /// rejected with `INVALID_VALUE` and never reach the wire.
    ///
    /// `mech_HA = 0` is "OTA on the meridian, counterweight down" on
    /// a polar-aligned Northern-Hemisphere setup. `±6 h` is the
    /// counterweight-horizontal east / west position; the GTi's
    /// hardware-verified mechanical limit sits at `±6.99 h` (reached
    /// cleanly with no audible motor stress or counterweight-pier
    /// contact during the 2026-05-13 hardware test), aligning with
    /// INDI eqmod's baked-in `±7 h` envelope for every Sky-Watcher
    /// mount (`zeroRAEncoder ± (totalRAEncoder/4 +
    /// totalRAEncoder/24)` in `eqmodbase.cpp::Goto`).
    ///
    /// Defaults are `[-6.95, +6.95]` — `0.05 h` (`3 arcmin`) inside
    /// the mechanical limit. The buffer covers two needs at once:
    /// the ASCOM `SlewToCoordinates(ra, dec)` round-trip means the
    /// driver re-reads LST a few tens of ms after the client
    /// computed the target, so a target quantised exactly to the
    /// limit would drift past it; and the deferred Phase 2
    /// meridian-flip planner will need headroom between the
    /// configured envelope and the mechanical stops to plan
    /// multi-stage flip slews. Tune narrower if your specific setup
    /// (mount-head-extension, OTA length, cable routing) clears
    /// less; tune wider only after verifying the extra travel on
    /// hardware.
    #[serde(default = "default_ra_min_hours")]
    pub ra_min_hours: f64,
    #[serde(default = "default_ra_max_hours")]
    pub ra_max_hours: f64,

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
/// mirror of the Phase 4 counterweight-up binding zone. See the plan
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
    /// corresponding to firmware encoder `(0, 0)` at this home pose.
    ///
    /// `mech_HA` reflects the RA-axis rotation, independent of pier
    /// side. Park 1, Park 3, and Park 5 all have the dec axis
    /// east-west horizontal (`mech_HA = 0`); Park 2 has the OTA at
    /// the east horizon, which puts target HA = −6 h regardless of
    /// hemisphere; Park 4 has the target on the meridian but
    /// approached from the post-flip side, which folds to `−12 h` at
    /// the encoder wrap.
    pub fn codebase_mech_ha_hours(&self) -> f64 {
        match self {
            Self::ApPark1 => 0.0,
            Self::ApPark2 => -6.0,
            Self::ApPark3 => 0.0,
            Self::ApPark4 => -12.0,
            Self::ApPark5 => 0.0,
        }
    }

    /// Codebase-convention Dec encoder reading (degrees, signed,
    /// `[−180, +180)`) corresponding to firmware encoder `(0, 0)` at
    /// this home pose, given the observer's latitude.
    ///
    /// Hemisphere handling: each AP pose's celestial-Dec target sign
    /// inverts between Northern and Southern observers (the OTA
    /// points at the visible pole / horizon, which has opposite Dec
    /// signs). This helper folds that into a single signed encoder
    /// reading.
    pub fn codebase_dec_encoder_degrees(&self, latitude_deg: f64) -> f64 {
        let northern = latitude_deg >= 0.0;
        let lat_abs = latitude_deg.abs();
        match self {
            Self::ApPark1 => {
                // Celestial dec at the polar-side horizon = ±(90 − |lat|),
                // pre-flip side → encoder = celestial dec.
                if northern {
                    90.0 - lat_abs
                } else {
                    -(90.0 - lat_abs)
                }
            }
            Self::ApPark2 => 0.0,
            Self::ApPark3 => {
                // OTA at the visible celestial pole. Encoder = ±90°.
                if northern {
                    90.0
                } else {
                    -90.0
                }
            }
            Self::ApPark4 => {
                // Post-flip, target on the anti-polar horizon at the
                // meridian. Celestial dec = ∓(90 − |lat|) (sign
                // opposite the hemisphere). Post-flip Dec encoder =
                // sign(dec) · (180 − |dec|) = ∓(90 + |lat|).
                if northern {
                    -(90.0 + lat_abs)
                } else {
                    90.0 + lat_abs
                }
            }
            Self::ApPark5 => {
                // Post-flip, target on the polar-side horizon at the
                // anti-meridian. Celestial dec = ±(90 − |lat|) (sign
                // matches the hemisphere). Post-flip Dec encoder =
                // sign(dec) · (180 − |dec|) = ±(90 + |lat|).
                if northern {
                    90.0 + lat_abs
                } else {
                    -(90.0 + lat_abs)
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
fn default_ra_min_hours() -> f64 {
    -6.95
}
fn default_ra_max_hours() -> f64 {
    6.95
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
            ra_min_hours: default_ra_min_hours(),
            ra_max_hours: default_ra_max_hours(),
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
    Ok(config)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
        // AP table: North Dec = (90 - Lat), South Dec = (-90 - Lat).
        // North: lat 32.7° → +57.3°. South: lat -33° → -57°.
        assert!(
            (HomePose::ApPark1.codebase_dec_encoder_degrees(32.7) - 57.3).abs() < 1e-9,
            "Park 1 N at 32.7°: got {}",
            HomePose::ApPark1.codebase_dec_encoder_degrees(32.7)
        );
        assert!(
            (HomePose::ApPark1.codebase_dec_encoder_degrees(-33.0) - (-57.0)).abs() < 1e-9,
            "Park 1 S at −33°: got {}",
            HomePose::ApPark1.codebase_dec_encoder_degrees(-33.0)
        );
        // mech_HA is 0 for both hemispheres (RA horizontal).
        assert_eq!(HomePose::ApPark1.codebase_mech_ha_hours(), 0.0);
    }

    #[test]
    fn home_pose_ap_park_2_is_hemisphere_independent() {
        // Park 2: "RA axis vertical, Dec = 0", both hemispheres.
        // The OTA points at the east-rising celestial equator → target
        // HA = −6 h, dec = 0, regardless of latitude.
        assert_eq!(HomePose::ApPark2.codebase_mech_ha_hours(), -6.0);
        for lat in [-89.0, -33.0, 0.0, 32.7, 89.0] {
            assert_eq!(
                HomePose::ApPark2.codebase_dec_encoder_degrees(lat),
                0.0,
                "Park 2 dec at lat {lat}"
            );
        }
    }

    #[test]
    fn home_pose_ap_park_3_visible_pole_inverts_with_hemisphere() {
        // AP Park 3 / Sky-Watcher home: OTA at the visible pole.
        // North: dec = +90° (NCP). South: dec = -90° (SCP). The
        // hemisphere case-split sits inside the helper.
        assert_eq!(HomePose::ApPark3.codebase_dec_encoder_degrees(32.7), 90.0);
        assert_eq!(HomePose::ApPark3.codebase_dec_encoder_degrees(-33.0), -90.0);
        // Boundary: lat = 0 falls into the "north" arm by the `>= 0`
        // convention `side_of_pier` uses.
        assert_eq!(HomePose::ApPark3.codebase_dec_encoder_degrees(0.0), 90.0);
        assert_eq!(HomePose::ApPark3.codebase_mech_ha_hours(), 0.0);
    }

    #[test]
    fn home_pose_ap_park_4_post_flip_dec_inverts_with_hemisphere() {
        // AP table: North Dec_celestial = (−90 + Lat), South =
        // (90 + Lat). The post-flip Dec encoder is at
        // sign(dec_celestial) · (180 − |dec_celestial|).
        // North: lat 32.7° → celestial = −57.3°, encoder = −122.7°.
        // South: lat −33° → celestial = +57°, encoder = +123°.
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
        // Park 4 sits at the encoder wrap (anti-meridian post-flip).
        assert_eq!(HomePose::ApPark4.codebase_mech_ha_hours(), -12.0);
    }

    #[test]
    fn home_pose_ap_park_5_post_flip_dec_matches_hemisphere() {
        // AP table: North Dec_celestial = (90 − Lat), South =
        // (−90 − Lat). Post-flip encoder = ±(90 + |lat|),
        // sign matching the hemisphere.
        // North: lat 32.7° → encoder = +122.7°.
        // South: lat −33° → encoder = −123°.
        assert!(
            (HomePose::ApPark5.codebase_dec_encoder_degrees(32.7) - 122.7).abs() < 1e-9,
            "Park 5 N at 32.7°: got {}",
            HomePose::ApPark5.codebase_dec_encoder_degrees(32.7)
        );
        assert!(
            (HomePose::ApPark5.codebase_dec_encoder_degrees(-33.0) - (-123.0)).abs() < 1e-9,
            "Park 5 S at −33°: got {}",
            HomePose::ApPark5.codebase_dec_encoder_degrees(-33.0)
        );
        // Park 5 target is on the anti-meridian; post-flip mech_HA
        // folds back to 0.
        assert_eq!(HomePose::ApPark5.codebase_mech_ha_hours(), 0.0);
    }

    #[test]
    fn home_pose_park4_and_park5_dec_encoders_are_mirror_images_about_zero() {
        // Park 4 and Park 5 differ only in which horizon the OTA faces;
        // the post-flip Dec encoder magnitudes match, with opposite
        // signs (Park 4's sign is anti-hemisphere, Park 5's is
        // pro-hemisphere).
        for lat in [-45.0, -33.0, 32.7, 45.0] {
            let p4 = HomePose::ApPark4.codebase_dec_encoder_degrees(lat);
            let p5 = HomePose::ApPark5.codebase_dec_encoder_degrees(lat);
            assert!((p4 + p5).abs() < 1e-9, "lat {lat}: p4 {p4}, p5 {p5}");
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
