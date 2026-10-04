# efr.plugin.zsh - talk to the eggfriedrice.code daemon from an interactive zsh.
#
#   , <prompt>     send a prompt with this shell's context (queues behind a running turn)
#   ,new [prompt]  start a new conversation for this terminal; without a prompt,
#                  the next `,` line starts it
#   ,! <text>      steer the running turn instead of queueing
#   Ctrl+Space     toggle sticky agent mode: every line goes to the agent, except
#                  lines that start with `!` (run as shell commands) or `,`; a line
#                  of just `,` leaves sticky mode
#
# The plugin only observes and relays. The daemon derives git root, scope and
# permissions, so nothing here runs git or forks per prompt.
#
# Written from scratch for this project (MIT); it is not derived from any terminal's
# shell integration.

[[ -o interactive ]] || return 0
# The daemon's own hidden shells read the user's .zshrc too; the plugin has no job
# there, and its hooks must stay out of the way of the hidden shell's integration.
[[ -n $EFR_HIDDEN_SHELL ]] && return 0

zmodload zsh/zleparameter 2>/dev/null
zmodload -F zsh/files b:zf_mv b:zf_rm 2>/dev/null
autoload -Uz add-zsh-hook

typeset -g _efr_sticky=0
# The last shell command line and its exit status, for the next `,` line, and the line
# that is running now. Declared without values so that re-sourcing keeps them.
typeset -g _efr_last_command _efr_last_command_status _efr_running
# 1 after a bare `,new`: the next `,` line starts a new conversation. A conversation
# exists only once it has a prompt, so a bare `,new` can only remember the wish.
typeset -gi _efr_new_pending
# Override before sourcing to change how sticky mode shows in the prompt.
: ${EFR_STICKY_INDICATOR:='%F{magenta}efr>%f '}

# --- helpers ----------------------------------------------------------------------

# whence is a builtin, so this costs no fork and sees a freshly installed binary.
_efr_available() {
  whence -p efr >/dev/null
}

_efr_missing() {
  print -u2 -- "efr: the efr binary is not on PATH; install it with 'just install'"
}

