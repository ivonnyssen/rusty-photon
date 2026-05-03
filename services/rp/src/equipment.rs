use std::sync::Arc;
use std::time::Duration;

use ascom_alpaca::api::{Camera, CoverCalibrator, FilterWheel, Focuser, Telescope, TypedDevice};
use ascom_alpaca::Client;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use rp_auth::config::ClientAuthConfig;
use serde::Serialize;
use tracing::debug;

use crate::config;
use crate::error::RpError;

pub struct CameraEntry {
    pub id: String,
    pub connected: bool,
    pub config: config::CameraConfig,
    pub device: Option<Arc<dyn Camera>>,
}

pub struct FilterWheelEntry {
    pub id: String,
    pub connected: bool,
    pub config: config::FilterWheelConfig,
    pub device: Option<Arc<dyn FilterWheel>>,
}

pub struct CoverCalibratorEntry {
    pub id: String,
    pub connected: bool,
    pub config: config::CoverCalibratorConfig,
    pub device: Option<Arc<dyn CoverCalibrator>>,
}

pub struct FocuserEntry {
    pub id: String,
    pub connected: bool,
    pub config: config::FocuserConfig,
    pub device: Option<Arc<dyn Focuser>>,
}

/// Singular mount entry. Piggyback rigs share one mount across multiple
/// optical trains, so `EquipmentRegistry.mount` is an `Option`, not a
/// `Vec`. No `id` field — there is nothing to disambiguate.
pub struct MountEntry {
    pub connected: bool,
    pub config: config::MountConfig,
    pub device: Option<Arc<dyn Telescope>>,
}

pub struct EquipmentRegistry {
    pub cameras: Vec<CameraEntry>,
    pub filter_wheels: Vec<FilterWheelEntry>,
    pub cover_calibrators: Vec<CoverCalibratorEntry>,
    pub focusers: Vec<FocuserEntry>,
    pub mount: Option<MountEntry>,
}

#[derive(Serialize)]
pub struct EquipmentStatus {
    pub cameras: Vec<DeviceStatus>,
    pub filter_wheels: Vec<DeviceStatus>,
    pub cover_calibrators: Vec<DeviceStatus>,
    pub focusers: Vec<DeviceStatus>,
    pub mount: Option<MountStatus>,
}

#[derive(Serialize)]
pub struct DeviceStatus {
    pub id: String,
    pub connected: bool,
}

/// Singular wire-format counterpart to [`MountEntry`] — no `id`.
#[derive(Serialize)]
pub struct MountStatus {
    pub connected: bool,
}

impl EquipmentRegistry {
    pub async fn new(equipment_config: &config::EquipmentConfig) -> Self {
        let mut cameras = Vec::new();
        let mut filter_wheels = Vec::new();
        let mut cover_calibrators = Vec::new();
        let mut focusers = Vec::new();

        for cam_config in &equipment_config.cameras {
            let entry = connect_camera(cam_config).await;
            cameras.push(entry);
        }

        for fw_config in &equipment_config.filter_wheels {
            let entry = connect_filter_wheel(fw_config).await;
            filter_wheels.push(entry);
        }

        for cc_config in &equipment_config.cover_calibrators {
            let entry = connect_cover_calibrator(cc_config).await;
            cover_calibrators.push(entry);
        }

        for foc_config in &equipment_config.focusers {
            let entry = connect_focuser(foc_config).await;
            focusers.push(entry);
        }

        let mount = match &equipment_config.mount {
            Some(mount_config) => Some(connect_mount(mount_config).await),
            None => None,
        };

        Self {
            cameras,
            filter_wheels,
            cover_calibrators,
            focusers,
            mount,
        }
    }

    pub fn status(&self) -> EquipmentStatus {
        EquipmentStatus {
            cameras: self
                .cameras
                .iter()
                .map(|c| DeviceStatus {
                    id: c.id.clone(),
                    connected: c.connected,
                })
                .collect(),
            filter_wheels: self
                .filter_wheels
                .iter()
                .map(|fw| DeviceStatus {
                    id: fw.id.clone(),
                    connected: fw.connected,
                })
                .collect(),
            cover_calibrators: self
                .cover_calibrators
                .iter()
                .map(|cc| DeviceStatus {
                    id: cc.id.clone(),
                    connected: cc.connected,
                })
                .collect(),
            focusers: self
                .focusers
                .iter()
                .map(|f| DeviceStatus {
                    id: f.id.clone(),
                    connected: f.connected,
                })
                .collect(),
            mount: self.mount.as_ref().map(|m| MountStatus {
                connected: m.connected,
            }),
        }
    }

    pub fn find_camera(&self, id: &str) -> Option<&CameraEntry> {
        self.cameras.iter().find(|c| c.id == id)
    }

    pub fn find_filter_wheel(&self, id: &str) -> Option<&FilterWheelEntry> {
        self.filter_wheels.iter().find(|fw| fw.id == id)
    }

