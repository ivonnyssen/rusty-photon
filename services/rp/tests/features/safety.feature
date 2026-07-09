@serial
Feature: Safety enforcement
  rp owns safety. Configured ASCOM SafetyMonitor devices are polled at
  safety.poll_interval, and conditions are safe only while every monitor
  reports safe; a monitor that cannot be read counts as unsafe. An
  unsafe transition interrupts the active session: open MCP sessions
  are terminated, the /mcp endpoint answers 503, and the session waits
  in "interrupted". The unsafe transition also stops the hardware,
  best-effort: in-progress exposures are aborted, guiding is stopped
  through the configured guider service (emitting "guide_stopped" with
  reason "safety"), and the mount is parked. The safe transition lifts
  the gate and re-invokes the orchestrator with recovery context — the
  same workflow and session ids, recovery reason "safety_interruption"
  — returning the session to "active". Each monitor transition emits a
  "safety_changed" event.

  Scenario: An unsafe transition interrupts the session and the safe transition re-invokes the orchestrator
    Given a running Alpaca simulator
    And a test orchestrator that waits for a stop signal
    And a safety monitor on the simulator
    And a test webhook receiver subscribed to "safety_changed"
    And rp is running with equipment and both plugins configured
    When a session is started via the REST API
    And the safety monitor reports unsafe
    Then the test webhook receiver should receive a "safety_changed" event
    And the "safety_changed" event payload field "new_state" should be "unsafe"
    And the session status should become "interrupted"
    When the safety monitor reports safe again
    Then the session status should become "active"
    And the test orchestrator should have been re-invoked with recovery reason "safety_interruption"
    And the recovery invocation should carry the original workflow and session ids

  Scenario: An unsafe transition stops guiding and parks the mount
    Given a running Alpaca simulator
    And a safety monitor on the simulator
    And a stub guider returning canned guiding stats
    And a test webhook receiver subscribed to "guide_stopped"
    And rp is running with a camera and a mount on the simulator
    And an MCP client connected to rp
    When the operator unparks the mount
    And the safety monitor reports unsafe
    Then the stub guider should have received a stop request within 5 seconds
    And the mount should report parked on the simulator within 10 seconds
    And the test webhook receiver should receive a "guide_stopped" event
    And the "guide_stopped" event payload field "reason" should be "safety"

  Scenario: The MCP endpoint rejects requests while conditions are unsafe
    Given a running Alpaca simulator
    And a safety monitor on the simulator
    And rp is running with a camera and filter wheel on the simulator
    When the safety monitor reports unsafe
    Then the MCP endpoint should reject requests with 503 within 5 seconds
    When the safety monitor reports safe again
    Then the MCP endpoint should accept requests again within 5 seconds
