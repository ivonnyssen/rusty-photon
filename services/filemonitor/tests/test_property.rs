use filemonitor::{
    Config, DeviceConfig, FileConfig, FileMonitorDevice, ParsingConfig, ParsingRule, RuleType,
    ServerConfig,
};
use proptest::prelude::*;
use std::path::PathBuf;

fn create_case_insensitive_config() -> Config {
    Config {
        device: DeviceConfig {
            name: "Test Device".to_string(),
            unique_id: "test-123".to_string(),
            description: "Test Description".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("/tmp/test.txt"),
            polling_interval_seconds: 1,
        },
        parsing: ParsingConfig {
            rules: vec![
                ParsingRule {
                    rule_type: RuleType::Contains,
                    pattern: "SAFE".to_string(),
                    safe: true,
                },
                ParsingRule {
                    rule_type: RuleType::Contains,
                    pattern: "DANGER".to_string(),
                    safe: false,
                },
            ],
            case_sensitive: false,
        },
        server: ServerConfig {
            port: 8080,
            device_number: 0,
            discovery_port: None,
        },
    }
}

fn create_test_config() -> Config {
    Config {
        device: DeviceConfig {
            name: "Test Device".to_string(),
            unique_id: "test-123".to_string(),
            description: "Test Description".to_string(),
        },
        file: FileConfig {
            path: PathBuf::from("/tmp/test.txt"),
            polling_interval_seconds: 1,
        },
        parsing: ParsingConfig {
            rules: vec![
                ParsingRule {
                    rule_type: RuleType::Contains,
                    pattern: "SAFE".to_string(),
                    safe: true,
                },
                ParsingRule {
                    rule_type: RuleType::Contains,
                    pattern: "DANGER".to_string(),
                    safe: false,
                },
            ],
            case_sensitive: true,
        },
        server: ServerConfig {
            port: 8080,
            device_number: 0,
            discovery_port: None,
        },
    }
}

proptest! {
    #[test]
    fn test_safety_evaluation_consistency(content in ".*") {
        let config = create_test_config();
        let device = FileMonitorDevice::new(config);

        // Safety evaluation should be deterministic
        let result1 = device.evaluate_safety(&content);
        let result2 = device.evaluate_safety(&content);
        prop_assert_eq!(result1, result2);
    }

    #[test]
    fn test_safe_content_always_safe(safe_suffix in ".*") {
        let content = format!("SAFE {}", safe_suffix);
        let config = create_test_config();
        let device = FileMonitorDevice::new(config);

        prop_assert!(device.evaluate_safety(&content));
    }

    #[test]
    fn test_danger_content_always_unsafe(danger_suffix in ".*") {
        let content = format!("DANGER {}", danger_suffix);
        let config = create_test_config();
        let device = FileMonitorDevice::new(config);

        prop_assert!(!device.evaluate_safety(&content));
    }

    #[test]
    fn test_regex_pattern_consistency(
        pattern in "[a-zA-Z0-9]+",
        content in ".*"
    ) {
        let config = Config {
            device: DeviceConfig {
                name: "Test".to_string(),
                unique_id: "test".to_string(),
                description: "Test".to_string(),
            },
            file: FileConfig {
                path: PathBuf::from("/tmp/test.txt"),
                polling_interval_seconds: 1,
            },
            parsing: ParsingConfig {
                rules: vec![ParsingRule {
                    rule_type: RuleType::Regex,
                    pattern: pattern.clone(),
                    safe: true,
                }],
                case_sensitive: true,
            },
            server: ServerConfig {
                port: 8080,
                device_number: 0,
                discovery_port: None,
            },
        };

        let device = FileMonitorDevice::new(config);

        // Should not panic on any input
        let _result = device.evaluate_safety(&content);
    }

    #[test]
    fn test_case_insensitive_contains_matches_any_case(
        prefix in "[a-zA-Z ]{0,10}",
        suffix in "[a-zA-Z ]{0,10}"
    ) {
        let config = create_case_insensitive_config();
        let device = FileMonitorDevice::new(config);

        // "safe" in any case should match
        let content = format!("{}safe{}", prefix, suffix);
        prop_assert!(device.evaluate_safety(&content));

        let content_upper = format!("{}SAFE{}", prefix, suffix);
        prop_assert!(device.evaluate_safety(&content_upper));

        let content_mixed = format!("{}SaFe{}", prefix, suffix);
        prop_assert!(device.evaluate_safety(&content_mixed));
    }

    #[test]
    fn test_case_insensitive_regex_consistency(
        pattern in "[a-zA-Z0-9]+",
        content in ".*"
    ) {
        let config = Config {
            device: DeviceConfig {
                name: "Test".to_string(),
                unique_id: "test".to_string(),
                description: "Test".to_string(),
            },
            file: FileConfig {
                path: PathBuf::from("/tmp/test.txt"),
                polling_interval_seconds: 1,
            },
            parsing: ParsingConfig {
                rules: vec![ParsingRule {
                    rule_type: RuleType::Regex,
                    pattern: pattern.clone(),
                    safe: true,
                }],
                case_sensitive: false,
            },
            server: ServerConfig {
                port: 8080,
                device_number: 0,
                discovery_port: None,
            },
        };

        let device = FileMonitorDevice::new(config);

        // Should not panic and should be deterministic
        let result1 = device.evaluate_safety(&content);
        let result2 = device.evaluate_safety(&content);
        prop_assert_eq!(result1, result2);
    }

    #[test]
    fn test_config_round_trip_serialization(
        name in "[a-zA-Z ]{1,20}",
        unique_id in "[a-z0-9-]{1,20}",
        port in 1024u16..65535u16,
        polling in 1u64..3600u64,
    ) {
        let config = Config {
            device: DeviceConfig {
                name,
                unique_id,
                description: "Test Description".to_string(),
            },
            file: FileConfig {
                path: PathBuf::from("/tmp/test.txt"),
                polling_interval_seconds: polling,
            },
            parsing: ParsingConfig {
                rules: vec![ParsingRule {
                    rule_type: RuleType::Contains,
                    pattern: "TEST".to_string(),
                    safe: true,
                }],
                case_sensitive: false,
            },
            server: ServerConfig {
                port,
                device_number: 0,
                discovery_port: None,
            },
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: Config = serde_json::from_str(&json).unwrap();

        prop_assert_eq!(config.device.name, deserialized.device.name);
        prop_assert_eq!(config.device.unique_id, deserialized.device.unique_id);
        prop_assert_eq!(config.server.port, deserialized.server.port);
        prop_assert_eq!(config.file.polling_interval_seconds, deserialized.file.polling_interval_seconds);
        prop_assert_eq!(config.parsing.rules.len(), deserialized.parsing.rules.len());
    }

    #[test]
    fn test_invalid_regex_never_panics(
        pattern in ".*",
        content in ".*"
    ) {
        let config = Config {
            device: DeviceConfig {
                name: "Test".to_string(),
                unique_id: "test".to_string(),
                description: "Test".to_string(),
            },
            file: FileConfig {
                path: PathBuf::from("/tmp/test.txt"),
                polling_interval_seconds: 1,
            },
            parsing: ParsingConfig {
                rules: vec![ParsingRule {
                    rule_type: RuleType::Regex,
                    pattern,
                    safe: true,
                }],
                case_sensitive: false,
            },
            server: ServerConfig {
                port: 8080,
                device_number: 0,
                discovery_port: None,
            },
        };

        let device = FileMonitorDevice::new(config);
        // Should never panic, even with arbitrary regex patterns
        let _result = device.evaluate_safety(&content);
    }
}