# Sets REPLY to $1 as a JSON string literal. Uses REPLY instead of command
# substitution so building the context forks nothing.
_efr_json_string() {
  emulate -L zsh
  local s=$1 code hex
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\n'/\\n}
  s=${s//$'\t'/\\t}
  s=${s//$'\r'/\\r}
  # JSON forbids raw control characters; escape whatever is left.
  for code in {1..31}; do
    if [[ $s == *${(#)code}* ]]; then
      printf -v hex '\\u%04x' $code
      s=${s//${(#)code}/$hex}
    fi
  done
  REPLY="\"$s\""
}

# Sets REPLY to $1 as a JSON string, or null when empty.
_efr_json_string_or_null() {
  if [[ -n $1 ]]; then _efr_json_string "$1"; else REPLY=null; fi
}

# Sets REPLY to $1 as a JSON integer, or null when it is not one.
_efr_json_int_or_null() {
  if [[ $1 == <-> ]]; then REPLY=$1; else REPLY=null; fi
}

# Sets REPLY to the ShellContext JSON object the daemon expects:
# {pwd, oldpwd, tty, shell_pid, last_status, shlvl, ssh_connection, hostname}.
# $1 is the exit status of the user's previous command, captured by the caller.
_efr_context_json() {
  emulate -L zsh
  local -a fields
  _efr_json_string "$PWD";                    fields+=("\"pwd\":$REPLY")
  _efr_json_string_or_null "$OLDPWD";         fields+=("\"oldpwd\":$REPLY")
  _efr_json_string_or_null "$TTY";            fields+=("\"tty\":$REPLY")
  fields+=("\"shell_pid\":$$")
  _efr_json_int_or_null "$1";                 fields+=("\"last_status\":$REPLY")
  _efr_json_int_or_null "$SHLVL";             fields+=("\"shlvl\":$REPLY")
  _efr_json_string_or_null "$SSH_CONNECTION"; fields+=("\"ssh_connection\":$REPLY")
  _efr_json_string "$HOST";                   fields+=("\"hostname\":$REPLY")
  REPLY="{${(j:,:)fields}}"
}

# True when $1 runs one of this plugin's commands (`,`, `,new`, `,!`), which is not a
# shell command worth reporting as the last command.
_efr_is_plugin_line() {
  emulate -L zsh -o extended_glob
  [[ $1 == [[:space:]]#,* ]]
}

# Sets REPLY to the line that runs for the line $1. A line that calls one of this
# plugin's commands comes back with its prompt quoted as one word, so zsh never reads
# the prompt as shell syntax: `?` and `*` as globs, `>` as a redirection, `;` or `|`
# as another command, an apostrophe as an unclosed quote, `!` as history. Any other
# line comes back as it is.
_efr_rewrite_line() {
  emulate -L zsh -o extended_glob
  REPLY=$1
  [[ $1 == (#b)([[:space:]]#)(,new|,!|,)([[:space:]]##(*)|) ]] || return 0
  # The leading blanks stay: with hist_ignore_space they keep the line out of history.
  local lead=$match[1] cmd=$match[2] rest=${match[4]%%[[:space:]]##}
  if [[ -z $rest ]]; then
    REPLY=$lead$cmd
    return 0
  fi
  # (q) leaves `!` alone, and history expansion still sees it in the accepted line.
  local quoted=${(q)rest}
  REPLY="$lead$cmd ${quoted//\!/\\!}"
}

# Sets reply to the efr arguments that carry the last shell command line. The line
# travels as its own argument, never inside the context JSON: it can hold a secret, and
# the daemon keeps the context in its event log.
_efr_last_command_args() {
  reply=()
  [[ -n $_efr_last_command ]] && reply=(--last-command "$_efr_last_command")
}

# --- commands ---------------------------------------------------------------------

function , {
  # Must be first: any other command would overwrite the status being reported.
  local last_status=$?
  emulate -L zsh
  _efr_available || { _efr_missing; return 127 }
  if (( $# == 0 )); then
    print -u2 -- "usage: , <prompt>   (Ctrl+Space toggles sticky agent mode)"
    return 2
  fi
  # After an earlier `,` line, $? is efr's own status; report the status of the shell
  # command that goes with the last command line instead.
  [[ -n $_efr_last_command ]] && last_status=$_efr_last_command_status
  _efr_context_json $last_status
  local context=$REPLY
  _efr_last_command_args
  if (( _efr_new_pending )); then
    _efr_new "$context" "$@"
  else
    efr send --context-json "$context" "${reply[@]}" -- "$@"
  fi
}

function ,new {
  local last_status=$?
  emulate -L zsh
  _efr_available || { _efr_missing; return 127 }
  if [[ -z ${*//[[:space:]]/} ]]; then
    _efr_new_pending=1
    print -u2 -- "efr: the next , line starts a new conversation"
    return 0
  fi
  [[ -n $_efr_last_command ]] && last_status=$_efr_last_command_status
  _efr_context_json $last_status
  local context=$REPLY
  _efr_last_command_args
  _efr_new "$context" "$@"
}

# Runs `efr new` with the context $1 and the prompt words after it; reply holds the
# last command arguments. A pending bare `,new` is settled unless efr refused the
# command line (2) or found no daemon (3): then no conversation started.
_efr_new() {
  local context=$1
  shift
  efr new --context-json "$context" "${reply[@]}" -- "$@"
  local code=$?
  (( code == 2 || code == 3 )) || _efr_new_pending=0
  return $code
}

# Steering goes through `efr send --steer`, the CLI side of turn.steer. A steer joins
# the running turn, whose context is already set, so it carries no last command.
function ,! {
  local last_status=$?
  emulate -L zsh
  _efr_available || { _efr_missing; return 127 }
  if (( $# == 0 )); then
    print -u2 -- "usage: ,! <text>   (steers the running turn)"
    return 2
  fi
  _efr_context_json $last_status
  efr send --steer --context-json "$REPLY" -- "$@"
}

# --- sticky agent mode ------------------------------------------------------------

# Themes that rebuild PROMPT on every precmd would drop the indicator, so it is
# reapplied from precmd as well as on toggle.
_efr_apply_indicator() {
  if (( _efr_sticky )); then
    [[ $PROMPT == "$EFR_STICKY_INDICATOR"* ]] || PROMPT="$EFR_STICKY_INDICATOR$PROMPT"
  elif [[ $PROMPT == "$EFR_STICKY_INDICATOR"* ]]; then
    PROMPT=${PROMPT#"$EFR_STICKY_INDICATOR"}
  fi
}

_efr_toggle_sticky() {
  if (( ! _efr_sticky )) && ! _efr_available; then
    zle -M "efr: not on PATH; sticky agent mode is unavailable"
    return 0
  fi
  (( _efr_sticky = ! _efr_sticky ))
  _efr_apply_indicator
  zle reset-prompt
}

# Wraps whatever accept-line was before (another plugin's widget or the builtin),
# so loading order with other plugins keeps working. Lines are rewritten rather than
# sent to efr directly, so each lands in history as the command that actually ran.
_efr_accept_line() {
  local line=$BUFFER
  if (( _efr_sticky )); then
    if [[ $line == ',' ]]; then
      _efr_sticky=0
      _efr_apply_indicator
      BUFFER=''
      zle reset-prompt
      return 0
    elif [[ $line == '!'* ]]; then
      # An escape hatch for one shell command without leaving sticky mode.
      BUFFER=${line#!}
    elif [[ -z ${line//[[:space:]]/} ]]; then
      : # an empty line runs as typed
    elif [[ $line == [[:space:]]#,* ]]; then
      _efr_rewrite_line "$line"
      BUFFER=$REPLY
    else
      BUFFER=", ${(q)line}"
    fi
  else
    _efr_rewrite_line "$line"
    BUFFER=$REPLY
  fi
  zle _efr_orig_accept_line
}

# --- daemon notices ---------------------------------------------------------------

# The daemon leaves notices (an approval waiting, a finished turn) in one file per
# terminal; the prompt shows them. Moving the file first means a notice written
# while printing lands in a new file instead of being lost.
_efr_print_notices() {
  [[ -n $TTY ]] || return 0
  local file="${XDG_RUNTIME_DIR:-/run/user/$UID}/efr/notices/${${TTY#/dev/}//\//-}"
  [[ -s $file ]] || return 0
  local shown="$file.shown.$$"
  zf_mv -f -- "$file" "$shown" 2>/dev/null || return 0
  print -r -- "$(<$shown)"
  zf_rm -f -- "$shown" 2>/dev/null
}

# --- last command -----------------------------------------------------------------

# preexec gets the line as typed, just before it runs.
_efr_preexec() {
  _efr_running=$1
}

# Keeps the line that just finished, with its status, unless it ran a plugin command.
_efr_remember_command() {
  if [[ -n $_efr_running ]] && ! _efr_is_plugin_line "$_efr_running"; then
    _efr_last_command=$_efr_running
    _efr_last_command_status=$1
  fi
  _efr_running=''
}

_efr_precmd() {
  # Must be first: the status of the line that just finished.
  local exit_status=$?
  _efr_remember_command $exit_status
  _efr_apply_indicator
  _efr_print_notices
}

# --- wiring -----------------------------------------------------------------------

# Re-sourcing must not wrap our own widget, which would recurse.
if (( ! ${+widgets[_efr_orig_accept_line]} )); then
  zle -A accept-line _efr_orig_accept_line
fi
zle -N accept-line _efr_accept_line
zle -N _efr_toggle_sticky
# Ctrl+Space sends NUL (^@) in common terminals.
bindkey -M emacs '^@' _efr_toggle_sticky
bindkey -M viins '^@' _efr_toggle_sticky
add-zsh-hook precmd _efr_precmd
add-zsh-hook preexec _efr_preexec

_efr_available || print -u2 -- "efr.plugin.zsh: efr is not on PATH; the , commands stay inactive until it is installed"
