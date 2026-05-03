Feature: Telescope-following pointing mode (F1, F2, F5, F6)

  When `pointing.telescope` is configured, every `StartExposure`
  reads RA/Dec from the configured ASCOM Telescope (with an
  optional configurable arcsec offset) and uses that as the
  exposure's pointing snapshot. Static-mode contracts P1–P7 are
  unchanged when `pointing.telescope` is absent — those scenarios
  live in `pointing_api.feature`.

  Scenario: F1/F4 — every exposure snapshots RA/Dec from the mount
    Given a mount reports RA 10.0 hours and Dec 30.0 degrees
    And the camera is configured to follow that mount
    And the camera is started and connected in follow mode
    Then after a successful exposure, the position endpoint reports RA approximately 150.0 Dec approximately 30.0

  Scenario: F2 — mount read failure surfaces as an exposure error
    Given a mount that errors on every read
    And the camera is configured to follow that mount
    And the camera is started and connected in follow mode
    When I StartExposure with default parameters
    Then the exposure fails with ASCOM UNSPECIFIED_ERROR

  Scenario: F5 — RA offset is applied to the snapshot
    Given a mount reports RA 10.0 hours and Dec 30.0 degrees
    And the camera is configured to follow that mount with offset RA 3600 arcsec and Dec 0 arcsec
    And the camera is started and connected in follow mode
    Then after a successful exposure, the position endpoint reports RA approximately 151.0 Dec approximately 30.0

  Scenario: F5 — Dec offset is applied to the snapshot
    Given a mount reports RA 10.0 hours and Dec 30.0 degrees
    And the camera is configured to follow that mount with offset RA 0 arcsec and Dec -3600 arcsec
    And the camera is started and connected in follow mode
    Then after a successful exposure, the position endpoint reports RA approximately 150.0 Dec approximately 29.0

  Scenario: F6 — POST /sky-survey/position is rejected in follow mode
    Given a mount reports RA 0.0 hours and Dec 0.0 degrees
    And the camera is configured to follow that mount
    And the camera is started and connected in follow mode
    When I POST RA 1 Dec 2 to the position endpoint
    Then the response status is 409
