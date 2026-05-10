Feature: Connection lifecycle
  The mount device opens its transport on Connected = true and runs an
  initialisation handshake before reporting Connected. Subsequent connects
  are reference-counted; the transport is torn down only when the last
  client disconnects. Disconnect aborts any motion in progress and stops
  tracking before closing the transport.

  Scenario: Device starts disconnected
    Given a running star-adventurer service
    Then the device should be disconnected

  Scenario: Device connects successfully after handshake
    Given a running star-adventurer service
    When I connect the device
    Then the device should be connected

  Scenario: Connect runs the initialisation handshake in order
    Given a running star-adventurer service
    When I connect the device
    Then the mount should have received commands in order:
      | command |
      | :F1     |
      | :F2     |
      | :a1     |
      | :a2     |
      | :b1     |
      | :g1     |
      | :g2     |
      | :e1     |
      | :j1     |
      | :j2     |

  Scenario: Connect populates the parameter cache from handshake replies
    Given a mount that reports CPR 3628800 on both axes
    And a mount that reports timer frequency 16000000
    And a running star-adventurer service
    When I connect the device
    Then the parameter cache should report CPR 3628800 on the RA axis
    And the parameter cache should report CPR 3628800 on the Dec axis
    And the parameter cache should report timer frequency 16000000

  Scenario: Disconnect after connect releases the transport
    Given a running star-adventurer service
    When I connect the device
    And I disconnect the device
    Then the device should be disconnected

  Scenario: Concurrent connects share one transport
    Given a running star-adventurer service
    When two clients connect the device
    Then the underlying transport should have been opened exactly once

  # Multi-client disconnect ref-counting is unit-tested at
  # `services/star-adventurer-gti/src/transport_manager.rs::tests::
  # connect_is_reference_counted` because BDD against a single-process
  # binary cannot drive two distinct ASCOM client sessions through one
  # device instance. Keeping the description here so a reader has a
  # pointer to the assertion.
  Scenario: Last disconnect tears the transport down
    Given a running star-adventurer service
    When I connect the device
    And I disconnect the device
    Then the device should be disconnected

  Scenario: Disconnect aborts motion in progress
    Given a running star-adventurer service
    When I connect the device
    And the mount is slewing
    And I disconnect the device
    Then the mount should have received command :L1
    And the mount should have received command :L2
