Feature: Park, unpark, and SetPark
  Park stops tracking, slews both axes to the in-memory park-target
  encoder pair, and sets AtPark when both axes report stopped. The park
  target is loaded on connect from `mount.park_ra_ticks` /
  `mount.park_dec_ticks` in the config, falling back to the encoder
  positions captured during the init handshake when those values are
  absent. Tracking remains disabled after Park (per ASCOM). Unpark clears
  AtPark but does not auto-enable tracking. SetPark captures the current
  encoder pair and writes it back into the running config file via
  atomic rename.

  Scenario: AtPark is false on first connect
    Given a running star-adventurer service
    When I connect the device
    Then AtPark should be false

  Scenario: Park stops tracking before slewing home
    Given a running star-adventurer service
    When I connect the device
    And I enable tracking
    And I park the mount
    Then the mount should have received command :K1 before any :S1
    And Tracking should be false

  Scenario: Park targets the handshake-captured encoder pair when the config has no park values
    Given a running star-adventurer service
    And the RA-axis encoder reads 0 ticks
    And the Dec-axis encoder reads 0 ticks
    When I connect the device
    And I park the mount
    Then the mount should have received a :S1 command targeting encoder 0
    And the mount should have received a :S2 command targeting encoder 0

  Scenario: Park targets the Dec encoder position captured at handshake
    Given a running star-adventurer service
    And the Dec-axis encoder reads 12000 ticks
    When I connect the device
    And I park the mount
    Then the mount should have received a :S2 command targeting encoder 12000

  Scenario: Park targets the configured park_ra_ticks when present
    Given a star-adventurer service configured with park_ra_ticks 5000 and park_dec_ticks -7000
    When I connect the device
    And I park the mount
    Then the mount should have received a :S1 command targeting encoder 5000
    And the mount should have received a :S2 command targeting encoder -7000

  Scenario: Park is idempotent
    Given a running star-adventurer service
    When I connect the device
    And I park the mount
    And the mount reports both axes stopped at encoder 0
    And I park the mount
    Then the mount should not have received a second :S1 command

  Scenario: AtPark becomes true once both axes settle at the park target
    Given a running star-adventurer service
    When I connect the device
    And I park the mount
    And the mount reports both axes stopped at encoder 0
    Then AtPark should eventually be true within 5 seconds

  Scenario: Unpark clears AtPark
    Given a running star-adventurer service
    And the device is parked
    When I unpark the mount
    Then AtPark should be false

  Scenario: Unpark does not auto-enable tracking
    Given a running star-adventurer service
    And the device is parked
    When I unpark the mount
    Then Tracking should be false

  Scenario: SetPark writes the current encoder ticks back to the config file
    Given a running star-adventurer service
    And the RA-axis encoder reads 8000 ticks
    And the Dec-axis encoder reads -3000 ticks
    When I connect the device
    And I set the park position
    Then the persisted config should have park_ra_ticks 8000 and park_dec_ticks -3000

  Scenario: SetPark fails when disconnected
    Given a running star-adventurer service
    When I try to set the park position
    Then the operation should fail with not-connected

  Scenario: Park fails while disconnected
    Given a running star-adventurer service
    When I try to park the mount
    Then the operation should fail with not-connected
