Feature: dsd-fp2 configuration page
  The BFF serves a configuration page for a real dsd-fp2 driver, backed by the
  driver's own `config.get` / `config.apply` ASCOM actions over HTTP. Opening
  the page renders a form filled with the driver's current effective
  configuration; a field pinned by a command-line override is shown disabled.
  Submitting the form calls `config.apply` on the driver: an unchanged
  submission persists nothing and reports no reload was needed; a valid change
  triggers the driver's in-process reload, the page reports it is reconnecting,
  and once the driver has reloaded the page serves the new value; an invalid
  change re-renders the form with the driver's field-level error and the
  submitted value preserved; and an unreachable driver surfaces an error banner.

  Scenario: The config page renders the driver's current configuration
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    When I open the dsd-fp2 config page
    Then the page shows the value "/dev/ttyACM0"
    And the page shows the value "4096"

  Scenario: A serial-port override is shown read-only
    Given a dsd-fp2 driver running with the serial port pinned to "/dev/ttyACM9" by a command-line override
    When I open the dsd-fp2 config page
    Then the serial.port field is disabled
    And the page explains the field is pinned by a command-line override

  Scenario: A valid change is applied and the page reports the driver is reloading
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    When I submit the config form setting max_brightness to 2048
    Then the page reports the driver is reloading
    And the page polls /config/dsd-fp2/status every 1s for reconnection

  Scenario: The reloaded driver's new configuration is served back through the page
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    When I submit the config form setting max_brightness to 2048
    And I poll the reconnect status until max_brightness 2048 is served
    Then the page shows the value "2048"

  Scenario: An unchanged submission reports the configuration was saved without a reload
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    When I submit the config form without changing anything
    Then the page reports the configuration was saved without a reload
    And the page shows the value "4096"

  Scenario: An invalid change re-renders the form with a field error
    Given a dsd-fp2 driver running with serial.port "/dev/ttyACM0" and max_brightness 4096
    When I submit the config form setting baud_rate to 0
    Then the form shows the validation error "must be greater than 0" on serial.baud_rate
    And the submitted baud_rate value 0 is preserved

  Scenario: An unreachable driver surfaces an error
    Given the BFF is pointed at a dsd-fp2 driver that is not running
    When I open the dsd-fp2 config page
    Then the page shows a driver error
