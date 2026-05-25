Feature: dsd-fp2 configuration page
  The BFF serves a configuration page for the dsd-fp2 driver, backed by the
  driver's own `config.get` / `config.apply` ASCOM actions. Opening the page
  renders a form filled with the driver's current effective configuration;
  fields pinned by a CLI override are shown disabled. Submitting the form calls
  `config.apply`: a valid change reports `status:"applying"` and the page polls
  the driver until it has reloaded, an invalid change re-renders the form with
  field-level errors and the submitted values preserved, and an unreachable
  driver surfaces an error.

  Scenario: The config page renders the driver's current configuration
    Given the dsd-fp2 driver reports serial.port "/dev/ttyACM0" and max_brightness 4096
    When I open the dsd-fp2 config page
    Then the page shows the value "/dev/ttyACM0"
    And the page shows the value "4096"

  Scenario: Override-pinned fields are shown read-only
    Given the dsd-fp2 driver reports serial.port "/dev/ttyACM9" pinned by a command-line override
    When I open the dsd-fp2 config page
    Then the serial.port field is disabled
    And the page explains the field is pinned by a command-line override

  Scenario: A valid change is applied and the page reports the driver is reconnecting
    Given the dsd-fp2 driver accepts config.apply with status applying reloading "cover_calibrator.max_brightness"
    When I submit the config form setting max_brightness to 2048
    Then the page reports the driver is reloading
    And the page polls /config/dsd-fp2/status every 1s for reconnection

  Scenario: A no-reload apply shows the driver's effective config, not the submitted value
    Given the dsd-fp2 driver reports serial.port "/dev/ttyACM0" and max_brightness 1234
    And the dsd-fp2 driver accepts config.apply with status ok
    When I submit the config form setting max_brightness to 9999
    Then the page shows the value "1234"
    And the page does not show the value "9999"

  Scenario: An invalid change re-renders the form with field errors
    Given the dsd-fp2 driver rejects config.apply with an invalid serial.baud_rate
    When I submit the config form setting baud_rate to 0
    Then the form shows the validation error "must be greater than 0" on serial.baud_rate
    And the submitted baud_rate value 0 is preserved

  Scenario: An unreachable driver surfaces an error
    Given the dsd-fp2 driver is unreachable
    When I open the dsd-fp2 config page
    Then the page shows a driver error
