# efr.plugin.zsh - talk to the eggfriedrice.code daemon from an interactive zsh.
#
#   , <prompt>     send a prompt with this shell's context (queues behind a running turn)
#   ,              a line of just `,` toggles sticky agent mode, as Ctrl+Space does
#   ,new [prompt]  start a new conversation for this terminal; without a prompt,
#                  the next `,` line starts it
#   ,! <text>      steer the running turn instead of queueing
#   ,mode [m]      this terminal's permission mode for its prompts: manual, cautious
#                  or auto; without a value, show it with its source and the
#                  choices; `default` lets the config decide again
#   ,model [id]    the same for the model (`efr models` lists them)
#   ,effort [e]    the same for the reasoning effort
#   ,<word> ...    a `,` word that names no plugin command and nothing that zsh
#                  could run is a prompt that starts with the word: `,run make`
#                  sends `run make`; a `,word` command of the user's own still runs
#   Ctrl+Space     toggle sticky agent mode: every line goes to the agent, except
#                  lines that start with `!` (run as shell commands) or `,` (the
#                  commands above), and a line of just `mode`, `model` or `effort`,
#                  alone or with one value that efr accepts, which runs as `,mode`,
#                  `,model` or `,effort`; a robot stands before the typed text while
#                  it is on (shown with PREDISPLAY, so the prompt itself never
#                  changes), after a dim tag with the terminal's own settings, such
#                  as `auto `
#   🤖 <prompt>    what a prompt line becomes on Enter in sticky mode: the robot is
#                  an alias of `,`, so the screen and history keep the line that ran,
#                  and the line still goes to the agent when history recalls it
#
# A prompt is never parsed as shell syntax, and the line stays exactly as typed on the
# screen and in history. The accept-line widget saves the prompt text, and `,`,
# `,new` and `,!` are aliases whose expansion ends in a comment marker, so zsh reads
# the rest of the line as a comment; the commands then take the saved text. The two
# options this needs (interactive_comments on, bang_hist off) hold for that one line
# only. A prompt that spans several lines falls back to a quoted rewrite, because a
# comment ends at the first newline.
#
# The plugin only observes and relays. The daemon derives git root, scope and
# permissions, so nothing here runs git or forks per prompt.
#
# What the user typed reaches efr in its environment, never in its arguments: the
# context as EFR_CONTEXT, the last command line as EFR_LAST_COMMAND and the prompt as
# EFR_PROMPT. Any local user can read a command line in /proc/<pid>/cmdline, while
# /proc/<pid>/environ is readable only by this user. The terminal's turn settings go
# the same way, as EFR_MODE, EFR_MODEL and EFR_EFFORT.
#
# Written from scratch for this project (MIT); it is not derived from any terminal's
# shell integration.

[[ -o interactive ]] || return 0
# The daemon's own hidden shells read the user's .zshrc too; the plugin has no job
# there, and its hooks must stay out of the way of the hidden shell's integration.
[[ -n $EFR_HIDDEN_SHELL ]] && return 0

zmodload zsh/parameter zsh/zleparameter 2>/dev/null
zmodload -F zsh/files b:zf_mv b:zf_rm 2>/dev/null
zmodload -F zsh/datetime p:EPOCHSECONDS 2>/dev/null
zmodload -F zsh/stat b:zstat 2>/dev/null
autoload -Uz add-zsh-hook

