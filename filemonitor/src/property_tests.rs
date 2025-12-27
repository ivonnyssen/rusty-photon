#[cfg(test)]
mod tests {
    use crate::*;
    use proptest::prelude::*;
    use std::path::PathBuf;

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
                default_safe: false,
                case_sensitive: true,
            },
            server: ServerConfig {
                port: 8080,
                device_number: 0,
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
                    default_safe: false,
                    case_sensitive: true,
                },
                server: ServerConfig {
                    port: 8080,
                    device_number: 0,
                },
            };

            let device = FileMonitorDevice::new(config);

            // Should not panic on any input
            let _result = device.evaluate_safety(&content);
        }
    }
}
