use thiserror::Error;

pub type Result<T> = std::result::Result<T, RpError>;

#[derive(Debug, Error)]
pub enum RpError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("alpaca error: {0}")]
    Alpaca(String),

    #[error("equipment not found: {0}")]
    EquipmentNotFound(String),

    #[error("session error: {0}")]
    Session(String),

    #[error("server error: {0}")]
    Server(String),

    #[error("imaging error: {0}")]
    Imaging(String),

    /// Configured site does not match the mount's reported
    /// `SiteLatitude` / `SiteLongitude`. Threshold is fixed at 0.01°
    /// in either dimension; see `docs/services/rp.md` §"Site
    /// Validation Against the ASCOM Mount" for the rationale.
    #[error(
        "site mismatch: config lat={config_lat:.4}° lon={config_lon:.4}°, \
         mount lat={mount_lat:.4}° lon={mount_lon:.4}° (threshold 0.01°)"
    )]
    SiteMismatch {
        config_lat: f64,
        config_lon: f64,
        mount_lat: f64,
        mount_lon: f64,
    },
}
