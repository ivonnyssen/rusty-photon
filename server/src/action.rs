enum DeviceTypes {
    Camera,
    CoverCalibrator,
    Dome,
    FilterWheel,
    Focuser,
    ObservingConditions,
    Rotator,
    SafetyMonitor,
    Switch,
    Telescope,
}

trait Action {
    fn preconditions_complete(&self);
    fn execute(&self);
    fn postconditions_complete(&self);
    fn required_devices() -> Vec<DeviceTypes>;
}
