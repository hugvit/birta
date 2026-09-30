#!/usr/bin/env bash
# Cold-agent eval: hands a fixture's pre-written review to `claude -p` and records what
# the agent did with it. It costs tokens, so it only runs when a person types `run` at a
# terminal; `--dry-run` prints the plan and calls nothing.
#
#   bash scripts/agent-eval/run.sh --dry-run
#   bash scripts/agent-eval/run.sh [--runs N]
#
# Env: BIRTA_BIN (default <repo>/target/debug/birta), CLAUDE_BIN (default: claude on
# PATH), BIRTA_EVAL_OUT (default <repo>/.context/output/agent-eval).
# Written for bash 3.2, the macOS /bin/bash.

set -u

FIXTURES="pr report questions"
DEFAULT_RUNS=3
MAX_RUNS=10
PROMPT='take a look at my notes on %s in birta'
TOOLS='Bash(birta:*) Read Edit'

usage() {
  echo "usage: run.sh [--dry-run] [--runs N]    N from 1 to $MAX_RUNS, default $DEFAULT_RUNS"
}

die() {
  echo "run.sh: $1" >&2
  exit "${2:-1}"
}

absolute() {
  case $1 in
    /*) printf '%s\n' "$1" ;;
    *) printf '%s/%s\n' "$PWD" "$1" ;;
  esac
}

# Appends one `## Run HH:MM:SS` section to a results file, creating it with its title if
# it doesn't exist. Never truncates a file that is already there.
append_results() {
  local file=$1 title=$2 body=$3
  if [ ! -e "$file" ]; then
    # noclobber: a file made meanwhile is appended to, not replaced
    (
      set -C
      printf '# %s\n\n%s\n' "$title" \
        "The agent is not cold: the user's global and project memory load as usual." \
        >"$file"
    ) 2>/dev/null
  fi
  printf '\n## Run %s\n\n%s\n' "$(date +%H:%M:%S)" "$body" >>"$file"
}

# The number after `"key":` in a small JSON file.
json_count() {
  grep -o "\"$2\"[[:space:]]*:[[:space:]]*[0-9]*" "$1" | grep -o '[0-9]*$' | head -n 1
}

# One agent run on a copy of fixture $1; prints a results table row.
run_one() {
  local name=$1 i=$2 tmp transcript code log claimed responded covers warned nh unasked
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/birta-eval.XXXXXX") || return 1
  mkdir "$tmp/bin" && ln -s "$birta" "$tmp/bin/birta" || return 1
  cp "$fixtures/$name.md" "$tmp/$name.md" || return 1
  cp "$fixtures/$name.handback.jsonl" "$tmp/$name.md.birta.jsonl" || return 1
  transcript="$out/$day-$name-$(date +%H%M%S)-$i.jsonl"
  (
    cd "$tmp" || exit 1
    # shellcheck disable=SC2086 # TOOLS is a list of tool names
    BIRTA_REVIEW=1 PATH="$tmp/bin:$PATH" "$claude" -p "$(printf "$PROMPT" "$name.md")" \
      --output-format stream-json --verbose --allowedTools $TOOLS \
      </dev/null >"$transcript" 2>"$tmp/stderr"
  )
  code=$?

  log="$tmp/$name.md.birta.jsonl"
  claimed=no
  grep '"op":"claim"' "$log" | grep -q '"type":"agent"' && claimed=yes
  responded=no
  grep '"type":"agent"' "$log" | grep -Eq '"op":"yield"|"late":true' && responded=yes
  covers=$(grep '"op":"covers"' "$log" |
    sed -e 's/.*"to":\([0-9]*\).*"quote":"\([^"]*\)".*/#\1 "\2"/' | tr '\n' ' ')
  # respond prints `warning: not handled: #N` and a `recorded: … N unasked` tally
  warned=$(grep -o 'not handled: #[0-9][0-9]*' "$transcript" | grep -o '#[0-9]*' | sort -u |
    tr '\n' ' ')
  if [ "$responded" = yes ]; then
    nh=$(printf '%s' "$warned" | wc -w | tr -d ' ')
  else
    nh="all (no respond)"
  fi
  unasked=$(grep -o '[0-9][0-9]* unasked' "$transcript" | awk '{ s += $1 } END { print s + 0 }')
  rm -rf -- "$tmp"
  printf '| %s | %s | %s | %s | %s | %s %s(expect %s) | %s (expect %s) | %s |\n' \
    "$i" "$code" "$claimed" "$responded" "${covers:--}" "$nh" "${warned:+[$warned] }" \
    "$(json_count "$fixtures/$name.expect.json" not_handled)" "$unasked" \
    "$(json_count "$fixtures/$name.expect.json" unasked)" "$(basename "$transcript")"
}

