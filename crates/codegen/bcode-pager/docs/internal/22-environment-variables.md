# Feature environment variables (operator reference)

One variable per registered feature, read once at startup. Accepted values are `1`/`true`/`on`
and `0`/`false`/`off`; anything else is ignored and the next tier decides. A pin in
`requirements.toml` outranks all of these — see `25-enterprise.md`.

Some names predate the key they gate and are not spelled after it; the table is the authority.

| Variable | Feature key | Default |
| --- | --- | --- |
| `BCODE_SESSION_SEARCH` | `session_search` | on |
| `BCODE_LSP_TOOLS` | `lsp_tools` | off |
| `BCODE_WEB_FETCH` | `web_fetch` | off |
| `BCODE_SESSION_RECAP` | `session_recap` | on |
| `BCODE_ASK_USER_QUESTION` | `ask_user_question` | on |
| `BCODE_VOICE_MODE` | `voice_mode` | on |
| `BCODE_WRITE_FILE` | `write_file` | on |
| `BCODE_FEEDBACK_ENABLED` | `feedback` | on |
| `BCODE_FEEDBACK_TRACE_CARD` | `feedback_trace_card` | off |
| `BCODE_TURN_SUMMARY` | `turn_summary` | on |
| `BCODE_CANCEL_REWIND` | `cancel_rewind` | on |
| `BCODE_COMPACTION_VERBATIM_INPUT` | `compaction_verbatim_input` | on |
| `BCODE_TWO_PASS_COMPACTION` | `two_pass_compaction` | on |
| `BCODE_BACKEND_SEARCH` | `backend_tools` | on |
| `BCODE_AUTO_WAKE` | `auto_wake` | on |
| `BCODE_SUBAGENT_WORKTREE_SNAPSHOT` | `subagent_worktree_snapshot` | off |
| `BCODE_ACTIVE_AGENT_MESSAGES` | `active_agent_messages` | off |
| `BCODE_REPO_STATUS_IN_SYSTEM_PROMPT` | `repo_status_in_system_prompt` | on |
| `BCODE_DOCK` | `dock` | off |

This table is checked against `FEATURES` by `registered_features_are_documented`.