typeset -g _efr_sticky=0
# This terminal's turn settings, which every `,` and `,new` prompt hands to efr: the
# permission mode, the model and the reasoning effort. Empty means that the daemon's
# config decides. They start from EFR_MODE, EFR_MODEL and EFR_EFFORT; re-sourcing keeps
# what the terminal chose since.
(( ${+_efr_turn_mode} )) || typeset -g _efr_turn_mode=${EFR_MODE-}
(( ${+_efr_turn_model} )) || typeset -g _efr_turn_model=${EFR_MODEL-}
(( ${+_efr_turn_effort} )) || typeset -g _efr_turn_effort=${EFR_EFFORT-}
# The model ids for completion and when they were read, so a burst of Tab presses runs
# `efr models` once a minute at most.
typeset -ga _efr_model_names
typeset -gi _efr_model_names_at
# 1 once the completions are registered with compinit's compdef.
typeset -gi _efr_completion_ready
# 1 while the line being accepted is a prompt that sticky mode sends to the agent.
typeset -gi _efr_prompt_line
# Where the per-user runtime directories live; tests point it at a temporary tree.
typeset -g _efr_run_user=${_efr_run_user:-/run/user}
# The last shell command line and its exit status, for the next `,` line, and the line
# that is running now. Declared without values so that re-sourcing keeps them.
typeset -g _efr_last_command _efr_last_command_status _efr_running
# 1 after a bare `,new`: the next `,` line starts a new conversation. A conversation
# exists only once it has a prompt, so a bare `,new` can only remember the wish.
typeset -gi _efr_new_pending
# The prompt text the accept-line widget saved for the command of the line that runs
# now, and 1 while it is waiting there.
typeset -g _efr_stash
typeset -gi _efr_stash_set
# The user's own interactive_comments and bang_hist ("on" or "off") while a plugin
# line runs with its own settings, or empty.
typeset -ga _efr_saved_options
# Override before sourcing to change how sticky mode shows before the typed text:
# plain text, and a region_highlight style for it (a colour emoji keeps its colours).
# The robot needs a UTF-8 locale; any other locale gets plain text.
if [[ -o multibyte && ${LC_ALL:-${LC_CTYPE:-$LANG}} == *.[Uu][Tt][Ff](-|)8* ]]; then
  : ${EFR_STICKY_INDICATOR:='🤖 '}
else
  : ${EFR_STICKY_INDICATOR:='efr> '}
fi
: ${EFR_STICKY_STYLE:='fg=magenta'}
# The region_highlight style of the tag with the terminal's settings before the robot.
# zsh has no dim attribute; colour 8 is the grey that most themes use for it.
: ${EFR_TAG_STYLE:='fg=8'}
# What the sticky word (see _efr_sticky_word) expands to. The backslash keeps zsh from
# expanding the `,` alias inside it again, which would leave a second `#` among the
# words of a quoted prompt.
typeset -g _efr_sticky_alias='\, #'
# The indicator that versions before PREDISPLAY put in front of PROMPT, removed once
# when such a shell sources this version.
typeset -g _efr_old_prompt_indicator='%F{magenta}efr>%f '

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

