@browser
Feature: Real-browser behavior
  These scenarios drive a real headless Firefox via WebDriver to prove the one
  thing the server-output layers cannot: that the vendored htmx.min.js actually
  loads and executes the declared swaps in a browser engine (obligation P3).
  They are advisory and gated behind UI_BROWSER_TESTS=1, and run on a single
  environment — the server-bytes layers (P1 correctness, P2 OS-invariance) carry
  the cross-OS guarantee, so behavior proven correct here holds on every OS
  without a browser on every OS.

  Scenario: The configuration form renders in a real browser
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    When I load the dsd-fp2 config page in a browser
    Then the browser renders the configuration form

  Scenario: Unlocking the identity field swaps the card via htmx
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    When I load the dsd-fp2 config page in a browser
    And I click the unlock link for cover_calibrator.unique_id
    Then the browser shows cover_calibrator.unique_id editable
