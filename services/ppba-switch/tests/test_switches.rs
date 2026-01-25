//! Switch definition tests for PPBA Switch driver

use ppba_switch::{get_switch_info, SwitchId, MAX_SWITCH};

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
    assert!(SwitchId::from_id(u16::MAX).is_none());
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
        let info = get_switch_info(id);
        assert!(info.is_some(), "Switch {} should have info", id);
    }
}

#[test]
fn switch_info_has_valid_ranges() {
    for id in 0..MAX_SWITCH {
        let info = get_switch_info(id).unwrap();
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
        let info = get_switch_info(id).unwrap();
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
        let info = get_switch_info(id).unwrap();
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
        let info = get_switch_info(id).unwrap();
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
        let info = get_switch_info(id).unwrap();
        assert_eq!(info.min_value, 0.0, "PWM switch {} min should be 0", id);
        assert_eq!(info.max_value, 255.0, "PWM switch {} max should be 255", id);
        assert_eq!(info.step, 1.0, "PWM switch {} step should be 1", id);
    }
}

#[test]
fn switch_names_are_not_empty() {
    for id in 0..MAX_SWITCH {
        let info = get_switch_info(id).unwrap();
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
        let info = get_switch_info(id).unwrap();
        assert!(
            !info.description.is_empty(),
            "Switch {} description should not be empty",
            id
        );
    }
}

#[test]
fn specific_switch_names() {
    assert_eq!(get_switch_info(0).unwrap().name, "Quad 12V Output");
    assert_eq!(get_switch_info(1).unwrap().name, "Adjustable Output");
    assert_eq!(get_switch_info(2).unwrap().name, "Dew Heater A");
    assert_eq!(get_switch_info(3).unwrap().name, "Dew Heater B");
    assert_eq!(get_switch_info(4).unwrap().name, "USB Hub");
    assert_eq!(get_switch_info(5).unwrap().name, "Auto-Dew");
    assert_eq!(get_switch_info(6).unwrap().name, "Average Current");
    assert_eq!(get_switch_info(7).unwrap().name, "Amp Hours");
    assert_eq!(get_switch_info(8).unwrap().name, "Watt Hours");
    assert_eq!(get_switch_info(9).unwrap().name, "Uptime");
    assert_eq!(get_switch_info(10).unwrap().name, "Input Voltage");
    assert_eq!(get_switch_info(11).unwrap().name, "Total Current");
    assert_eq!(get_switch_info(12).unwrap().name, "Temperature");
    assert_eq!(get_switch_info(13).unwrap().name, "Humidity");
    assert_eq!(get_switch_info(14).unwrap().name, "Dewpoint");
    assert_eq!(get_switch_info(15).unwrap().name, "Power Warning");
}
