use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Target {
    name: String,
    ra: f64,
    dec: f64,
    epoch: f64,
}
