Feature: Activity stream
  /stream renders rp's real-time event stream as a narrative feed. The
  browser holds one SSE connection to the BFF's /stream/events proxy; the
  proxy holds one connection to rp's GET /api/events/subscribe, forwarding
  the browser's Last-Event-ID cursor to rp (a fresh page subscribes from 0,
  so rp's retained history replays into the feed) and translating each event
  envelope into named HTML fragments: an "event: feed" frame per envelope
  (its SSE id is the envelope's event_seq) plus the status-strip slot frames
  the event warrants. On rp connection loss the proxy pushes an "operation"
  slot fragment saying rp is unreachable and ends the stream, so the
  browser's EventSource reconnects with its cursor and the page self-heals.

  Scenario: The stream page declares the SSE wiring and the fold panel
    Given a running rp orchestrator with an empty roster
    And a BFF pointed at rp
    When I open the stream page
    Then the page declares an SSE connection to "/stream/events"
    And the feed region prepends "feed" events
    And the fold panel polls "/stream/equipment" every 10 seconds

  Scenario: Session events arrive as rendered feed cards with the rp sequence as SSE id
    Given a running rp orchestrator with an empty roster
    And a BFF pointed at rp
    And a connected reader on the BFF event stream
    When a session is started on rp
    And the session is stopped on rp
    Then a "feed" frame arrives whose card mentions "Session started"
    And a "feed" frame arrives whose card mentions "Session stopped"
    And every received "feed" frame carries a numeric SSE id

  Scenario: Reconnecting with a cursor replays only events after it
    Given a running rp orchestrator with an empty roster
    And a BFF pointed at rp
    And a session was started and stopped on rp
    And a connected reader on the BFF event stream
    And a "feed" frame arrives whose card mentions "Session stopped"
    When I remember the highest received SSE id and reconnect with it as the cursor
    And a session is started on rp
    Then a "feed" frame arrives whose card mentions "Session started"
    And every received "feed" frame carries an SSE id greater than the cursor

  Scenario: An unreachable rp yields the retry status frame and the stream ends
    Given a BFF pointed at an unreachable rp
    When I connect a reader to the BFF event stream
    Then an "operation" frame arrives mentioning "unreachable"
    And the event stream ends

