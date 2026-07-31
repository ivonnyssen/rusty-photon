Feature: The fake-mount hazard (joins.fake-mount)
  planetarium-bridge serves a virtual ASCOM Telescope for planetarium
  apps to connect to — it never touches hardware, and slews against it
  "just succeed" without moving anything. Wiring it in as rp's real
  mount (equipment.mount.alpaca_url) would therefore defeat every
  motion safeguard rp believes it has, so doctor grades that a hard
  failure, never a warning (docs/services/planetarium-bridge.md,
  Doctor integration section). Two legs: a static loopback port join
  (the same resolution every client-target join uses), and a UniqueID
  probe of the configured mount's management API for the host-name
  URLs a rig actually uses, where the loopback join is silently
  skipped by design. The bridge also registers with doctor like any
  Alpaca service, and doctor --fix wires its nested rp client block
  (rp.service_auth / rp.ca_cert) the same way it wires sentinel's and
  session-runner's top-level pair.

  Scenario: rp's mount wired at the bridge's port is a hard failure
    Given a config file "planetarium-bridge.json" containing:
      """
      { "server": { "port": 11126 }, "device": { "unique_id": "bridge-uid" } }
      """
    And a config file "rp.json" containing:
      """
      { "server": { "port": 11115 },
        "equipment": { "mount": { "alpaca_url": "http://127.0.0.1:11126" } } }
      """
    When I run doctor with --json
    Then the report contains a "fail" check named "joins.fake-mount" for service "rp"
    And that check's detail mentions "planetarium-bridge"
    And doctor exits with code 1

  Scenario: a mount answering with the bridge's UniqueID is a hard failure
    Given a stub management endpoint serving a device with UniqueID "bridge-uid"
    And a config file "planetarium-bridge.json" containing:
      """
      { "server": { "port": 11126 }, "device": { "unique_id": "bridge-uid" } }
      """
    And a config file "rp.json" whose equipment.mount.alpaca_url points at the stub endpoint
    When I run doctor with --json
    Then the report contains a "fail" check named "joins.fake-mount" for service "rp"
    And that check's detail mentions "UniqueID"
    And doctor exits with code 1

  Scenario: a real mount answering its own UniqueID stays silent
    Given a stub management endpoint serving a device with UniqueID "real-mount-uid"
    And a config file "planetarium-bridge.json" containing:
      """
      { "server": { "port": 11126 }, "device": { "unique_id": "bridge-uid" } }
      """
    And a config file "rp.json" whose equipment.mount.alpaca_url points at the stub endpoint
    When I run doctor with --json
    Then the report contains no check named "joins.fake-mount"

  Scenario: doctor --fix wires the bridge's nested rp client block
    Given a config file "planetarium-bridge.json" containing:
      """
      { "server": { "port": 11126 },
        "rp": { "mcp_server_url": "http://127.0.0.1:11115/mcp" } }
      """
    When I run doctor with --fix and --json
    Then the config file "planetarium-bridge.json" has "/rp/ca_cert" pointing at the pki file "ca.pem"
    And the config file "planetarium-bridge.json" has the string "observatory" at "/rp/service_auth/username"
