@wip
Feature: Round-trippable file-naming template config-load validation (P1 planned)
  P1 turns `session.file_naming_pattern` from render-only into a
  round-trippable template rp both renders and parses back (plus a new
  `session.directory_pattern`), superseding the `{duration}`/
  `{sequence}` tokens with `{exposure}`/`{frame_number}` — the parser
  keeps accepting the old names as deprecated aliases for backward
  compatibility (rp.md § Persistence, rp-targets.md § File-naming
  template). The pattern is parsed and checked at startup: a bad
  pattern fails the load, not a session. Rejection rules: the pattern
  must carry every token needed to derive the quota key (`{target}`,
  `{filter}`, `{binning}`, `{exposure}`) plus a per-frame uniqueness
  token (`{uuid8}` or `{frame_number}`); it must compile to an
  unambiguous anchored regex — two variable-width tokens can't sit
  adjacent with no literal separator excluded from both charsets; and
  unknown tokens are rejected. *(Planned, P1 — not yet implemented;
  scenarios are tagged @wip.)*

  Scenario: A pattern missing a required quota token is rejected at config load
    Given an rp config with file_naming_pattern "{target}_{frame_number}_{uuid8}"
    When rp attempts to start
    Then rp should fail to start

  # {frame_number} and {exposure} are both purely-numeric charsets with
  # no literal separator between them here — the doc's own canonical
  # example of an unresolvable split.
  Scenario: A pattern placing two ambiguous variable-width tokens adjacent is rejected at config load
    Given an rp config with file_naming_pattern "{target}_{filter}_{binning}_{frame_number}{exposure}_fpos_{filter_position}_{sensor_temp}_{uuid8}"
    When rp attempts to start
    Then rp should fail to start

  Scenario: An unknown token is rejected at config load
    Given an rp config with file_naming_pattern "{target}_{filter}_{binning}_{frame_number}_{exposure}_{bogus_token}"
    When rp attempts to start
    Then rp should fail to start

  Scenario Outline: The default pattern and its deprecated-token-alias equivalent both start successfully
    Given an rp config with file_naming_pattern "<pattern>"
    When rp attempts to start
    Then rp should start successfully

    Examples:
      | pattern                                                                                            |
      | {target}_{filter}_{binning}_{frame_number}_{exposure}_fpos_{filter_position}_{sensor_temp}_{uuid8} |
      | {target}_{filter}_{binning}_{sequence}_{duration}_fpos_{filter_position}_{sensor_temp}_{uuid8}     |