# Sets REPLY to the sticky word: the indicator without its trailing blanks, such as
# the robot. A prompt line sent in sticky mode starts with it, where the indicator
# stood, and it is an alias of `,`. It must be one plain word that names nothing else;
# returns 1 when the indicator is no such word (`efr> ` holds a redirection), and a
# prompt line then starts with `, ` instead.
_efr_sticky_word() {
  emulate -L zsh -o extended_glob
  REPLY=${EFR_STICKY_INDICATOR%%[[:space:]]##}
  [[ -n $REPLY && $REPLY != -* ]] || return 1
  # What zsh reads as syntax in a command word, and the `,` of the plugin's commands.
  [[ $REPLY != *[[:space:]\\\'\"\`\$\;\&\|\<\>\(\)\[\]\{\}\*\?\~\^\#\=\!,%]* ]] || return 1
  [[ ${aliases[$REPLY]-$_efr_sticky_alias} == "$_efr_sticky_alias" ]] || return 1
  (( ! $+commands[$REPLY] && ! $+functions[$REPLY] && ! $+builtins[$REPLY] && ! ${reswords[(Ie)$REPLY]} ))
}

# Makes the sticky word an alias of `,`. Runs when the plugin is sourced and on every
# prompt line in sticky mode, so a new indicator works at once.
_efr_alias_sticky_word() {
  _efr_sticky_word || return 0
  [[ ${aliases[$REPLY]-} == "$_efr_sticky_alias" ]] || alias -- "$REPLY=$_efr_sticky_alias"
}

# Sets REPLY to a pattern for the first word of a plugin line: `,new`, `,!`, `,` or
# the sticky word.
_efr_command_pattern() {
  local word=
  _efr_sticky_word && word="|${(b)REPLY}"
  REPLY=",new|,!|,$word"
}

# True when $1 runs one of this plugin's commands (`,`, `,new`, `,!`, the sticky
# word), which is not a shell command worth reporting as the last command.
_efr_is_plugin_line() {
  emulate -L zsh -o extended_glob
  local REPLY
  [[ $1 == [[:space:]]#,* ]] && return 0
  _efr_sticky_word && [[ $1 == [[:space:]]#"$REPLY"([[:space:]]*|) ]]
}

# Sets REPLY to the line that runs for the line $1. A line that calls one of this
# plugin's commands comes back with its prompt quoted as one word, so zsh never reads
# the prompt as shell syntax: `?` and `*` as globs, `>` as a redirection, `;` or `|`
# as another command, an apostrophe as an unclosed quote, `!` as history. Any other
# line comes back as it is.
_efr_rewrite_line() {
  emulate -L zsh -o extended_glob
  _efr_command_pattern
  local commands=$REPLY
  REPLY=$1
  [[ $1 == (#b)([[:space:]]#)(${~commands})([[:space:]]##(*)|) ]] || return 0
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

# Runs efr with the arguments after the first three, and hands it the context JSON $1,
# the last command line $2, the prompt $3 and the terminal's turn settings in its
# environment. The last command travels on its own, never inside the context: it can
# hold a secret, and the daemon keeps the context in its event log. Prefix assignments
# set the variables for this one command, so they never stay in the shell, and an
# empty one hides a value that the shell may have exported.
_efr_call() {
  # NOTE: not named prompt, which is zsh's special parameter for PS1.
  local context=$1 last_command=$2 text=$3
  shift 3
  EFR_CONTEXT=$context EFR_LAST_COMMAND=$last_command EFR_PROMPT=$text \
    EFR_MODE=$_efr_turn_mode EFR_MODEL=$_efr_turn_model EFR_EFFORT=$_efr_turn_effort \
    efr "$@"
}

# Runs `efr settings` with the terminal's turn settings and the arguments "$@", such as
# `--mode=auto` for a value to check, and sets reply to the lines it printed. Returns
# efr's status; efr itself says on stderr what is wrong and lists the choices.
_efr_settings() {
  emulate -L zsh
  local out code
  out=$(EFR_MODE=$_efr_turn_mode EFR_MODEL=$_efr_turn_model EFR_EFFORT=$_efr_turn_effort \
    efr settings "$@")
  code=$?
  reply=(${(f)out})
  return $code
}

# Sets REPLY to the tag of the terminal's own turn settings, such as `auto gpt-5.4 `:
# each value that is set, then a blank. Empty when the config decides all of them.
_efr_tag() {
  emulate -L zsh
  # Unquoted, an empty value leaves no word.
  local -a set=($_efr_turn_mode $_efr_turn_model $_efr_turn_effort)
  REPLY=${(j: :)set}
  [[ -z $REPLY ]] || REPLY+=' '
}

# Sets REPLY to the prompt text of a plugin command called with the arguments "$@".
# After a typed line it is the text the accept-line widget saved, exactly as typed;
# the arguments are then only the comment marker that ended the alias. Called another
# way (from a script, or with aliases off), the words are the prompt, and a leading
# `#` left by the alias with interactive_comments off is dropped.
_efr_prompt_text() {
  emulate -L zsh
  if (( _efr_stash_set )); then
    REPLY=$_efr_stash
    _efr_stash=''
    _efr_stash_set=0
    return 0
  fi
  [[ $1 == '#' ]] && shift
  REPLY=${(j: :)@}
}

# Gives interactive_comments and bang_hist back their values from before a plugin
# line. Runs from preexec, once the line is parsed, and from precmd as a backstop.
_efr_restore_options() {
  (( ${#_efr_saved_options} == 2 )) || return 0
  if [[ $_efr_saved_options[1] == on ]]; then setopt interactive_comments; else unsetopt interactive_comments; fi
  if [[ $_efr_saved_options[2] == on ]]; then setopt bang_hist; else unsetopt bang_hist; fi
  _efr_saved_options=()
}

# --- commands ---------------------------------------------------------------------

function , {
  # Must be first: any other command would overwrite the status being reported.
  local last_status=$?
  emulate -L zsh
  _efr_prompt_text "$@"
  local text=$REPLY
  _efr_available || { _efr_missing; return 127 }
  if [[ -z ${text//[[:space:]]/} ]]; then
    print -u2 -- "usage: , <prompt>   (a line of just , or Ctrl+Space toggles sticky agent mode)"
    return 2
  fi
  # After an earlier `,` line, $? is efr's own status; report the status of the shell
  # command that goes with the last command line instead.
  [[ -n $_efr_last_command ]] && last_status=$_efr_last_command_status
  _efr_context_json $last_status
  local context=$REPLY
  if (( _efr_new_pending )); then
    _efr_new "$context" "$text"
  else
    _efr_call "$context" "$_efr_last_command" "$text" send
  fi
}

function ,new {
  local last_status=$?
  emulate -L zsh
  _efr_prompt_text "$@"
  local text=$REPLY
  _efr_available || { _efr_missing; return 127 }
  if [[ -z ${text//[[:space:]]/} ]]; then
    _efr_new_pending=1
    print -u2 -- "efr: the next , line starts a new conversation"
    return 0
  fi
  [[ -n $_efr_last_command ]] && last_status=$_efr_last_command_status
  _efr_context_json $last_status
  local context=$REPLY
  _efr_new "$context" "$text"
}

# Runs `efr new` with the context $1 and the prompt words after it. A pending bare
# `,new` is settled unless efr refused the command line (2) or found no daemon (3):
# then no conversation started.
_efr_new() {
  local context=$1
  shift
  _efr_call "$context" "$_efr_last_command" "${(j: :)@}" new
  local code=$?
  (( code == 2 || code == 3 )) || _efr_new_pending=0
  return $code
}

# Steering goes through `efr send --steer`, the CLI side of turn.steer. A steer joins
# the running turn, whose context is already set, so it carries no last command.
function ,! {
  local last_status=$?
  emulate -L zsh
  _efr_prompt_text "$@"
  local text=$REPLY
  _efr_available || { _efr_missing; return 127 }
  if [[ -z ${text//[[:space:]]/} ]]; then
    print -u2 -- "usage: ,! <text>   (steers the running turn)"
    return 2
  fi
  _efr_context_json $last_status
  # A running turn keeps its settings, so a steer hands none over. The locals hide the
  # terminal's values from _efr_call, which sees them through zsh's dynamic scope.
  local _efr_turn_mode= _efr_turn_model= _efr_turn_effort=
  _efr_call "$REPLY" '' "$text" send --steer
}

# --- turn settings ----------------------------------------------------------------

function ,mode {
  _efr_turn_setting mode "$@"
}

function ,model {
  _efr_turn_setting model "$@"
}

function ,effort {
  _efr_turn_setting effort "$@"
}

# The commands `,mode`, `,model` and `,effort`: $1 names the setting, and the word
# after it is the value. Without a value, prints the setting that a prompt from this
# terminal gets now, with its source and the choices. `default` clears the terminal's
# value, so the config decides again. Any other value is kept only when `efr settings`
# accepts it together with the terminal's other values; otherwise efr says why and
# lists the choices, and nothing changes.
_efr_turn_setting() {
  emulate -L zsh
  local name=$1 var=_efr_turn_$1
  shift
  _efr_available || { _efr_missing; return 127 }
  if (( $# > 1 )); then
    print -u2 -- "usage: ,$name [<$name>|default]"
    return 2
  fi
  local value=$1
  local -a reply
  if [[ $value == default ]]; then
    typeset -g -- "$var="
    _efr_settings || return
  elif [[ -n $value ]]; then
    # One word with `=`, so a value that starts with `-` stays a value for efr.
    _efr_settings "--$name=$value" || return
    typeset -g -- "$var=$value"
  else
    _efr_settings || return
  fi
  _efr_print_setting $name
}

# Prints the line of the setting $1 from reply, the output of `efr settings`. A value
# that this terminal set reached efr as a flag or a variable; the line says so in the
# terminal's words instead.
_efr_print_setting() {
  emulate -L zsh
  local name=$1 line
  for line in $reply; do
    [[ $line == "$name = "* ]] || continue
    if [[ -n ${(P)${:-_efr_turn_$name}} ]]; then
      line=${line/"  # --$name"/"  # this terminal (,$name)"}
      line=${line/"  # EFR_${(U)name}"/"  # this terminal (,$name)"}
    fi
    print -r -- "$line"
  done
}

# --- sticky agent mode ------------------------------------------------------------

# Shows the indicator before the typed text while sticky mode is on, after the dim tag
# of the terminal's own turn settings (see _efr_tag). With $1 `tag` only the tag stays,
# for a prompt line whose first word takes the indicator's place; with `hide`, or with
# sticky mode off, nothing shows. PREDISPLAY is not part of the buffer and leaves
# PROMPT alone, so it works with any prompt theme, including one whose prompt starts
# with a newline, and history never sees it. Only a command line gets it, never a
# continuation line or a value that vared edits. Runs from the line-init hook, on
# every toggle and when a line is accepted.
_efr_show_indicator() {
  # NOTE: zle gives region_highlight back with other offsets than it was given, so the
  # plugin's own entries are found by their memo, not by their text.
  region_highlight=(${region_highlight:#*memo=efr*})
  local -a mine
  if (( _efr_sticky )) && [[ $1 != hide && $CONTEXT == start ]]; then
    local REPLY tag
    _efr_tag
    tag=$REPLY
    PREDISPLAY=$tag
    [[ -n $tag ]] && mine+=("P0 ${#tag} $EFR_TAG_STYLE memo=efr")
    if [[ $1 != tag ]]; then
      PREDISPLAY+=$EFR_STICKY_INDICATOR
      mine+=("P${#tag} $(( ${#tag} + ${#EFR_STICKY_INDICATOR} )) $EFR_STICKY_STYLE memo=efr")
    fi
    region_highlight+=($mine)
  else
    PREDISPLAY=''
  fi
}

_efr_line_init() {
  _efr_show_indicator
}

# Turns sticky agent mode on (1) or off (0); turning it on fails while efr is missing.
_efr_set_sticky() {
  (( $1 )) && ! _efr_available && return 1
  _efr_sticky=$1
  _efr_show_indicator
  zle -R
}

_efr_toggle_sticky() {
  _efr_set_sticky $(( ! _efr_sticky )) ||
    zle -M "efr: not on PATH; sticky agent mode is unavailable"
  return 0
}

# True when $1 is a line of just `,`, which toggles sticky agent mode.
_efr_is_toggle_line() {
  emulate -L zsh -o extended_glob
  [[ $1 == [[:space:]]#,[[:space:]]# ]]
}

# Sets REPLY to the line that runs for the line $1 when $1 is a bare setting word in
# sticky mode: exactly `mode`, `model` or `effort`, alone or with one value, becomes
# `,mode`, `,model` or `,effort` with that value. A value counts only when it is
# `default` or `efr settings` accepts it with the terminal's other values, so a
# prompt that merely starts with the word (`model the database schema`, `effort
# matters`) still goes to the agent. Returns 1 for any other line.
_efr_setting_line() {
  emulate -L zsh -o extended_glob
  [[ $1 == (#b)[[:space:]]#(mode|model|effort)([[:space:]]##([^[:space:]]##)|)[[:space:]]# ]] ||
    return 1
  local name=$match[1] value=$match[3]
  if [[ -n $value && $value != default ]]; then
    _efr_available || return 1
    local -a reply
    _efr_settings "--$name=$value" >/dev/null 2>&1 || return 1
  fi
  REPLY=",$name${value:+ $value}"
}

# Sets REPLY to the line that runs for the accepted line $1. In sticky agent mode a
# line goes to the agent unless it is a plugin line (`,` and the commands above, or a
# line that history recalled with the sticky word in front), starts with `!` (an
# escape hatch for one shell command), is a bare setting word (see
# _efr_setting_line), or is empty. Such a prompt line gets the indicator's own text in
# front, so the line looks on the screen as it did while it was typed; when the
# indicator is no sticky word, it gets `, ` instead. Nothing else changes. Outside
# sticky mode every line runs as typed. Sets _efr_prompt_line to 1 for a prompt line
# and to 0 for any other.
_efr_line_to_run() {
  emulate -L zsh -o extended_glob
  local line=$1
  _efr_prompt_line=0
  if (( ! _efr_sticky )) || _efr_is_plugin_line "$line"; then
    REPLY=$line
  elif [[ $line == '!'* ]]; then
    REPLY=${line#!}
  elif [[ -z ${line//[[:space:]]/} ]]; then
    REPLY=$line
  elif _efr_setting_line "$line"; then
    :
  elif _efr_sticky_word; then
    local gap=${EFR_STICKY_INDICATOR#"$REPLY"}
    REPLY="$REPLY${gap:- }$line"
    _efr_prompt_line=1
  else
    REPLY=", $line"
    _efr_prompt_line=1
  fi
}

# Sets REPLY to the line $1 with a blank after its leading `,` when its first word is a
# `,` word that names nothing zsh could run: no plugin command, and no alias,
# function, builtin, reserved word or command of the user's. `,run sudo pacman -Syu`
# becomes the prompt line `, run sudo pacman -Syu`, so the word is part of the prompt
# instead of a `command not found`. Returns 1 for any other line, which stays as it is.
_efr_unknown_comma_line() {
  emulate -L zsh -o extended_glob
  [[ $1 == (#b)([[:space:]]#),([^[:space:]]##)(*) ]] || return 1
  # whence knows every kind of name that zsh runs, the plugin's own commands too.
  whence -- ",$match[2]" >/dev/null && return 1
  REPLY="$match[1], $match[2]$match[3]"
}

# For a one-line plugin line $1 (`,`, `,new`, `,!` or the sticky word, followed by a
# prompt): saves the prompt for the command and the user's two options for
# _efr_restore_options, and returns 0. The caller then sets the options, because an
# emulate here would undo them. Any other line returns 1 and changes nothing.
_efr_stash_line() {
  emulate -L zsh -o extended_glob
  [[ $1 == *$'\n'* ]] && return 1
  _efr_command_pattern
  [[ $1 == (#b)[[:space:]]#(${~REPLY})[[:space:]]##(*) ]] || return 1
  local rest=${match[2]%%[[:space:]]##}
  [[ -n $rest ]] || return 1
  _efr_stash=$rest
  _efr_stash_set=1
  _efr_saved_options=($options[interactivecomments] $options[banghist])
  return 0
}

# Wraps whatever accept-line was before (another plugin's widget or the builtin),
# so loading order with other plugins keeps working. The wrapped widget runs outside
# any emulate, with the user's own options.
_efr_accept_line() {
  # Only a command line is the plugin's: a continuation line (PS2) or a value that
  # vared edits is accepted as it is.
  if [[ $CONTEXT != start ]]; then
    zle _efr_orig_accept_line
    return
  fi
  # A line of just `,` toggles sticky agent mode, as Ctrl+Space does: nothing runs and
  # nothing lands in history. Without efr it runs, and `,` says what is missing.
  if _efr_is_toggle_line "$BUFFER" && _efr_set_sticky $(( ! _efr_sticky )); then
    BUFFER=''
    return 0
  fi
  if (( _efr_sticky )); then
    _efr_line_to_run "$BUFFER"
    [[ $REPLY == "$BUFFER" ]] || BUFFER=$REPLY
    # The line that runs takes the indicator's place on the screen: a prompt line
    # starts with the sticky word where the indicator stood, after the tag that stays,
    # so nothing moves, and a `!` line shows the command that runs.
    if (( _efr_prompt_line )); then _efr_show_indicator tag; else _efr_show_indicator hide; fi
    _efr_alias_sticky_word
  fi
  _efr_unknown_comma_line "$BUFFER" && BUFFER=$REPLY
  if _efr_stash_line "$BUFFER"; then
    # The line stays as typed. The alias ends in `#`, which starts a comment only with
    # interactive_comments on, and bang_hist off keeps `!` in the prompt literal.
    setopt interactive_comments
    unsetopt bang_hist
  elif _efr_is_plugin_line "$BUFFER"; then
    # A prompt over several lines, or a bare `,new`: a comment would end at the first
    # newline and leave the next lines to run as commands, so the prompt is quoted as
    # one word instead, and the alias's `#` must stay a plain word for this line.
    _efr_rewrite_line "$BUFFER"
    BUFFER=$REPLY
    _efr_saved_options=($options[interactivecomments] $options[banghist])
    unsetopt interactive_comments
  fi
  zle _efr_orig_accept_line
}

# --- daemon notices ---------------------------------------------------------------

# Sets REPLY to the daemon's runtime root by the one rule that efr_stdx's Dirs::runtime
# follows too: EFR_RUNTIME_DIR (which `just run` sets), else $EFR_HOME/runtime, else
# $XDG_RUNTIME_DIR/efr, else /run/user/$UID/efr when /run/user/$UID exists, belongs to
# this user and has mode 0700. An empty variable counts as unset, and so does an
# XDG_RUNTIME_DIR that is not absolute, as the XDG specification says. Returns 1 when
# there is no root: EFR_RUNTIME_DIR or EFR_HOME is not absolute, or nothing applies.
_efr_runtime_root() {
  emulate -L zsh
  if [[ -n $EFR_RUNTIME_DIR ]]; then
    REPLY=$EFR_RUNTIME_DIR
  elif [[ -n $EFR_HOME ]]; then
    REPLY=$EFR_HOME/runtime
  elif [[ $XDG_RUNTIME_DIR == /* ]]; then
    REPLY=$XDG_RUNTIME_DIR/efr
  else
    local dir=$_efr_run_user/$UID
    local -a mode
    [[ -d $dir && -O $dir ]] || return 1
    zstat -A mode +mode -- $dir 2>/dev/null || return 1
    (( (mode[1] & 8#7777) == 8#700 )) || return 1
    REPLY=$dir/efr
  fi
  [[ $REPLY == /* ]]
}

# The daemon leaves notices (an approval waiting, a finished turn) in one file per
# terminal under its runtime root; the prompt shows them. Moving the file first means
# a notice written while printing lands in a new file instead of being lost.
_efr_print_notices() {
  [[ -n $TTY ]] || return 0
  local REPLY
  _efr_runtime_root || return 0
  local file="$REPLY/notices/${${TTY#/dev/}//\//-}"
  [[ -s $file ]] || return 0
  local shown="$file.shown.$$"
  zf_mv -f -- "$file" "$shown" 2>/dev/null || return 0
  print -r -- "$(<$shown)"
  zf_rm -f -- "$shown" 2>/dev/null
}

# --- last command -----------------------------------------------------------------

# preexec gets the line as typed, just before it runs. The line is parsed by then, so
# a plugin line's own options can go.
_efr_preexec() {
  _efr_running=$1
  _efr_restore_options
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
  _efr_restore_options
  # A saved prompt that no command took (the line failed before it ran) must not
  # leak into the next one.
  _efr_stash=''
  _efr_stash_set=0
  _efr_remember_command $exit_status
  _efr_print_notices
  _efr_register_completion
}

# --- completion -------------------------------------------------------------------

# The arguments of `,mode`, `,model` and `,effort`: `default` and the names that efr
# accepts. Only the first argument completes.
_efr_complete_mode() {
  (( CURRENT == 2 )) || return 1
  compadd -- default manual cautious auto
}

_efr_complete_model() {
  (( CURRENT == 2 )) || return 1
  _efr_load_model_names
  compadd -- default $_efr_model_names
}

# The efforts that the terminal's model takes, from the choices `efr settings` lists.
_efr_complete_effort() {
  (( CURRENT == 2 )) || return 1
  local -a reply efforts
  local line
  _efr_settings 2>/dev/null
  line=${(M)reply:#effort = *}
  [[ $line == *'; choices: '* ]] && efforts=(${(s:, :)${line##*; choices: }})
  compadd -- default $efforts
}

# Reads the model ids with `efr models --names`, at most once a minute.
_efr_load_model_names() {
  (( ${#_efr_model_names} && EPOCHSECONDS - _efr_model_names_at < 60 )) && return 0
  _efr_model_names=(${(f)"$(efr models --names 2>/dev/null)"})
  _efr_model_names_at=$EPOCHSECONDS
}

# Registers the completions once compinit has defined compdef. A plugin is often
# sourced before compinit runs, so precmd tries again until it can.
_efr_register_completion() {
  (( _efr_completion_ready || ! $+functions[compdef] )) && return 0
  compdef _efr_complete_mode ,mode
  compdef _efr_complete_model ,model
  compdef _efr_complete_effort ,effort
  _efr_completion_ready=1
}

# --- wiring -----------------------------------------------------------------------

# Calls the builtin accept-line through its dot name, which no plugin can rebind.
_efr_builtin_accept_line() {
  zle .accept-line
}

# Saves the accept-line that was there before, so _efr_accept_line can call it. A user
# widget (another plugin's) is aliased. The builtin is NOT aliased: an alias of a
# builtin has type "builtin", and plugins such as zsh-autosuggestions rebind every
# builtin widget to a wrapper that calls `zle .<name>`, which exists only for real
# builtins, so Enter would fail with "No such widget". A user widget that calls
# `zle .accept-line` is wrapped correctly. Re-sourcing must not wrap our own widget
# (that would recurse), but it repairs an alias left by an older version.
case ${widgets[_efr_orig_accept_line]-} in
  '')
    if [[ ${widgets[accept-line]} == user:* ]]; then
      zle -A accept-line _efr_orig_accept_line
    else
      zle -N _efr_orig_accept_line _efr_builtin_accept_line
    fi
    ;;
  builtin)
    zle -N _efr_orig_accept_line _efr_builtin_accept_line
    ;;
esac
zle -N accept-line _efr_accept_line
zle -N _efr_toggle_sticky
autoload -Uz add-zle-hook-widget
add-zle-hook-widget line-init _efr_line_init
# Each alias ends in a comment marker; see the header. Recursion cannot happen: zsh does
# not expand an alias again inside its own expansion, so `,` reaches the function.
alias ,=', #' ,new=',new #'
alias ',!=,! #'
_efr_alias_sticky_word
# A shell that ran a version before PREDISPLAY still has its indicator in PROMPT.
[[ $PROMPT == "$_efr_old_prompt_indicator"* ]] && PROMPT=${PROMPT#"$_efr_old_prompt_indicator"}
# Ctrl+Space sends NUL (^@) in common terminals.
bindkey -M emacs '^@' _efr_toggle_sticky
bindkey -M viins '^@' _efr_toggle_sticky
add-zsh-hook precmd _efr_precmd
add-zsh-hook preexec _efr_preexec
_efr_register_completion

_efr_available || print -u2 -- "efr.plugin.zsh: efr is not on PATH; the , commands stay inactive until it is installed"
