Feature: Position reads
  Each property read maps to a single FA request — there is no cached state.
  MechanicalPosition is the device's raw position in [0, 360). Position adds
  the driver-side sync_offset and re-normalises. TargetPosition returns the
  stored sky-coordinate target when one is outstanding, otherwise it falls
  back to the current Position.

  Scenario: MechanicalPosition reflects the device's reported degrees
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 142.30 degrees
    And I read MechanicalPosition
    Then MechanicalPosition should be 142.30 degrees
    And an FA command should have been issued

  Scenario: Position adds the driver-side sync offset to MechanicalPosition
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 142.30 degrees
    And the driver-side sync offset is -104.80 degrees
    And I read Position
    Then Position should be 37.50 degrees

  Scenario: Position normalises offset-induced negative values into [0, 360)
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 10.00 degrees
    And the driver-side sync offset is -30.00 degrees
    And I read Position
    Then Position should be 340.00 degrees

  Scenario: TargetPosition falls back to Position when no move is outstanding
    Given a running pa-falcon-rotator service
    When I connect the rotator
    And the rotator reports mechanical position 90.00 degrees
    And I read TargetPosition
    Then TargetPosition should be 90.00 degrees
