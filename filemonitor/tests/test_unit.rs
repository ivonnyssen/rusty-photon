use filemonitor::LogLevel;

#[test]
fn test_log_level_conversion() {
    assert_eq!(tracing::Level::from(LogLevel::Error), tracing::Level::ERROR);
    assert_eq!(tracing::Level::from(LogLevel::Warn), tracing::Level::WARN);
    assert_eq!(tracing::Level::from(LogLevel::Info), tracing::Level::INFO);
    assert_eq!(tracing::Level::from(LogLevel::Debug), tracing::Level::DEBUG);
    assert_eq!(tracing::Level::from(LogLevel::Trace), tracing::Level::TRACE);
}

#[test]
fn test_log_level_clone() {
    let level = LogLevel::Info;
    let cloned = level.clone();
    assert_eq!(tracing::Level::from(level), tracing::Level::from(cloned));
}

#[test]
fn test_log_level_debug() {
    let level = LogLevel::Debug;
    let debug_str = format!("{:?}", level);
    assert_eq!(debug_str, "Debug");
}
