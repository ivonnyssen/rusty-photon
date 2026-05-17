Feature: Rotator metadata
  The Falcon Rotator advertises ASCOM IRotatorV4 metadata that is fixed for
  the device family — clients use these values to size their UI and decide
  how to issue Move targets.

  Scenario: CanReverse is always advertised true
    Given a running pa-falcon-rotator service
    When I connect the rotator
    Then CanReverse should be true

  Scenario: StepSize matches the vendor product page
    Given a running pa-falcon-rotator service
    When I connect the rotator
    Then StepSize should be 0.01155

  Scenario: Rotator name comes from config
    Given a running pa-falcon-rotator service
    Then Name should be "Pegasus Falcon Rotator"

  Scenario: Rotator UniqueID comes from config
    Given a running pa-falcon-rotator service
    Then UniqueID should be "pa-falcon-rotator-001"

  Scenario: Rotator advertises ASCOM IRotatorV4
    Given a running pa-falcon-rotator service
    When I connect the rotator
    Then InterfaceVersion should be 4