    pub fn find_cover_calibrator(&self, id: &str) -> Option<&CoverCalibratorEntry> {
        self.cover_calibrators.iter().find(|cc| cc.id == id)
    }

    pub fn find_focuser(&self, id: &str) -> Option<&FocuserEntry> {
        self.focusers.iter().find(|f| f.id == id)
    }

    /// Returns the singular mount entry, or `None` when no mount is
    /// configured. Singular: there is no `id` parameter.
    pub fn find_mount(&self) -> Option<&MountEntry> {
        self.mount.as_ref()
    }

    /// Validate the configured site against the mount's reported
    /// `SiteLatitude`/`SiteLongitude`. Returns:
    ///
    /// - `Ok(())` when no site is configured, no mount is connected,
    ///   the mount lacks the property (any read error → debug-log
    ///   skip), or the values agree to within `SITE_MATCH_TOLERANCE_DEG`.
    /// - `Err(RpError::SiteMismatch)` when both sides expose values
    ///   and they disagree past the tolerance.
    ///
    /// ASCOM does **not** expose a `CanGetSiteLatitude` capability
    /// bit — the read attempt itself is the capability probe, and
    /// `NOT_IMPLEMENTED` (or any other ASCOM error) is treated as
    /// "skip validation" rather than "fail loud".
    pub async fn validate_site(
        &self,
        site: Option<&config::SiteConfig>,
    ) -> crate::error::Result<()> {
        let Some(site) = site else {
            debug!("no site configured; skipping mount-side site validation");
            return Ok(());
        };
        let Some(mount) = self.mount.as_ref() else {
            debug!("no mount configured; skipping mount-side site validation");
            return Ok(());
        };
        if !mount.connected {
            debug!("mount not connected; skipping mount-side site validation");
            return Ok(());
        }
        let Some(t) = mount.device.as_ref() else {
            debug!("mount entry has no device handle; skipping site validation");
            return Ok(());
        };

        let mount_lat = match t.site_latitude().await {
            Ok(v) => v,
            Err(e) => {
                debug!(
                    error = %e,
                    "mount did not report SiteLatitude; skipping mount-side site validation"
                );
                return Ok(());
            }
        };
        let mount_lon = match t.site_longitude().await {
            Ok(v) => v,
            Err(e) => {
                debug!(
                    error = %e,
                    "mount did not report SiteLongitude; skipping mount-side site validation"
                );
                return Ok(());
            }
        };

        let lat_diff = (mount_lat - site.latitude_degrees).abs();
        let lon_diff = (mount_lon - site.longitude_degrees).abs();
        if lat_diff > SITE_MATCH_TOLERANCE_DEG || lon_diff > SITE_MATCH_TOLERANCE_DEG {
            return Err(RpError::SiteMismatch {
                config_lat: site.latitude_degrees,
                config_lon: site.longitude_degrees,
                mount_lat,
                mount_lon,
            });
        }
        debug!(
            site_lat = site.latitude_degrees,
            site_lon = site.longitude_degrees,
            "mount-side site validation: configured site agrees with mount"
        );
        Ok(())
    }
}

/// Hard tolerance for the lat/lon mismatch check. 0.01° ≈ 1 km on the
/// ground — finer than any operator would set deliberately, well above
/// numerical noise on either side. Not configurable.
pub const SITE_MATCH_TOLERANCE_DEG: f64 = 0.01;

/// Build an Alpaca client with optional HTTP Basic Auth credentials.
fn build_alpaca_client(
    url: &str,
    auth: Option<&ClientAuthConfig>,
) -> Result<Client, Box<dyn std::error::Error + Send + Sync>> {
    match auth {
        Some(a) => {
            let encoded = BASE64.encode(format!("{}:{}", a.username, a.password));
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                "authorization",
                format!("Basic {encoded}")
                    .parse()
                    .expect("valid header value"),
            );
            let http = reqwest::Client::builder()
                .default_headers(headers)
                .build()?;
            Ok(Client::new_with_client(url, http)?)
        }
        None => Ok(Client::new(url)?),
    }
}

