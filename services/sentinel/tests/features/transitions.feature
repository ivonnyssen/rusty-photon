Feature: State transitions and notification dispatch
  The sentinel engine detects state changes in monitors
  and dispatches notifications according to configured transition rules.

  Scenario: Safe to unsafe triggers notification
    Given a monitor named "Roof" in state "Safe"
    And a transition rule for "Roof" on "safe_to_unsafe" via "test"
    When the monitor transitions to "Unsafe"
    Then a notification should be dispatched
    And the notification message should contain "Roof"
    And the notification message should contain "Unsafe"

  Scenario: Unsafe to safe triggers notification
    Given a monitor named "Roof" in state "Unsafe"
    And a transition rule for "Roof" on "unsafe_to_safe" via "test"
    When the monitor transitions to "Safe"
    Then a notification should be dispatched

  Scenario: No notification on non-matching direction
    Given a monitor named "Roof" in state "Safe"
    And a transition rule for "Roof" on "unsafe_to_safe" via "test"
    When the monitor transitions to "Unsafe"
    Then no notification should be dispatched

  Scenario: No notification when state unchanged
    Given a monitor named "Roof" in state "Safe"
    And a transition rule for "Roof" on "safe_to_unsafe" via "test"
    When the monitor transitions to "Safe"
    Then no notification should be dispatched

  Scenario: Both direction matches either transition
    Given a monitor named "Roof" in state "Safe"
    And a transition rule for "Roof" on "both" via "test"
    When the monitor transitions to "Unsafe"
    Then a notification should be dispatched

  Scenario: Unknown state does not trigger notification
    Given a monitor named "Roof" in state "Safe"
    And a transition rule for "Roof" on "safe_to_unsafe" via "test"
    When the monitor transitions to "Unknown"
    Then no notification should be dispatched

  Scenario: Failed notification is recorded in history
    Given a monitor named "Roof" in state "Safe"
    And a transition rule for "Roof" on "safe_to_unsafe" via "failing"
    And a notifier "failing" that returns errors
    When the monitor transitions to "Unsafe"
    Then a notification should be dispatched
    And the notification should be marked as failed
