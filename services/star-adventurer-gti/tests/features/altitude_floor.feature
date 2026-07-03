Feature: Altitude floor on slew and sync targets
  The driver rejects slew and sync targets whose apparent altitude is
  below mount.min_altitude_degrees. Altitude is computed from the
  target hour angle, declination, and site latitude via
  sin(alt) = sin(lat) * sin(dec) + cos(lat) * cos(dec) * cos(HA); a
  target below the floor returns invalid-value naming the computed
  altitude, before any wire motion. The default floor 0.0 is the
  geometric horizon; positive floors add an operator buffer for
  refraction or local obstructions; negative floors permit
  below-horizon pointing (dust-cap operations, closed-roof flats).

  Apparent altitude is a function of hour angle, not right ascension,
  so these scenarios address targets by hour angle: the steps compute
  RA = LST - HA when they run. Worked values below are for site
  latitude 45 N — HA 0 h / Dec -44 sits at altitude +1.0, HA -3 h /
  Dec -40 at -4.1, HA -3 h / Dec -30 at +4.6. Exact-boundary behaviour
  (a target at precisely the floor is accepted) is pinned by unit
  tests, where the hour angle is passed directly instead of round-
  tripping through wallclock LST.

  Scenario: Slew just above the horizon is accepted at the default floor
    Given a star-adventurer service configured with site latitude 45.0 degrees and minimum target altitude 0.0 degrees
    When I connect the device
    And I slew asynchronously to a target at hour angle 0.0 hours and Dec -44.0 degrees
    Then TargetDeclination should be -44.0 degrees within 0.001

  Scenario: Slew below the horizon is rejected with invalid-value
    Given a star-adventurer service configured with site latitude 45.0 degrees and minimum target altitude 0.0 degrees
    When I connect the device
    And I try to slew asynchronously to a target at hour angle -3.0 hours and Dec -40.0 degrees
    Then the operation should fail with invalid-value
    And the error message should mention "altitude"

  Scenario: Low target above the horizon at extreme Dec is accepted
    Given a star-adventurer service configured with site latitude 45.0 degrees and minimum target altitude 0.0 degrees
    When I connect the device
    And I slew asynchronously to a target at hour angle -3.0 hours and Dec -30.0 degrees
    Then TargetDeclination should be -30.0 degrees within 0.001

  Scenario: A raised floor rejects a target the default floor accepts
    Given a star-adventurer service configured with site latitude 45.0 degrees and minimum target altitude 5.0 degrees
    When I connect the device
    And I try to slew asynchronously to a target at hour angle -3.0 hours and Dec -30.0 degrees
    Then the operation should fail with invalid-value
    And the error message should mention "altitude"

  Scenario: A negative floor permits below-horizon pointing
    Given a star-adventurer service configured with site latitude 45.0 degrees and minimum target altitude -45.0 degrees
    When I connect the device
    And I slew asynchronously to a target at hour angle -3.0 hours and Dec -40.0 degrees
    Then TargetDeclination should be -40.0 degrees within 0.001

  Scenario: Sync below the floor is rejected with invalid-value
    Given a star-adventurer service configured with site latitude 45.0 degrees and minimum target altitude 0.0 degrees
    When I connect the device
    And I try to sync to a target at hour angle -3.0 hours and Dec -40.0 degrees
    Then the operation should fail with invalid-value
    And the error message should mention "altitude"