async fn connect_camera(config: &config::CameraConfig) -> CameraEntry {
    debug!(camera_id = %config.id, alpaca_url = %config.alpaca_url, device_number = config.device_number, "connecting to camera");

    let client = match build_alpaca_client(&config.alpaca_url, config.auth.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            debug!(camera_id = %config.id, error = %e, "failed to create Alpaca client for camera");
            return CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let devices = match tokio::time::timeout(Duration::from_secs(5), client.get_devices()).await {
        Ok(Ok(devices)) => devices,
        Ok(Err(e)) => {
            debug!(camera_id = %config.id, error = %e, "failed to get devices from Alpaca server");
            return CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
        Err(_) => {
            debug!(camera_id = %config.id, "timeout connecting to Alpaca server");
            return CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let mut camera_index = 0u32;
    let mut found_camera: Option<Arc<dyn Camera>> = None;

    for device in devices {
        if let TypedDevice::Camera(cam) = device {
            if camera_index == config.device_number {
                found_camera = Some(cam);
                break;
            }
            camera_index += 1;
        }
    }

    let cam = match found_camera {
        Some(c) => c,
        None => {
            debug!(camera_id = %config.id, device_number = config.device_number, "camera not found on Alpaca server");
            return CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    match cam.set_connected(true).await {
        Ok(()) => {
            debug!(camera_id = %config.id, "camera connected successfully");
            CameraEntry {
                id: config.id.clone(),
                connected: true,
                config: config.clone(),
                device: Some(cam),
            }
        }
        Err(e) => {
            debug!(camera_id = %config.id, error = %e, "failed to connect camera");
            CameraEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            }
        }
    }
}

async fn connect_filter_wheel(config: &config::FilterWheelConfig) -> FilterWheelEntry {
    debug!(fw_id = %config.id, alpaca_url = %config.alpaca_url, device_number = config.device_number, "connecting to filter wheel");

    let client = match build_alpaca_client(&config.alpaca_url, config.auth.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            debug!(fw_id = %config.id, error = %e, "failed to create Alpaca client for filter wheel");
            return FilterWheelEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let devices = match tokio::time::timeout(Duration::from_secs(5), client.get_devices()).await {
        Ok(Ok(devices)) => devices,
        Ok(Err(e)) => {
            debug!(fw_id = %config.id, error = %e, "failed to get devices from Alpaca server");
            return FilterWheelEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
        Err(_) => {
            debug!(fw_id = %config.id, "timeout connecting to Alpaca server");
            return FilterWheelEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let mut fw_index = 0u32;
    let mut found_fw: Option<Arc<dyn FilterWheel>> = None;

    for device in devices {
        if let TypedDevice::FilterWheel(fw) = device {
            if fw_index == config.device_number {
                found_fw = Some(fw);
                break;
            }
            fw_index += 1;
        }
    }

    let fw = match found_fw {
        Some(f) => f,
        None => {
            debug!(fw_id = %config.id, device_number = config.device_number, "filter wheel not found on Alpaca server");
            return FilterWheelEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    match fw.set_connected(true).await {
        Ok(()) => {
            debug!(fw_id = %config.id, "filter wheel connected successfully");
            FilterWheelEntry {
                id: config.id.clone(),
                connected: true,
                config: config.clone(),
                device: Some(fw),
            }
        }
        Err(e) => {
            debug!(fw_id = %config.id, error = %e, "failed to connect filter wheel");
            FilterWheelEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            }
        }
    }
}

async fn connect_cover_calibrator(config: &config::CoverCalibratorConfig) -> CoverCalibratorEntry {
    debug!(cc_id = %config.id, alpaca_url = %config.alpaca_url, device_number = config.device_number, "connecting to cover calibrator");

    let client = match build_alpaca_client(&config.alpaca_url, config.auth.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            debug!(cc_id = %config.id, error = %e, "failed to create Alpaca client for cover calibrator");
            return CoverCalibratorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let devices = match tokio::time::timeout(Duration::from_secs(5), client.get_devices()).await {
        Ok(Ok(devices)) => devices,
        Ok(Err(e)) => {
            debug!(cc_id = %config.id, error = %e, "failed to get devices from Alpaca server");
            return CoverCalibratorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
        Err(_) => {
            debug!(cc_id = %config.id, "timeout connecting to Alpaca server");
            return CoverCalibratorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let mut cc_index = 0u32;
    let mut found_cc: Option<Arc<dyn CoverCalibrator>> = None;

    for device in devices {
        if let TypedDevice::CoverCalibrator(cc) = device {
            if cc_index == config.device_number {
                found_cc = Some(cc);
                break;
            }
            cc_index += 1;
        }
    }

    let cc = match found_cc {
        Some(c) => c,
        None => {
            debug!(cc_id = %config.id, device_number = config.device_number, "cover calibrator not found on Alpaca server");
            return CoverCalibratorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    match cc.set_connected(true).await {
        Ok(()) => {
            debug!(cc_id = %config.id, "cover calibrator connected successfully");
            CoverCalibratorEntry {
                id: config.id.clone(),
                connected: true,
                config: config.clone(),
                device: Some(cc),
            }
        }
        Err(e) => {
            debug!(cc_id = %config.id, error = %e, "failed to connect cover calibrator");
            CoverCalibratorEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            }
        }
    }
}

async fn connect_focuser(config: &config::FocuserConfig) -> FocuserEntry {
    debug!(focuser_id = %config.id, alpaca_url = %config.alpaca_url, device_number = config.device_number, "connecting to focuser");

    let client = match build_alpaca_client(&config.alpaca_url, config.auth.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            debug!(focuser_id = %config.id, error = %e, "failed to create Alpaca client for focuser");
            return FocuserEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let devices = match tokio::time::timeout(Duration::from_secs(5), client.get_devices()).await {
        Ok(Ok(devices)) => devices,
        Ok(Err(e)) => {
            debug!(focuser_id = %config.id, error = %e, "failed to get devices from Alpaca server");
            return FocuserEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
        Err(_) => {
            debug!(focuser_id = %config.id, "timeout connecting to Alpaca server");
            return FocuserEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let mut focuser_index = 0u32;
    let mut found_focuser: Option<Arc<dyn Focuser>> = None;

    for device in devices {
        if let TypedDevice::Focuser(foc) = device {
            if focuser_index == config.device_number {
                found_focuser = Some(foc);
                break;
            }
            focuser_index += 1;
        }
    }

    let foc = match found_focuser {
        Some(f) => f,
        None => {
            debug!(focuser_id = %config.id, device_number = config.device_number, "focuser not found on Alpaca server");
            return FocuserEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    match foc.set_connected(true).await {
        Ok(()) => {
            debug!(focuser_id = %config.id, "focuser connected successfully");
            FocuserEntry {
                id: config.id.clone(),
                connected: true,
                config: config.clone(),
                device: Some(foc),
            }
        }
        Err(e) => {
            debug!(focuser_id = %config.id, error = %e, "failed to connect focuser");
            FocuserEntry {
                id: config.id.clone(),
                connected: false,
                config: config.clone(),
                device: None,
            }
        }
    }
}

async fn connect_mount(config: &config::MountConfig) -> MountEntry {
    debug!(alpaca_url = %config.alpaca_url, device_number = config.device_number, "connecting to mount");

    let client = match build_alpaca_client(&config.alpaca_url, config.auth.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            debug!(error = %e, "failed to create Alpaca client for mount");
            return MountEntry {
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let devices = match tokio::time::timeout(Duration::from_secs(5), client.get_devices()).await {
        Ok(Ok(devices)) => devices,
        Ok(Err(e)) => {
            debug!(error = %e, "failed to get devices from Alpaca server");
            return MountEntry {
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
        Err(_) => {
            debug!("timeout connecting to Alpaca server");
            return MountEntry {
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    let mut mount_index = 0u32;
    let mut found_mount: Option<Arc<dyn Telescope>> = None;

    for device in devices {
        if let TypedDevice::Telescope(t) = device {
            if mount_index == config.device_number {
                found_mount = Some(t);
                break;
            }
            mount_index += 1;
        }
    }

    let t = match found_mount {
        Some(t) => t,
        None => {
            debug!(
                device_number = config.device_number,
                "mount not found on Alpaca server"
            );
            return MountEntry {
                connected: false,
                config: config.clone(),
                device: None,
            };
        }
    };

    match t.set_connected(true).await {
        Ok(()) => {
            debug!("mount connected successfully");
            MountEntry {
                connected: true,
                config: config.clone(),
                device: Some(t),
            }
        }
        Err(e) => {
            debug!(error = %e, "failed to connect mount");
            MountEntry {
                connected: false,
                config: config.clone(),
                device: None,
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn build_alpaca_client_without_auth() {
        build_alpaca_client("http://localhost:11111", None).unwrap();
    }

    #[test]
    fn build_alpaca_client_with_auth() {
        let auth = ClientAuthConfig {
            username: "observatory".to_string(),
            password: "secret".to_string(),
        };
        build_alpaca_client("http://localhost:11111", Some(&auth)).unwrap();
    }

    #[test]
    fn build_alpaca_client_with_invalid_url_fails() {
        let result = build_alpaca_client("not-a-url", None);
        assert!(result.is_err());
    }

    /// `connect_focuser` swallows every failure mode into a disconnected
    /// `FocuserEntry`; the registry never refuses to start because a focuser
    /// went missing. The two unit tests below pin two of the failure paths
    /// directly, complementing the single failure path exercised by the
    /// "rp is running with a focuser at \"http://localhost:1\" device 0"
    /// BDD scenario.
    #[tokio::test]
    async fn connect_focuser_invalid_url_returns_disconnected_entry() {
        let cfg = config::FocuserConfig {
            id: "main-focuser".to_string(),
            camera_id: String::new(),
            alpaca_url: "not-a-url".to_string(),
            device_number: 0,
            min_position: None,
            max_position: None,
            auth: None,
        };
        let entry = connect_focuser(&cfg).await;
        assert_eq!(entry.id, "main-focuser");
        assert!(!entry.connected);
        assert!(entry.device.is_none());
    }

    #[tokio::test]
    async fn connect_focuser_unreachable_returns_disconnected_entry() {
        // 127.0.0.1:1 is reserved and refuses connections — `get_devices`
        // returns an error inside the 5s timeout window, exercising the
        // `Ok(Err(e))` arm of `connect_focuser`'s match.
        let cfg = config::FocuserConfig {
            id: "main-focuser".to_string(),
            camera_id: String::new(),
            alpaca_url: "http://127.0.0.1:1".to_string(),
            device_number: 0,
            min_position: None,
            max_position: None,
            auth: None,
        };
        let entry = connect_focuser(&cfg).await;
        assert_eq!(entry.id, "main-focuser");
        assert!(!entry.connected);
        assert!(entry.device.is_none());
    }

    // -----------------------------------------------------------------------
    // Alpaca stub server — covers the three remaining failure branches in
    // `connect_focuser` (timeout, no-focuser-in-list, set_connected error).
    //
    // We spawn an axum router on `127.0.0.1:0` and shape the responses to
    // hit each branch deterministically. The wire format is the standard
    // ASCOM Alpaca shape (PascalCase keys, `Value` envelope around device
    // arrays, `ErrorNumber`/`ErrorMessage` for action failures).
    //
    // A workspace-wide testing-strategy decision lives in issue #111;
    // stubs in this module are the agreed interim approach for this PR.
    // -----------------------------------------------------------------------

    use axum::{
        routing::{get, put},
        Json, Router,
    };
    use std::net::SocketAddr;

    struct AlpacaStub {
        addr: SocketAddr,
        shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
        handle: Option<tokio::task::JoinHandle<()>>,
    }

    impl AlpacaStub {
        fn url(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    impl Drop for AlpacaStub {
        fn drop(&mut self) {
            if let Some(tx) = self.shutdown_tx.take() {
                let _ = tx.send(());
            }
            if let Some(h) = self.handle.take() {
                h.abort();
            }
        }
    }

    async fn spawn_stub(router: Router) -> AlpacaStub {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        AlpacaStub {
            addr,
            shutdown_tx: Some(tx),
            handle: Some(handle),
        }
    }

    fn focuser_config_for(url: &str) -> config::FocuserConfig {
        config::FocuserConfig {
            id: "main-focuser".to_string(),
            camera_id: String::new(),
            alpaca_url: url.to_string(),
            device_number: 0,
            min_position: None,
            max_position: None,
            auth: None,
        }
    }

    /// `Value: []` — `get_devices` succeeds but yields no Focuser, so the
    /// device-search loop falls into the `None` arm and `connect_focuser`
    /// returns a disconnected entry without ever calling `set_connected`.
    ///
    /// Transaction IDs are deliberately omitted from the response. The
    /// upstream client deserializes them as `Option<NonZeroU32>`; emitting
    /// the literal `0` would fail `NonZeroU32` parsing and short-circuit
    /// the request as a network error rather than letting the device-list
    /// branch fire.
    #[tokio::test]
    async fn connect_focuser_no_focuser_in_devices_returns_disconnected_entry() {
        let app = Router::new().route(
            "/management/v1/configureddevices",
            get(|| async {
                Json(serde_json::json!({
                    "Value": [],
                    "ErrorNumber": 0,
                    "ErrorMessage": ""
                }))
            }),
        );
        let stub = spawn_stub(app).await;
        let entry = connect_focuser(&focuser_config_for(&stub.url())).await;
        assert!(!entry.connected);
        assert!(entry.device.is_none());
    }

    /// Server returns a Focuser at device 0, but the `set_connected` PUT
    /// responds with `ErrorNumber != 0`, exercising the final match arm in
    /// `connect_focuser` (the Alpaca-level rejection of `Connected=true`).
    #[tokio::test]
    async fn connect_focuser_set_connected_fails_returns_disconnected_entry() {
        let app = Router::new()
            .route(
                "/management/v1/configureddevices",
                get(|| async {
                    Json(serde_json::json!({
                        "Value": [{
                            "DeviceName": "Focuser 0",
                            "DeviceType": "Focuser",
                            "DeviceNumber": 0,
                            "UniqueID": "test-focuser-uid"
                        }],
                        "ErrorNumber": 0,
                        "ErrorMessage": ""
                    }))
                }),
            )
            .route(
                "/api/v1/focuser/0/connected",
                put(|| async {
                    // Alpaca convention: action-level failure is signalled
                    // by a non-zero ErrorNumber + ErrorMessage in the body,
                    // not by an HTTP status code. ASCOMErrorCode is in
                    // 0x400..=0xFFF, so 1024 (0x400) is the smallest valid
                    // non-OK value.
                    Json(serde_json::json!({
                        "ErrorNumber": 1024,
                        "ErrorMessage": "simulated set_connected failure"
                    }))
                }),
            );
        let stub = spawn_stub(app).await;
        let entry = connect_focuser(&focuser_config_for(&stub.url())).await;
        assert!(!entry.connected);
        assert!(entry.device.is_none());
    }

    /// Handler hangs forever; the 5 s `tokio::time::timeout` wrapping
    /// `client.get_devices()` fires and `connect_focuser` falls into the
    /// `Err(_)` (timeout) arm. `start_paused = true` lets tokio
    /// auto-advance virtual time once every other future is pending, so
    /// the test completes in real-time milliseconds rather than waiting 5
    /// wallclock seconds.
    #[tokio::test(start_paused = true)]
    async fn connect_focuser_timeout_returns_disconnected_entry() {
        let app = Router::new().route(
            "/management/v1/configureddevices",
            get(|| async { std::future::pending::<Json<serde_json::Value>>().await }),
        );
        let stub = spawn_stub(app).await;
        let entry = connect_focuser(&focuser_config_for(&stub.url())).await;
        assert!(!entry.connected);
        assert!(entry.device.is_none());
    }

    /// Build a router that successfully advertises one focuser at index 0
    /// and accepts `set_connected(true)`. Shared by the success-path
    /// `connect_focuser` test and the `EquipmentRegistry` end-to-end test.
    fn ok_focuser_router() -> Router {
        Router::new()
            .route(
                "/management/v1/configureddevices",
                get(|| async {
                    Json(serde_json::json!({
                        "Value": [{
                            "DeviceName": "Focuser 0",
                            "DeviceType": "Focuser",
                            "DeviceNumber": 0,
                            "UniqueID": "test-focuser-uid"
                        }],
                        "ErrorNumber": 0,
                        "ErrorMessage": ""
                    }))
                }),
            )
            .route(
                "/api/v1/focuser/0/connected",
                put(|| async {
                    Json(serde_json::json!({
                        "ErrorNumber": 0,
                        "ErrorMessage": ""
                    }))
                }),
            )
    }

    /// Server advertises a focuser at index 0 and accepts `set_connected`,
    /// exercising the `Ok(())` arm of `connect_focuser` plus the device
    /// iteration `Some(_)` match arm — the success path that doesn't run
    /// in any of the failure-branch tests above.
    #[tokio::test]
    async fn connect_focuser_success_returns_connected_entry() {
        let stub = spawn_stub(ok_focuser_router()).await;
        let entry = connect_focuser(&focuser_config_for(&stub.url())).await;
        assert!(entry.connected, "expected entry to be connected");
        assert!(entry.device.is_some(), "expected entry to hold a device");
        assert_eq!(entry.id, "main-focuser");
    }

    /// `EquipmentRegistry::new` with a focuser entry, plus `status()` and
    /// `find_focuser`. Pins the `EquipmentStatus.focusers` collection and
    /// the lookup helper that aren't exercised by `connect_focuser` tests
    /// in isolation.
    #[tokio::test]
    async fn equipment_registry_surfaces_connected_focuser_in_status_and_lookup() {
        let stub = spawn_stub(ok_focuser_router()).await;
        let equipment_cfg = config::EquipmentConfig {
            cameras: vec![],
            mount: None,
            focusers: vec![focuser_config_for(&stub.url())],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            safety_monitors: vec![],
        };
        let registry = EquipmentRegistry::new(&equipment_cfg).await;
        assert_eq!(registry.focusers.len(), 1);

        let found = registry
            .find_focuser("main-focuser")
            .expect("find_focuser should return the configured focuser");
        assert!(found.connected);
        assert!(registry.find_focuser("nonexistent").is_none());

        let status = registry.status();
        assert_eq!(status.focusers.len(), 1);
        assert_eq!(status.focusers[0].id, "main-focuser");
        assert!(status.focusers[0].connected);
        assert!(
            status.mount.is_none(),
            "mount should be None when unconfigured"
        );
    }

    // -----------------------------------------------------------------------
    // Mount tests — same pattern as focuser tests above. Five failure arms
    // + one success arm + one EquipmentRegistry end-to-end test.
    // -----------------------------------------------------------------------

    fn mount_config_for(url: &str) -> config::MountConfig {
        config::MountConfig {
            alpaca_url: url.to_string(),
            device_number: 0,
            settle_after_slew: None,
            auth: None,
        }
    }

    #[tokio::test]
    async fn connect_mount_invalid_url_returns_disconnected_entry() {
        let cfg = mount_config_for("not-a-url");
        let entry = connect_mount(&cfg).await;
        assert!(!entry.connected);
        assert!(entry.device.is_none());
    }

    #[tokio::test]
    async fn connect_mount_unreachable_returns_disconnected_entry() {
        let cfg = mount_config_for("http://127.0.0.1:1");
        let entry = connect_mount(&cfg).await;
        assert!(!entry.connected);
        assert!(entry.device.is_none());
    }

    #[tokio::test]
    async fn connect_mount_no_telescope_in_devices_returns_disconnected_entry() {
        let app = Router::new().route(
            "/management/v1/configureddevices",
            get(|| async {
                Json(serde_json::json!({
                    "Value": [],
                    "ErrorNumber": 0,
                    "ErrorMessage": ""
                }))
            }),
        );
        let stub = spawn_stub(app).await;
        let entry = connect_mount(&mount_config_for(&stub.url())).await;
        assert!(!entry.connected);
        assert!(entry.device.is_none());
    }

    #[tokio::test]
    async fn connect_mount_set_connected_fails_returns_disconnected_entry() {
        let app = Router::new()
            .route(
                "/management/v1/configureddevices",
                get(|| async {
                    Json(serde_json::json!({
                        "Value": [{
                            "DeviceName": "Telescope 0",
                            "DeviceType": "Telescope",
                            "DeviceNumber": 0,
                            "UniqueID": "test-mount-uid"
                        }],
                        "ErrorNumber": 0,
                        "ErrorMessage": ""
                    }))
                }),
            )
            .route(
                "/api/v1/telescope/0/connected",
                put(|| async {
                    Json(serde_json::json!({
                        "ErrorNumber": 1024,
                        "ErrorMessage": "simulated set_connected failure"
                    }))
                }),
            );
        let stub = spawn_stub(app).await;
        let entry = connect_mount(&mount_config_for(&stub.url())).await;
        assert!(!entry.connected);
        assert!(entry.device.is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn connect_mount_timeout_returns_disconnected_entry() {
        let app = Router::new().route(
            "/management/v1/configureddevices",
            get(|| async { std::future::pending::<Json<serde_json::Value>>().await }),
        );
        let stub = spawn_stub(app).await;
        let entry = connect_mount(&mount_config_for(&stub.url())).await;
        assert!(!entry.connected);
        assert!(entry.device.is_none());
    }

    fn ok_mount_router() -> Router {
        Router::new()
            .route(
                "/management/v1/configureddevices",
                get(|| async {
                    Json(serde_json::json!({
                        "Value": [{
                            "DeviceName": "Telescope 0",
                            "DeviceType": "Telescope",
                            "DeviceNumber": 0,
                            "UniqueID": "test-mount-uid"
                        }],
                        "ErrorNumber": 0,
                        "ErrorMessage": ""
                    }))
                }),
            )
            .route(
                "/api/v1/telescope/0/connected",
                put(|| async {
                    Json(serde_json::json!({
                        "ErrorNumber": 0,
                        "ErrorMessage": ""
                    }))
                }),
            )
    }

    #[tokio::test]
    async fn connect_mount_success_returns_connected_entry() {
        let stub = spawn_stub(ok_mount_router()).await;
        let entry = connect_mount(&mount_config_for(&stub.url())).await;
        assert!(entry.connected, "expected entry to be connected");
        assert!(entry.device.is_some(), "expected entry to hold a device");
    }

    #[tokio::test]
    async fn equipment_registry_surfaces_connected_mount_in_status_and_lookup() {
        let stub = spawn_stub(ok_mount_router()).await;
        let equipment_cfg = config::EquipmentConfig {
            cameras: vec![],
            mount: Some(mount_config_for(&stub.url())),
            focusers: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            safety_monitors: vec![],
        };
        let registry = EquipmentRegistry::new(&equipment_cfg).await;

        let found = registry
            .find_mount()
            .expect("find_mount should return the configured mount");
        assert!(found.connected);

        let status = registry.status();
        let mount_status = status
            .mount
            .as_ref()
            .expect("EquipmentStatus.mount should be Some when configured");
        assert!(mount_status.connected);
    }

    /// Build an `ok_mount_router` extended with `SiteLatitude` /
    /// `SiteLongitude` Get handlers that return the supplied values.
    fn mount_router_with_site(lat: f64, lon: f64) -> Router {
        ok_mount_router()
            .route(
                "/api/v1/telescope/0/sitelatitude",
                get(move || async move {
                    Json(serde_json::json!({
                        "Value": lat,
                        "ErrorNumber": 0,
                        "ErrorMessage": ""
                    }))
                }),
            )
            .route(
                "/api/v1/telescope/0/sitelongitude",
                get(move || async move {
                    Json(serde_json::json!({
                        "Value": lon,
                        "ErrorNumber": 0,
                        "ErrorMessage": ""
                    }))
                }),
            )
    }

    /// Build an `ok_mount_router` whose `SiteLatitude` / `SiteLongitude`
    /// endpoints respond with `NOT_IMPLEMENTED` (ASCOM error 0x400),
    /// modelling a mount that lacks the property. The validate_site
    /// path treats this as "skip validation" rather than "fail loud".
    fn mount_router_without_site() -> Router {
        ok_mount_router()
            .route(
                "/api/v1/telescope/0/sitelatitude",
                get(|| async {
                    Json(serde_json::json!({
                        "ErrorNumber": 0x400,
                        "ErrorMessage": "Property SiteLatitude is not implemented"
                    }))
                }),
            )
            .route(
                "/api/v1/telescope/0/sitelongitude",
                get(|| async {
                    Json(serde_json::json!({
                        "ErrorNumber": 0x400,
                        "ErrorMessage": "Property SiteLongitude is not implemented"
                    }))
                }),
            )
    }

    async fn registry_with_mount(stub_url: &str) -> EquipmentRegistry {
        let equipment_cfg = config::EquipmentConfig {
            cameras: vec![],
            mount: Some(mount_config_for(stub_url)),
            focusers: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            safety_monitors: vec![],
        };
        EquipmentRegistry::new(&equipment_cfg).await
    }

    #[tokio::test]
    async fn validate_site_no_op_when_site_absent() {
        let stub = spawn_stub(ok_mount_router()).await;
        let registry = registry_with_mount(&stub.url()).await;
        registry
            .validate_site(None)
            .await
            .expect("missing site config must short-circuit cleanly");
    }

    #[tokio::test]
    async fn validate_site_no_op_when_mount_absent() {
        let registry = EquipmentRegistry::new(&config::EquipmentConfig {
            cameras: vec![],
            mount: None,
            focusers: vec![],
            filter_wheels: vec![],
            cover_calibrators: vec![],
            safety_monitors: vec![],
        })
        .await;
        let site = config::SiteConfig {
            latitude_degrees: 47.6062,
            longitude_degrees: -122.3321,
        };
        registry
            .validate_site(Some(&site))
            .await
            .expect("no mount → no validation, no error");
    }

    #[tokio::test]
    async fn validate_site_skips_when_mount_lacks_property() {
        let stub = spawn_stub(mount_router_without_site()).await;
        let registry = registry_with_mount(&stub.url()).await;
        let site = config::SiteConfig {
            latitude_degrees: 47.6062,
            longitude_degrees: -122.3321,
        };
        // No `CanGetSiteLatitude`/`Longitude` in ASCOM — the read
        // attempt itself is the capability probe. NOT_IMPLEMENTED
        // (or any other error) should be debug-logged and skipped.
        registry
            .validate_site(Some(&site))
            .await
            .expect("missing property must skip, not error");
    }

    #[tokio::test]
    async fn validate_site_passes_when_mount_agrees() {
        let stub = spawn_stub(mount_router_with_site(47.6062, -122.3321)).await;
        let registry = registry_with_mount(&stub.url()).await;
        let site = config::SiteConfig {
            latitude_degrees: 47.6062,
            longitude_degrees: -122.3321,
        };
        registry.validate_site(Some(&site)).await.unwrap();
    }

    #[tokio::test]
    async fn validate_site_passes_when_within_tolerance() {
        // Diff = 0.005° in each dim, below the 0.01° hard cap.
        let stub = spawn_stub(mount_router_with_site(47.611, -122.337)).await;
        let registry = registry_with_mount(&stub.url()).await;
        let site = config::SiteConfig {
            latitude_degrees: 47.606,
            longitude_degrees: -122.332,
        };
        registry.validate_site(Some(&site)).await.unwrap();
    }

    #[tokio::test]
    async fn validate_site_errors_on_latitude_mismatch() {
        // Mount reports lat off by 1°, well past the 0.01° cap.
        let stub = spawn_stub(mount_router_with_site(48.6062, -122.3321)).await;
        let registry = registry_with_mount(&stub.url()).await;
        let site = config::SiteConfig {
            latitude_degrees: 47.6062,
            longitude_degrees: -122.3321,
        };
        let err = registry.validate_site(Some(&site)).await.unwrap_err();
        match err {
            RpError::SiteMismatch {
                config_lat,
                config_lon,
                mount_lat,
                mount_lon,
            } => {
                assert!((config_lat - 47.6062).abs() < 1e-9);
                assert!((config_lon - -122.3321).abs() < 1e-9);
                assert!((mount_lat - 48.6062).abs() < 1e-9);
                assert!((mount_lon - -122.3321).abs() < 1e-9);
            }
            other => panic!("expected SiteMismatch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validate_site_errors_on_longitude_mismatch() {
        let stub = spawn_stub(mount_router_with_site(47.6062, -120.0)).await;
        let registry = registry_with_mount(&stub.url()).await;
        let site = config::SiteConfig {
            latitude_degrees: 47.6062,
            longitude_degrees: -122.3321,
        };
        let err = registry.validate_site(Some(&site)).await.unwrap_err();
        assert!(
            matches!(err, RpError::SiteMismatch { .. }),
            "expected SiteMismatch, got {err}"
        );
        // The Display impl must name both pairs so an operator who
        // sees this in a startup log knows what the disagreement is.
        let s = err.to_string();
        assert!(s.contains("47.6062"), "missing config lat: {s}");
        assert!(s.contains("-122.3321"), "missing config lon: {s}");
        assert!(s.contains("-120.0000"), "missing mount lon: {s}");
    }
}
