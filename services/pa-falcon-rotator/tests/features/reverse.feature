Feature: Reverse (EEPROM-wear-aware)
  FN:b writes the motor-reverse flag to EEPROM, which has finite write
  endurance. The driver therefore reads FA first and only issues FN when the
  requested value differs from the device's current state. Reverse get is a
  single FA read so operator changes from the Pegasus Unity app are visible
  on the next request.

  Scenario: Setting Reverse to the same value as the device is a no-op on EEPROM
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the device's motor_reverse is currently true
    And I set Reverse to true
    Then no FN command should have been sent

  Scenario: Setting Reverse to a different value writes FN to the device
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the device's motor_reverse is currently false
    And I set Reverse to true
    Then FN:1 should have been sent

  Scenario: Reading Reverse reflects the device's current motor_reverse
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the device's motor_reverse is currently true
    And I read Reverse
    Then Reverse should be true
