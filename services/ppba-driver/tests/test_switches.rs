//! Switch definition tests for PPBA Switch driver

use ppba_driver::{SwitchId, MAX_SWITCH};

#[test]
fn max_switch_is_sixteen() {
    assert_eq!(MAX_SWITCH, 16);
}

#[test]
fn all_switch_ids_are_valid() {
    for id in 0..MAX_SWITCH {
        let switch_id = SwitchId::from_id(id);
        assert!(switch_id.is_some(), "Switch ID {} should be valid", id);
    }
}

#[test]
fn switch_id_beyond_max_is_invalid() {
    assert!(SwitchId::from_id(16).is_none());
    assert!(SwitchId::from_id(100).is_none());
    assert!(SwitchId::from_id(usize::MAX).is_none());
}

#[test]
fn switch_id_roundtrip() {
    for id in 0..MAX_SWITCH {
        let switch_id = SwitchId::from_id(id).unwrap();
        assert_eq!(switch_id.id(), id);
    }
}

#[test]
fn all_switches_have_info() {
    for id in 0..MAX_SWITCH {
        let info = SwitchId::from_id(id).map(|s| s.info());
        assert!(info.is_some(), "Switch {} should have info", id);
    }
}

#[test]
fn switch_info_has_valid_ranges() {
    for id in 0..MAX_SWITCH {
        let info = SwitchId::from_id(id).unwrap().info();
        assert!(
            info.min_value <= info.max_value,
            "Switch {} min ({}) > max ({})",
            id,
            info.min_value,
            info.max_value
        );
        assert!(info.step > 0.0, "Switch {} step must be positive", id);
    }
}

#[test]
fn controllable_switches_are_writable() {
    // Switches 0-5 should be controllable
    let writable_ids = [0, 1, 2, 3, 4, 5];
    for id in writable_ids {
        let info = SwitchId::from_id(id).unwrap().info();
        assert!(
            info.can_write,
            "Switch {} ({}) should be writable",
            id, info.name
        );
    }
}

#[test]
fn sensor_switches_are_readonly() {
    // Switches 6-15 should be read-only
    for id in 6..16 {
        let info = SwitchId::from_id(id).unwrap().info();
        assert!(
            !info.can_write,
            "Switch {} ({}) should be read-only",
            id, info.name
        );
    }
}

#[test]
fn boolean_switches_have_correct_range() {
    // Boolean switches should have 0-1 range with step 1
    let boolean_ids = [0, 1, 4, 5, 15];
    for id in boolean_ids {
        let info = SwitchId::from_id(id).unwrap().info();
        assert_eq!(info.min_value, 0.0, "Boolean switch {} min should be 0", id);
        assert_eq!(info.max_value, 1.0, "Boolean switch {} max should be 1", id);
        assert_eq!(info.step, 1.0, "Boolean switch {} step should be 1", id);
    }
}

#[test]
fn pwm_switches_have_correct_range() {
    // Dew heater switches should have 0-255 range
    let pwm_ids = [2, 3];
    for id in pwm_ids {
        let info = SwitchId::from_id(id).unwrap().info();
        assert_eq!(info.min_value, 0.0, "PWM switch {} min should be 0", id);
        assert_eq!(info.max_value, 255.0, "PWM switch {} max should be 255", id);
        assert_eq!(info.step, 1.0, "PWM switch {} step should be 1", id);
    }
}

#[test]
fn switch_names_are_not_empty() {
    for id in 0..MAX_SWITCH {
        let info = SwitchId::from_id(id).unwrap().info();
        assert!(
            !info.name.is_empty(),
            "Switch {} name should not be empty",
            id
        );
    }
}

#[test]
fn switch_descriptions_are_not_empty() {
    for id in 0..MAX_SWITCH {
        let info = SwitchId::from_id(id).unwrap().info();
        assert!(
            !info.description.is_empty(),
            "Switch {} description should not be empty",
            id
        );
    }
}

#[test]
fn specific_switch_names() {
    assert_eq!(SwitchId::from_id(0).unwrap().info().name, "Quad 12V Output");
    assert_eq!(
        SwitchId::from_id(1).unwrap().info().name,
        "Adjustable Output"
    );
    assert_eq!(SwitchId::from_id(2).unwrap().info().name, "Dew Heater A");
    assert_eq!(SwitchId::from_id(3).unwrap().info().name, "Dew Heater B");
    assert_eq!(SwitchId::from_id(4).unwrap().info().name, "USB Hub");
    assert_eq!(SwitchId::from_id(5).unwrap().info().name, "Auto-Dew");
    assert_eq!(SwitchId::from_id(6).unwrap().info().name, "Average Current");
    assert_eq!(SwitchId::from_id(7).unwrap().info().name, "Amp Hours");
    assert_eq!(SwitchId::from_id(8).unwrap().info().name, "Watt Hours");
    assert_eq!(SwitchId::from_id(9).unwrap().info().name, "Uptime");
    assert_eq!(SwitchId::from_id(10).unwrap().info().name, "Input Voltage");
    assert_eq!(SwitchId::from_id(11).unwrap().info().name, "Total Current");
    assert_eq!(SwitchId::from_id(12).unwrap().info().name, "Temperature");
    assert_eq!(SwitchId::from_id(13).unwrap().info().name, "Humidity");
    assert_eq!(SwitchId::from_id(14).unwrap().info().name, "Dewpoint");
    assert_eq!(SwitchId::from_id(15).unwrap().info().name, "Power Warning");
}
