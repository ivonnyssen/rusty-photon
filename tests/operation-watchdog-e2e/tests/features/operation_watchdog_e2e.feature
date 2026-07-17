@serial
Feature: Operation watchdog end-to-end (real rp + real sentinel)
  Sentinel's operation watchdog subscribes to rp's real /api/events/subscribe
  stream, tracks each operation against the predicted deadline rp stamps on the
  *_started event, and escalates through the corrective-action ladder when one
  overruns. These scenarios run a real rp binary and a real sentinel binary
  together — the per-service suites only cover each half against stubs.

  The driver is the centering operation: rp advertises a centering deadline but
  does NOT enforce it (it is advisory for the watchdog), so the watchdog timer
  is the only thing that fires. Because centering has no single Alpaca device to
  abort, the ladder skips the abort rung and exercises the restart rung — the
  rung that is otherwise only unit-tested. The rp service the ladder restarts
  is discovered from the service manager (a directory-backed stub here), and
  the restart is the manager's derived restart of the rusty-photon-rp unit.

  Scenario: A wedged centering operation escalates to the restart rung
    Given rp's plate solver hangs so a centering operation never completes
    And a running rp and sentinel with the operation watchdog enabled
    When the operator starts centering on a target
    Then the watchdog escalates the centering operation
    And the corrective ladder restarts the rp service

  Scenario: A centering operation that completes in time is not escalated
    Given rp's plate solver returns the target field center immediately
    And a running rp and sentinel with the operation watchdog enabled
    When the operator centers on the target and it converges
    Then the watchdog records no escalation for the centering operation

  Scenario: Sentinel reports rp unresponsive when its event stream dies
    Given a running rp and sentinel with the operation watchdog enabled
    When rp stops responding
    Then the watchdog reports rp unresponsive
