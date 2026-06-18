@serial
Feature: Real-time event stream (SSE)
  rp exposes every operation event over `GET /api/events/subscribe` as a
  Server-Sent-Events stream. Each frame's SSE `id` is the envelope's
  `event_seq` (a monotonic u64 — the `Last-Event-ID` replay key), the SSE
  `event` is the event type, and `data` is the full event envelope JSON
  (carrying the `operation_id` that correlates an operation's lifecycle
  events). A client that reconnects with `Last-Event-ID` replays every
  buffered event after that cursor before tailing live, so events emitted
  while it was disconnected are recovered from rp's in-memory history ring.
  The stream is additive over the webhook delivery path — both run together.

  Scenario: A live subscriber receives an operation's lifecycle frames
    Given a running Alpaca simulator
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    And the mount is unparked
    And the mount tracking is set to true
    And the operator is subscribed to the event stream
    When the operator slews to ra 10.6847 dec 41.2689
    Then the event stream delivers the "slew_started" event
    And the event stream delivers the "slew_complete" event
    And the "slew_started" stream frame's SSE id equals its event_seq
    And the "slew_started" and "slew_complete" stream frames share one operation_id

  Scenario: A reconnecting subscriber replays events missed while disconnected
    Given a running Alpaca simulator
    And rp is running with a mount on the simulator
    And an MCP client connected to rp
    And the mount is unparked
    And the mount tracking is set to true
    And the operator is subscribed to the event stream
    When the operator slews to ra 10.6847 dec 41.2689
    And the event stream delivers the "slew_complete" event
    And the operator disconnects from the event stream
    And the operator parks the mount
    And the operator reconnects to the event stream from the last received event id
    Then the event stream delivers the "park_started" event
    And the event stream delivers the "park_complete" event
