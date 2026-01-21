//! Common types used across the PHD2 guider client

use serde::{Deserialize, Serialize};

/// Rectangle for specifying regions of interest
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    /// Create a new rectangle
    pub fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// PHD2 equipment profile
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub id: i32,
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_creation() {
        let rect = Rect::new(100, 200, 50, 50);
        assert_eq!(rect.x, 100);
        assert_eq!(rect.y, 200);
        assert_eq!(rect.width, 50);
        assert_eq!(rect.height, 50);
    }

    #[test]
    fn test_rect_serialization() {
        let rect = Rect::new(100, 200, 50, 50);
        let json = serde_json::to_value(&rect).unwrap();
        assert_eq!(json["x"], 100);
        assert_eq!(json["y"], 200);
        assert_eq!(json["width"], 50);
        assert_eq!(json["height"], 50);
    }

    #[test]
    fn test_profile_parsing() {
        let json = r#"{"id":1,"name":"Default Equipment"}"#;
        let profile: Profile = serde_json::from_str(json).unwrap();
        assert_eq!(profile.id, 1);
        assert_eq!(profile.name, "Default Equipment");
    }
}
