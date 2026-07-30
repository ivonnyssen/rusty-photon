@wip
Feature: Align imports targets
  Align (sync) is the sole import gesture: each accepted sync verb fires one
  add_target MCP call to rp carrying bare ICRS coordinates plus provenance —
  no name and no catalog_ref, because naming, dedup, and slug allocation are
  rp policy. Slew verbs are simulated motion only and never import. Below-
  horizon Aligns import normally (the reported-position floor only changes
  what the client reads back). Coordinates are converted per assume_epoch
  once, at receipt; everything downstream is ICRS.

  Background:
    Given a stub rp MCP server is running

  Scenario: SyncToCoordinates fires one add_target import with the received coordinates
    Given the bridge is running
    And a connected planetarium client
    When the client aligns at ra 20.9877 hours dec 44.5253 degrees
    Then the stub rp should receive 1 add_target call
    And the import should carry ra_hours 20.9877 and dec_degrees 44.5253
    And the import source kind should be "planetarium-bridge" with a client address and an RFC 3339 receipt time
    And the import should carry no catalog_ref and no display_name

  Scenario: SyncToTarget imports the previously set Target coordinates
    Given the bridge is running
    And a connected planetarium client
    When the client sets TargetRightAscension to 0.79253 hours
    And the client sets TargetDeclination to -25.2875 degrees
    And the client aligns to target
    Then the stub rp should receive 1 add_target call
    And the import should carry ra_hours 0.79253 and dec_degrees -25.2875

  Scenario: Slew verbs never import
    Given the simulated slew duration is "1s"
    And the bridge is running
    And a connected planetarium client
    When the client starts an async slew to ra 5.0 hours dec 40.0 degrees
    And all simulated slews have completed
    And the client slews blocking to ra 6.0 hours dec 30.0 degrees
    And the client sets TargetRightAscension to 7.0 hours
    And the client sets TargetDeclination to 20.0 degrees
    And the client slews to target asynchronously
    And all simulated slews have completed
    Then the stub rp should receive 0 add_target calls

  Scenario: A below-horizon Align imports normally
    Given the configured site is latitude 33.0 longitude -117.0
    And the bridge is running
    And a connected planetarium client
    When the client aligns at ra 5.0 hours dec -80.0 degrees
    Then the stub rp should receive 1 add_target call
    And the import should carry ra_hours 5.0 and dec_degrees -80.0

  Scenario: Repeated Aligns each submit an import
    Given the bridge is running
    And a connected planetarium client
    When the client aligns at ra 0.79253 hours dec -25.2875 degrees
    And the client aligns at ra 0.79253 hours dec -25.2875 degrees
    And the client aligns at ra 0.79253 hours dec -25.2875 degrees
    Then the stub rp should receive 3 add_target calls

  Scenario: Under assume_epoch jnow received coordinates are converted to ICRS before import
    Given assume_epoch is "jnow"
    And the bridge is running
    And a connected planetarium client
    When the client aligns at ra 14.0 hours dec -40.0 degrees
    Then the stub rp should receive 1 add_target call
    And the imported coordinates should be within 5.0 arcminutes of ra 13.97306 hours dec -39.86972 degrees
    And the imported coordinates should be at least 15.0 arcminutes away from ra 14.0 hours dec -40.0 degrees

  Scenario: An out-of-range Align is rejected and fires no import
    Given the bridge is running
    And a connected planetarium client
    When the client tries to align at ra 24.5 hours dec 10.0 degrees
    Then the call should be rejected with ASCOM INVALID_VALUE
    And the stub rp should receive 0 add_target calls
