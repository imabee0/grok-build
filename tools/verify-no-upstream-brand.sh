#!/usr/bin/env bash
# The zero-branding guarantee.
#
# Fails if any upstream identifier survives outside the two allowlisted places:
#   1. the provider registry / model catalog, where endpoints and wire model ids
#      are legitimately data (see the plan, "Branding vs. providers");
#   2. LICENSE / NOTICE / THIRD-PARTY-NOTICES, which Apache-2.0 s4(c)/(d)
#      requires a derivative work to retain.
#
# Every surviving identifier is also a coexistence bug: it is a path, env var or
# socket that bcode would then share with a real upstream install.
set -uo pipefail
cd "$(dirname "$0")/.."

# Case-SENSITIVE, listing exactly the casings tools/rebrand.toml rewrites, so the
# gate and the codemod agree by construction. A case-insensitive `xai` would also
# match base64 JWT fixtures (eyJ0eXAi...), which are not branding.
PATTERN='grok|Grok|GROK|xai|Xai|XAI|xAI|x\.ai|X\.AI|SpaceXAI|spacexai'

# Exemptions. Nothing here is compiled into the binary or shown to a user.
ALLOW=(
  # Apache-2.0 s4(c)/(d): a derivative work must retain these.
  ':(exclude)LICENSE'
  ':(exclude)NOTICE'
  ':(exclude)THIRD-PARTY-NOTICES'
  # Provider registry / model catalog: endpoints and wire model ids are data.
  # Delete a provider's rows and the tree has literal zero occurrences.
  ':(exclude)crates/codegen/bcode-models/default_models.json'
  # The fork's own machinery: rename rules must name what they replace, and the
  # coexistence test must name the CLIs whose directories it protects.
  ':(exclude)tools/'
)

# Only tracked files, so build artefacts and vendored caches cannot fail the gate.
hits=$(git grep -I -n -E "$PATTERN" -- . "${ALLOW[@]}" 2>/dev/null || true)

# Path names carry the brand too, and git grep only searches contents.
path_hits=$(git ls-files | grep -E "$PATTERN" || true)

status=0
if [ -n "$hits" ]; then
  echo "brand gate: FAIL - upstream identifiers in file contents:"
  echo "$hits" | head -50
  echo "... $(echo "$hits" | wc -l) total"
  status=1
fi
if [ -n "$path_hits" ]; then
  echo "brand gate: FAIL - upstream identifiers in path names:"
  echo "$path_hits" | head -50
  echo "... $(echo "$path_hits" | wc -l) total"
  status=1
fi

if [ "$status" -eq 0 ]; then
  echo "brand gate: clean"
fi
exit "$status"
