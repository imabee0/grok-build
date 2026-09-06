# Feature pinning (operator reference)

Every row in `FEATURES` (`crates/codegen/bcode-config-types/src/registry.rs`) appears here.
`registered_features_are_documented` fails the build if one does not, so add the row in the
same commit that adds the feature.

Resolution order, highest first: pin (`requirements.toml`) > environment variable > `config.toml` > remote > default.
A pin is the only tier an operator can make unoverridable.

| Key | Config path | Default | Remote tier |
| --- | --- | --- | --- |
| `session_search` | `features.session_search` | on | yes |
| `lsp_tools` | `features.lsp_tools` | off | yes |
| `web_fetch` | `features.web_fetch` | off | yes |
| `session_recap` | `features.session_recap` | on | yes |
| `ask_user_question` | `features.ask_user_question` | on | yes |
| `voice_mode` | `features.voice_mode` | on | yes |
| `write_file` | `features.write_file` | on | yes |
| `feedback` | `features.feedback` | on | yes |
| `feedback_trace_card` | `features.feedback_trace_card` | off | yes |
| `turn_summary` | `features.turn_summary` | on | yes |
| `cancel_rewind` | `features.cancel_rewind` | on | yes |
| `compaction_verbatim_input` | `features.compaction_verbatim_input` | on | yes |
| `two_pass_compaction` | `features.two_pass_compaction` | on | yes |
| `backend_tools` | `features.backend_tools` | on | no |
| `auto_wake` | `features.auto_wake` | on | yes |
| `subagent_worktree_snapshot` | `features.subagent_worktree_snapshot` | off | yes |
| `active_agent_messages` | `features.active_agent_messages` | off | yes |
| `repo_status_in_system_prompt` | `features.repo_status_in_system_prompt` | on | yes |
| `dock` | `features.dock` | off | yes |

To pin a feature for a machine, write it under `[features]` in `/etc/bcode/requirements.toml`:

```toml
[features]
dock = false
```
