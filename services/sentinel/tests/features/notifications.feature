Feature: Sending notifications
  The sentinel service sends notifications through configured channels
  when state transitions occur.

  Scenario: Pushover notification sends successfully
    Given a Pushover notifier with valid credentials
    When a notification is sent with title "Alert" and message "Roof unsafe"
    Then the notification should succeed

  Scenario: Pushover notification fails on API error
    Given a Pushover notifier that returns an API error
    When a notification is sent with title "Alert" and message "Roof unsafe"
    Then the notification should fail with an error

  Scenario: Pushover notification fails on network error
    Given a Pushover notifier that is unreachable
    When a notification is sent with title "Alert" and message "Roof unsafe"
    Then the notification should fail with an error

  Scenario: Pushover uses default title when empty
    Given a Pushover notifier with valid credentials
    When a notification is sent with title "" and message "test"
    Then the notification should succeed