main() {
  local dry=0 runs=$DEFAULT_RUNS
  while [ $# -gt 0 ]; do
    case $1 in
      --dry-run) dry=1 ;;
      --runs)
        [ $# -ge 2 ] || { usage >&2; die "--runs needs a number" 2; }
        runs=$2
        shift
        ;;
      -h | --help)
        usage
        exit 0
        ;;
      *)
        usage >&2
        die "unknown argument: $1" 2
        ;;
    esac
    shift
  done
  case $runs in
    '' | *[!0-9]*) die "--runs takes a number from 1 to $MAX_RUNS" 2 ;;
  esac
  if [ "$runs" -lt 1 ] || [ "$runs" -gt "$MAX_RUNS" ]; then
    die "--runs takes a number from 1 to $MAX_RUNS" 2
  fi

  local here root
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd) || die "can't find the script's folder"
  fixtures="$here/fixtures"
  root=$(git -C "$here" rev-parse --show-toplevel 2>/dev/null) || root=$(cd "$here/../.." && pwd)

  birta=$(absolute "${BIRTA_BIN:-$root/target/debug/birta}")
  if [ -n "${CLAUDE_BIN+set}" ]; then
    case $CLAUDE_BIN in
      /*) ;;
      *) die "CLAUDE_BIN must be an absolute path: $CLAUDE_BIN" 2 ;;
    esac
    [ -f "$CLAUDE_BIN" ] && [ -x "$CLAUDE_BIN" ] || die "CLAUDE_BIN is not an executable file: $CLAUDE_BIN" 2
    claude=$CLAUDE_BIN
  else
    claude=$(type -P claude) || claude=
  fi
  local context_missing=0
  if [ -n "${BIRTA_EVAL_OUT+set}" ]; then
    out=$(absolute "$BIRTA_EVAL_OUT")
  else
    out="$root/.context/output/agent-eval"
    [ -d "$root/.context" ] || context_missing=1
  fi
  day=$(date +%Y-%m-%d)

  if [ "$dry" = 1 ]; then
    echo "agent eval: dry run, nothing is called or written"
    echo "repo:   $root"
    if [ -x "$birta" ]; then
      echo "birta:  $birta"
    else
      echo "birta:  $birta (missing: run cargo build first)"
    fi
    if [ -n "$claude" ]; then
      echo "claude: $claude"
    else
      echo "claude: not found (put claude on PATH or set CLAUDE_BIN)"
    fi
    echo "runs: $runs per fixture"
    echo "agent env: BIRTA_REVIEW=1 PATH=<tmp>/bin:\$PATH, with <tmp>/bin/birta -> $birta"
    echo "allowed tools: $TOOLS"
    local name
    for name in $FIXTURES; do
      echo
      echo "fixture: $fixtures/$name.md"
      echo "  copied with $name.handback.jsonl to <tmp>/$name.md and <tmp>/$name.md.birta.jsonl"
      printf '  cd <tmp> && BIRTA_REVIEW=1 %q -p "%s" --output-format stream-json --verbose --allowedTools %s\n' \
        "${claude:-claude}" "$(printf "$PROMPT" "$name.md")" "$TOOLS"
      echo "  results: $out/$day-$name.md"
    done
    if [ "$context_missing" = 1 ]; then
      echo
      echo "out: $root/.context does not exist; a run would refuse (set BIRTA_EVAL_OUT)"
    fi
    exit 0
  fi

  [ -z "${CI+set}" ] || die "CI is set: the eval never runs in CI"
  if [ -n "${CLAUDECODE+set}" ] || [ -n "${AI_AGENT+set}" ]; then
    die "CLAUDECODE or AI_AGENT is set: run it from your own terminal, not through an agent"
  fi
  [ -t 0 ] || die "needs a terminal: type run to confirm, a pipe can't"
  [ -n "$claude" ] || die "claude not found: put it on PATH or set CLAUDE_BIN"
  [ -x "$birta" ] || die "no birta at $birta: run cargo build first"
  [ "$context_missing" = 0 ] || die "$root/.context does not exist: set BIRTA_EVAL_OUT"

  local count
  count=$(($(echo $FIXTURES | wc -w) * runs))
  printf 'This calls %s %s times and costs tokens. Type run to go on: ' "$claude" "$count" >&2
  local ans=
  read -r ans || true
  [ "$ans" = run ] || die "not confirmed; nothing was called"

  mkdir -p "$out" || die "can't create $out"
  local name i body
  for name in $FIXTURES; do
    body="birta $(git -C "$root" rev-parse --short HEAD 2>/dev/null || echo '?'), claude $claude, $runs runs"
    body="$body

| run | exit | claimed | respond | covers | not handled | unasked | transcript |
|---|---|---|---|---|---|---|---|"
    i=1
    while [ "$i" -le "$runs" ]; do
      echo "run.sh: $name $i/$runs" >&2
      body="$body
$(run_one "$name" "$i")"
      i=$((i + 1))
    done
    append_results "$out/$day-$name.md" "Agent eval: $name" "$body"
    echo "run.sh: results in $out/$day-$name.md" >&2
  done
}

# Sourcing only defines the functions
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  main "$@"
fi
