# Sourced by the PR doctor workflow's credentialed steps (brwr-p2a7): gives
# git, in this step's shell only, the token in $DOCTOR_GIT_TOKEN for
# github.com, through GIT_CONFIG_* environment entries rather than any
# .git/config, so a later step (the merge and regeneration) never sees it.
# Appends to existing GIT_CONFIG_* entries instead of replacing them.
if [ -z "${DOCTOR_GIT_TOKEN:-}" ]; then
  echo "pr-doctor-git-auth: DOCTOR_GIT_TOKEN is empty" >&2
  return 1
fi
doctor_basic=$(printf 'x-access-token:%s' "$DOCTOR_GIT_TOKEN" | base64 | tr -d '\n')
echo "::add-mask::$doctor_basic"
doctor_n=${GIT_CONFIG_COUNT:-0}
export "GIT_CONFIG_KEY_$doctor_n=http.https://github.com/.extraheader"
export "GIT_CONFIG_VALUE_$doctor_n=AUTHORIZATION: basic $doctor_basic"
export GIT_CONFIG_COUNT=$((doctor_n + 1))
unset doctor_basic doctor_n
