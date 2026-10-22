# efr.plugin.zsh - talk to the eggfriedrice.code daemon from an interactive zsh.
#
#   , <prompt>     send a prompt with this shell's context (queues behind a running turn).
#                  It goes to this terminal's conversation of the scope where you send
#                  it: the registered project that holds $PWD (also below its root), or
#                  the one conversation for every place outside projects. The
#                  conversation changes when you send a prompt, not when you cd, and a
#                  dim line before the reply says when it changes. A paste of several
#                  lines that all start with `,` is one prompt for each line: the first
#                  runs, and the others wait behind it in their order, as Tab in the
#                  input row queues them. A paste with any other line is one prompt
#   ,              a line of just `,` toggles sticky agent mode, as Ctrl+Space does
#   ,new [prompt]  start a new conversation for the scope where you are; this
#                  terminal's other scopes keep theirs. Without a prompt, the next `,`
#                  line starts it
#   ,! <text>      steer the running turn instead of queueing (from another terminal;
#                  the input row below steers from this one)
#   ,compact [focus]
#                  make room in the model's context of this terminal's conversation
#                  now: efrd writes a summary of the earlier turns, keeps the newest
#                  ones word for word and starts no turn; the focus says what the
#                  summary must keep. While a turn runs, efrd refuses it: the turn
#                  compacts on its own when its context is full
#   ,context [id]  show what fills the model's context of this terminal's
#                  conversation: the model's last count, efr's estimate of the next
#                  request by part, the largest items, the compactions and the cache
#                  use; it shows no text of a message and starts no turn. The words
#                  go to `efr context` as they are, such as a conversation id or
#                  --json
#   ,resume [N]    find a conversation to continue: with fzf 0.46 or newer, a live
#                  search picker below the command line (type to search the titles
#                  and the text, Enter continues one here, Ctrl-O prints it, Esc
#                  closes); without fzf, the recent conversations, the ones of the
#                  current project first, each with its number, title, project, age
#                  and turns. `,resume N` (or an id, or 4 characters of one) makes that
#                  one this terminal's conversation for its project again: the next
#                  `,` prompt there continues it, its hidden shell starts in its last
#                  directory and its jobs report here. When another open terminal
#                  holds it, efr asks first; `,resume --take N` moves it with no
#                  question. `,resume <words>` searches: the picker opens with the
#                  words, or without fzf the best matches print. It hands over the
#                  shell context and the words, but no prompt and no turn settings
#   ,away          tell efr that you are away from this terminal's conversation: a
#                  question then waits only a short time (presence.away_wait_secs),
#                  and its call does not run; your next input there ends it
#   ,shell [id]    answer or stop a command that still runs in the hidden shell of
#                  this terminal's conversation, such as an rsync that waits for a
#                  password after its call timed out: it shows the command and the
#                  line that it waits at; Enter answers, Ctrl+C stops it, a second
#                  Ctrl+C closes the hidden shell, Esc leaves it. Type such a
#                  password only here, never at the prompt. It also runs in sticky
#                  mode
#   ,mode [m]      this terminal's permission mode for its prompts: manual, cautious,
#                  auto or full; without a value, show it with its source and the
#                  choices; `default` lets the config decide again. `,mode full`
#                  works only when efr's config sets permissions.allow_full = true:
#                  commands then run with your rights, with no sandbox and no
#                  questions, and efr says what it still reads and asks. While a
#                  terminal's prompts run in full mode (its own value, or the
#                  config's default when it has none), sticky mode shows the mark
#                  EFR_FULL_MARK (🔓) next to the robot; outside sticky mode the
#                  turn's red first line says it. EFR_MODE=full from the
#                  environment is ignored when the plugin loads: only `,mode full`
#                  makes a terminal full
#   ,model [id]    the same for the model (`efr models` lists them)
#   ,effort [e]    the same for the reasoning effort
#   ,<word> ...    a `,` word that names no plugin command and nothing that zsh
#                  could run is a prompt that starts with the word: `,run make`
#                  sends `run make`; a `,word` command of the user's own still runs,
#                  as does a line that defines one (`,mine() { ... }`),
#                  `,!word` steers with `word`, and a typo of a plugin command such
#                  as `,moed`, `,compcat`, `,contxet`, `,resmue`, `,aawy` or `,shlel` stays on
#                  the line with a hint instead of running
#   , <word>       a `,` prompt that is one command word alone runs that command:
#                  `, resume` runs `,resume`, and so do `new`, `compact`, `context`,
#                  `away` and `shell`; `mode`, `model` and `effort` may have one value
#                  that efr accepts (`, mode auto`). A prompt with any other word, such
#                  as `, resume the build`, stays a prompt
#   Ctrl+Space     toggle sticky agent mode: every line goes to the agent, except
#                  lines that start with `!` (run as shell commands) or `,` (the
#                  commands above), and a line of just a command word, as after
#                  `, ` above (`resume`, or `mode` with one value that efr accepts),
#                  which runs as its `,` command; a robot and the scope of the next prompt
#                  stand before the typed text while it is on, such as `🤖 app › `
#                  in a registered project and `🤖 ○ › ` outside projects (shown
#                  with PREDISPLAY, so the prompt itself never changes), after a dim
#                  tag with the terminal's own settings, such as `auto `; in full
#                  mode the mark 🔓 stands next to the robot: `🤖🔓 app › `
#   🤖 <prompt>    what a prompt line becomes on Enter in sticky mode: the robot is
#                  an alias of `,`, so the screen and history keep the line that ran,
#                  and the line still goes to the agent when history recalls it
#
# The scope of the next prompt shows before you send it: in sticky mode, and through
# efr_prompt_info, a function that prints `app` or `○` (with the full mark in full
# mode) for your own prompt (`setopt prompt_subst; RPROMPT='$(efr_prompt_info)'`), and
# the variable $efr_scope, which precmd sets, for a prompt with no subshell
# (`RPROMPT='${efr_scope}'`). Both double each `%` of a project's name, so a prompt
# shows `50%` as `50%` and not as a prompt escape. The plugin never changes PROMPT or
# RPROMPT itself. It knows the scope with no fork and no git: efrd writes the
# registered projects to projects.tsv in its runtime root, one `<root><tab><name>`
# line each, and the plugin reads the file again only when it changed, then matches
# $PWD and ${PWD:A} by whole components, the deepest root first. Before efrd ever ran
# there is no file and no scope. The head line of efr's reply names the scope that
# efrd routed by.
#
# A prompt is never parsed as shell syntax, and the line stays as typed on the screen
# and in history, except that a sticky prompt gets the robot in front and an unknown
# `,word` line gets a blank after its `,` (`,!word` one after its `,!`). The accept-line widget saves the prompt text, and `,`,
# `,new`, `,!` and `,compact` are aliases whose expansion ends in a comment marker, so
# zsh reads the rest of the line as a comment; the commands then take the saved text. The two
# options this needs (interactive_comments on, bang_hist off) hold for that one line
# only. A comment ends at the first newline, so a prompt that spans several lines runs
# as its first line alone, and the screen shows the other lines after it (POSTDISPLAY),
# as they were typed or pasted. History keeps the whole text, and Enter on it sends the
# same prompts again.
#
# The plugin only observes and relays. The daemon derives git root, scope and
# permissions, so nothing here runs git or forks per prompt.
#
# What the user typed reaches efr in its environment, never in its arguments: the
# context as EFR_CONTEXT, the last command line as EFR_LAST_COMMAND, the terminal's
# recent command lines as EFR_RECENT_COMMANDS, the prompt as EFR_PROMPT (the focus
# of `,compact` too), the prompts that a paste queues behind it as EFR_QUEUED_PROMPTS
# (one per line) and the words of `,resume` as EFR_RESUME_WORDS. Any local user can read a command line in
# /proc/<pid>/cmdline, while /proc/<pid>/environ is readable only by this user. The
# terminal's turn settings go
# the same way, as EFR_MODE, EFR_MODEL and EFR_EFFORT.
#
# While efr follows a turn at a terminal, the keys go to an input row below the turn
# (render.turn_input in efr's config): Enter steers the turn, Tab queues a prompt
# behind it, Esc interrupts it and Alt+Up takes back the newest prompt queued there.
# Text that is still in the row when efr ends comes back to the command line as
# `, <text>`: the plugin names a file in its runtime directory in EFR_DRAFT_FILE, efr
# writes the text there (mode 0600), and precmd reads the file, removes it and puts the
# text in the line editor with print -z. A prompt of several lines stays one prompt.
#
# When it loads at a terminal, the plugin asks the terminal once for its background
# colour (OSC 11) and exports EFR_TERMINAL_BG, dark or light, for render.theme = "auto"
# in efr's config. efr itself never asks the terminal anything.
#
# Written from scratch for this project (MIT); it is not derived from any terminal's
# shell integration.

[[ -o interactive ]] || return 0
# The daemon's own hidden shells read the user's .zshrc too; the plugin has no job
# there, and its hooks must stay out of the way of the hidden shell's integration.
[[ -n $EFR_HIDDEN_SHELL ]] && return 0

zmodload zsh/parameter zsh/zleparameter 2>/dev/null
zmodload -F zsh/files b:zf_mv b:zf_rm 2>/dev/null
zmodload -F zsh/datetime p:EPOCHSECONDS p:EPOCHREALTIME 2>/dev/null
zmodload -F zsh/stat b:zstat 2>/dev/null
zmodload -F zsh/system p:sysparams 2>/dev/null
autoload -Uz add-zsh-hook

typeset -g _efr_sticky=0
# This terminal's turn settings, which every `,` and `,new` prompt hands to efr: the
# permission mode, the model and the reasoning effort. Empty means that the daemon's
# config decides. They start from EFR_MODE, EFR_MODEL and EFR_EFFORT; re-sourcing keeps
# what the terminal chose since.
# Full mode is never inherited: a terminal becomes full only by `,mode full`, which
# checks the gate and says what full mode does.
if (( ! ${+_efr_turn_mode} )); then
  if [[ ${EFR_MODE-} == full ]]; then
    typeset -g _efr_turn_mode=
    print -u2 -- "efr: EFR_MODE=full is ignored here; use ,mode full"
  else
    typeset -g _efr_turn_mode=${EFR_MODE-}
  fi
fi
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
# Where the line that is running now started, and when (EPOCHREALTIME).
typeset -g _efr_running_cwd _efr_running_start
# The terminal's recent command lines for the next `,` line, oldest first, at most
# _efr_recent_max of them: each one a JSON object with its command, status, directory,
# end (EPOCHSECONDS) and duration in milliseconds, built once when it ends. A `,` line,
# an efr command and a line that starts with a blank (HIST_IGNORE_SPACE's rule) never
# go in. Declared without values so that re-sourcing keeps them.
typeset -ga _efr_recent
typeset -gi _efr_recent_max=10
# The most characters of a command line that the last command and each recent one keep.
# efrd shows only the first 400, and Linux refuses to start a program with one variable
# of more than 128 KiB.
typeset -gi _efr_command_max=2000
# The most bytes of the recent commands together, as the JSON that efr gets. The oldest
# go first.
typeset -gi _efr_recent_max_bytes=65536
# 1 after a bare `,new`: the next `,` line starts a new conversation. A conversation
# exists only once it has a prompt, so a bare `,new` can only remember the wish.
typeset -gi _efr_new_pending
# The prompt text the accept-line widget saved for the command of the line that runs
# now, and 1 while it is waiting there.
typeset -g _efr_stash
typeset -gi _efr_stash_set
# The prompts that a paste of `,` lines queues behind the first one, oldest first, for
# the command of the line that runs now (see _efr_split_line).
typeset -ga _efr_queue
# The whole text of a line of several lines whose first line alone runs, for history
# to keep instead of that first line (see _efr_add_history), or empty.
typeset -g _efr_history_line
# 1 after a line ran efr for a prompt, until the next precmd shows the cursor again.
typeset -gi _efr_called
# The user's own interactive_comments and bang_hist ("on" or "off") while a plugin
# line runs with its own settings, or empty.
typeset -ga _efr_saved_options
# Override before sourcing to change how sticky mode shows before the typed text:
# plain text, and a region_highlight style for it (a colour emoji keeps its colours).
# The robot needs a UTF-8 locale; any other locale gets plain text.
# EFR_FULL_MARK is the mark next to the robot while the terminal's prompts run in full
# mode; an empty value shows none. The marks of the scope (`○` outside projects, `›`
# after the scope) have plain-text forms too. `○` has the "ambiguous" East Asian width
# and `›` the "neutral" one. A CJK locale keeps them: glibc's wcwidth, which zle uses,
# gives both one column in every UTF-8 locale, and Ghostty draws both one column wide
# whatever the locale (efr-screen-ghostty's widths test).
if [[ -o multibyte && ${LC_ALL:-${LC_CTYPE:-$LANG}} == *.[Uu][Tt][Ff](-|)8* ]]; then
  : ${EFR_STICKY_INDICATOR:='🤖 '}
  : ${EFR_FULL_MARK='🔓'}
  typeset -g _efr_outside_mark='○' _efr_scope_end='›'
else
  : ${EFR_STICKY_INDICATOR:='efr> '}
  : ${EFR_FULL_MARK='!'}
  typeset -g _efr_outside_mark='o' _efr_scope_end='>'
fi
: ${EFR_STICKY_STYLE:='fg=magenta'}
# The region_highlight style of the tag with the terminal's settings before the robot.
# zsh has no dim attribute; colour 8 is the grey that most themes use for it.
: ${EFR_TAG_STYLE:='fg=8'}
# The region_highlight style of the scope after the robot: the same muted grey.
: ${EFR_SCOPE_STYLE:='fg=8'}
# The scope of the next prompt for your own prompt (see efr_prompt_info): the name of
# the registered project that holds $PWD, the outside mark (`○`) anywhere else, or
# empty while efrd never wrote its projects file. precmd sets it. Each `%` of the name
# is doubled, because a prompt reads `%` as the start of an escape.
typeset -g efr_scope=
# The same scope as efr_scope with its `%` as typed, for the marker in PREDISPLAY,
# which zle shows as plain text.
typeset -g _efr_scope_text=
# The projects file's stamp that _efr_read_projects read last, and its projects: their
# roots and names in the order of the file, the deepest root first.
typeset -g _efr_projects_seen
typeset -ga _efr_project_roots _efr_project_names
# The config file and its stamp that _efr_config_full read last, and whether it made
# full mode the default.
typeset -g _efr_config_seen
typeset -gi _efr_config_full_mode
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

# Sets REPLY to a pattern for the first word of a plugin line: `,new`, `,compact`,
# `,!`, `,` or the sticky word.
_efr_command_pattern() {
  local word=
  _efr_sticky_word && word="|${(b)REPLY}"
  REPLY=",new|,compact|,!|,$word"
}

# True when $1 runs one of this plugin's commands (`,`, `,new`, `,!`, `,compact`, the
# sticky word), which is not a shell command worth reporting as the last command.
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
# the last command line $2, the prompt $3, the prompts to queue behind it (see
# _efr_queue) one per line, the terminal's recent command lines (see
# _efr_recent) as a JSON array, the terminal's turn settings and the file for the text
# of the input row (see _efr_draft_file) in its environment. The last command and the
# recent ones travel on their own, never inside the context: they can hold a secret,
# and the daemon keeps the context in its event log. Prefix assignments set the variables for
# this one command, so they never stay in the shell, and an empty one hides a value
# that the shell may have exported.
_efr_call() {
  # NOTE: not named prompt, which is zsh's special parameter for PS1.
  local context=$1 last_command=$2 text=$3
  shift 3
  local REPLY draft= recent= queued=${(pj:\n:)_efr_queue}
  _efr_queue=()
  _efr_draft_file && draft=$REPLY
  _efr_recent_json && recent=$REPLY
  _efr_called=1
  EFR_CONTEXT=$context EFR_LAST_COMMAND=$last_command EFR_RECENT_COMMANDS=$recent \
    EFR_PROMPT=$text EFR_QUEUED_PROMPTS=$queued \
    EFR_MODE=$_efr_turn_mode EFR_MODEL=$_efr_turn_model EFR_EFFORT=$_efr_turn_effort \
    EFR_DRAFT_FILE=$draft efr "$@"
}

# Sets REPLY to the recent commands (see _efr_recent) as one JSON array, or to an empty
# string when there is none. While the text has more than _efr_recent_max_bytes bytes,
# the oldest entries go.
_efr_recent_json() {
  # NOTE: without multibyte, ${#REPLY} counts bytes, which is what Linux limits.
  emulate -L zsh +o multibyte
  local -a entries=("${(@)_efr_recent}")
  REPLY=
  (( $#entries )) || return 0
  REPLY="[${(j:,:)entries}]"
  while (( ${#REPLY} > _efr_recent_max_bytes && $#entries > 1 )); do
    shift entries
    REPLY="[${(j:,:)entries}]"
  done
  (( ${#REPLY} <= _efr_recent_max_bytes )) || REPLY=
  return 0
}

# The pid namespace of this shell, by the inode of its link in /proc; empty when it
# cannot be read. A shell in a container (distrobox, toolbox) shares the runtime dir
# with the host but has pids of its own.
typeset -g _efr_pid_ns=
() {
  local -a ino
  zstat -A ino +inode /proc/self/ns/pid 2>/dev/null && _efr_pid_ns=$ino[1]
}

# The name of this shell's draft files: the pid, the time the plugin loaded and the
# pid namespace, so a later shell that gets the same pid never takes a file of this
# one. Sourcing the plugin again in the same shell keeps the name, so a run that
# started before still leaves its file where the prompt looks.
if [[ ${_efr_draft_name-} != "$$".* ]]; then
  typeset -g _efr_draft_name="$$.${${EPOCHREALTIME-}/./}"
  [[ $_efr_draft_name == *. ]] && _efr_draft_name+="$RANDOM$RANDOM"
  _efr_draft_name+=".${_efr_pid_ns:-0}"
  typeset -gi _efr_draft_count=0
fi

# Sets REPLY to the file where efr leaves the text that was still in the input row of a
# turn when it ended: a new file for each efr run, named after this shell, in the drafts
# directory of the runtime root (see _efr_runtime_root), which efr creates with mode
# 0700. Two runs before the next prompt do not overwrite each other: a run in a
# subshell, such as in a pipeline, counts in a copy of the count, so its name also
# holds the pid of that subshell. Returns 1 without a root.
_efr_draft_file() {
  _efr_runtime_root || return 1
  REPLY=$REPLY/drafts/$_efr_draft_name.$(( ++_efr_draft_count ))
  [[ -n ${sysparams[pid]-} && ${sysparams[pid]} != $$ ]] && REPLY+=.${sysparams[pid]}
  return 0
}

# Puts the text that efr handed back on the command line as a prompt: `, ` and the
# texts of this shell's files, oldest first, which may span several lines. Each file
# goes once it is read, so the text comes back once. $(<file) is read by zsh itself,
# without a fork.
_efr_take_draft() {
  emulate -L zsh
  local REPLY
  _efr_runtime_root || return 0
  local file text
  local -a texts
  for file in $REPLY/drafts/$_efr_draft_name.*(N.non); do
    text=$(<$file)
    zf_rm -f -- $file 2>/dev/null
    [[ -n ${text//[[:space:]]/} ]] && texts+=($text)
  done
  (( $#texts )) || return 0
  print -rz -- ", ${(pj:\n:)texts}"
}

# Removes the draft files of shells that are gone, such as a terminal that closed while
# efr followed a turn: no prompt of theirs comes again to take them. Only the files of
# shells in this pid namespace: kill cannot see a shell of another one, which may still
# wait for its file.
_efr_clean_drafts() {
  emulate -L zsh
  local REPLY file
  local -a parts
  [[ -n $_efr_pid_ns ]] || return 0
  _efr_runtime_root || return 0
  for file in $REPLY/drafts/*(N.); do
    parts=(${(s:.:)file:t})
    (( $#parts >= 4 )) && [[ $parts[1] == <-> && $parts[1] != $$ ]] || continue
    [[ $parts[3] == $_efr_pid_ns ]] || continue
    kill -0 $parts[1] 2>/dev/null || zf_rm -f -- $file 2>/dev/null
  done
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
# each value that is set, then a blank. Empty when the config decides all of them. Full
# mode is no word of the tag: the full mark next to the robot shows it.
_efr_tag() {
  emulate -L zsh
  local mode=$_efr_turn_mode
  [[ $mode == full ]] && mode=
  # Unquoted, an empty value leaves no word.
  local -a set=($mode $_efr_turn_model $_efr_turn_effort)
  REPLY=${(j: :)set}
  [[ -z $REPLY ]] || REPLY+=' '
}

# True when this terminal's prompts run in full mode: its own value is `full`, or it
# has none and the config's default is full (see _efr_config_full).
_efr_full_mode() {
  [[ $_efr_turn_mode == full ]] && return 0
  [[ -z $_efr_turn_mode ]] && _efr_config_full
}

# True when efr's config.toml makes full mode the default of a prompt that names no
# mode: `mode = "full"` and `allow_full = true` in its [permissions] table (or as the
# dotted keys permissions.mode and permissions.allow_full). The file is found by the
# rule of efr's config root: EFR_CONFIG_DIR, else $EFR_HOME/config, else
# $XDG_CONFIG_HOME/efr, else ~/.config/efr. It is read again only when its time or its
# size changed. The answer only picks the tag; efrd decides the mode of each turn.
_efr_config_full() {
  emulate -L zsh -o extended_glob
  local file
  if [[ -n $EFR_CONFIG_DIR ]]; then
    file=$EFR_CONFIG_DIR/config.toml
  elif [[ -n $EFR_HOME ]]; then
    file=$EFR_HOME/config/config.toml
  elif [[ $XDG_CONFIG_HOME == /* ]]; then
    file=$XDG_CONFIG_HOME/efr/config.toml
  else
    file=$HOME/.config/efr/config.toml
  fi
  local -A stat
  local stamp=$file
  zstat -H stat -- $file 2>/dev/null && stamp+=":$stat[mtime]:$stat[size]"
  if [[ $stamp != "$_efr_config_seen" ]]; then
    _efr_config_seen=$stamp
    _efr_config_full_mode=0
    local line table= key value mode= allow=
    if [[ -f $file && -r $file ]]; then
      for line in "${(@f)$(<$file)}"; do
        # Neither key takes a `#` or a blank in its value, so both can go.
        line=${${line%%\#*}//[[:space:]]/}
        case $line in
          (\[\[*) table='[]' ;;
          (\[*\]) table=${line[2,-2]} ;;
          (*=*)
            key=${line%%=*}
            value=${${line#*=}//[\"\']/}
            [[ -n $table ]] && key=$table.$key
            case $key in
              (permissions.mode) mode=$value ;;
              (permissions.allow_full) allow=$value ;;
            esac
            ;;
        esac
      done
    fi
    [[ $mode == full && $allow == true ]] && _efr_config_full_mode=1
  fi
  (( _efr_config_full_mode ))
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
    print -u2 -- "efr: the next , line starts a new conversation for its scope"
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
# the running turn, whose context is already set, so it carries no last command and no
# recent commands.
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
  local -a _efr_recent=() _efr_queue=()
  _efr_call "$REPLY" '' "$text" send --steer
}

# Makes room in the model's context of this terminal's conversation now, through
# `efr compact`: efrd prunes old tool output and writes a summary of the earlier turns.
# The words are the focus, which says what the summary must keep, and reach efr as
# EFR_PROMPT, as a prompt does. It starts no turn, so it hands over no last command, no
# recent commands and none of the terminal's turn settings: the compaction uses the
# conversation's model.
function ,compact {
  emulate -L zsh
  _efr_prompt_text "$@"
  local text=$REPLY
  _efr_available || { _efr_missing; return 127 }
  _efr_context_json ''
  # The locals hide the terminal's values from _efr_call, as for `,!`.
  local _efr_turn_mode= _efr_turn_model= _efr_turn_effort=
  local -a _efr_recent=() _efr_queue=()
  _efr_call "$REPLY" '' "$text" compact
}

# Shows what fills the model's context of this terminal's conversation, through `efr
# context`. It starts no turn, so it hands nothing over: no prompt, no context and no
# turn settings. The words are arguments of `efr context`, such as a conversation id,
# --model or --json, not a prompt.
function ,context {
  emulate -L zsh
  _efr_available || { _efr_missing; return 127 }
  efr context "$@"
}

# Finds a conversation to continue, or makes one this terminal's conversation again,
# through `efr resume`: efr opens its fzf picker (or lists the recent conversations,
# the ones of the current project first), and `,resume N` (or a conversation id, or 4
# characters of one) continues that one here; other words search. It hands over the
# shell context, so efr knows the terminal, its shell and the directory, and nothing
# else: no prompt, no last command and no turn settings. A word that starts with `-`,
# such as --take, is an option of `efr resume`; the other words reach efr joined by
# blanks in EFR_RESUME_WORDS, never in its arguments, as a prompt does.
function ,resume {
  emulate -L zsh
  _efr_available || { _efr_missing; return 127 }
  local -a options words
  local word
  for word in "$@"; do
    if [[ $word == -* ]]; then
      options+=($word)
    else
      words+=($word)
    fi
  done
  _efr_context_json ''
  EFR_CONTEXT=$REPLY EFR_RESUME_WORDS=${(j: :)words} efr resume "${options[@]}"
}

# Tells efr that you are away from this terminal's conversation, through `efr away`:
# the reviewer of auto still decides most actions, but a question for you then waits
# only presence.away_wait_secs, and its call does not run. Your next input in the
# conversation ends it, so there is no command to come back. It takes no words and
# hands nothing over: no prompt, no context and no turn settings.
function ,away {
  emulate -L zsh
  _efr_available || { _efr_missing; return 127 }
  if (( $# )); then
    print -u2 -- "usage: ,away"
    return 2
  fi
  efr away
}

# Answers or stops the command that a shell call left running in the hidden shell of
# this terminal's conversation, through `efr shell`. It is the one place to type a
# password for such a command: a line typed at the prompt goes to the shell, or in
# sticky mode to the agent. It hands nothing over: no prompt, no context and no turn
# settings. The words are arguments of `efr shell`, such as a conversation id.
function ,shell {
  emulate -L zsh
  _efr_available || { _efr_missing; return 127 }
  efr shell "$@"
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

# Shows the indicator before the typed text while sticky mode is on: the dim tag of the
# terminal's own turn settings (see _efr_tag), the robot, the full mark while the
# terminal's prompts run in full mode (EFR_FULL_MARK, unstyled), and the
# muted scope of the next prompt with `›` after it (see _efr_update_scope), such as
# `auto 🤖🔓 app › `. Without a projects file the scope part is left out. With $1
# `tag` only the tag stays, for a prompt line whose first word takes the robot's
# place; with `hide` nothing shows. With sticky mode off nothing shows: the turn's red
# first line says full mode. PREDISPLAY is not part of the buffer and leaves PROMPT
# alone, so it works with any prompt theme, including one whose prompt starts with a
# newline, and history never sees it. Only a command line gets it, never a
# continuation line or a value that vared edits. Runs from the line-init hook, on every
# toggle and when a line is accepted.
_efr_show_indicator() {
  # NOTE: zle gives region_highlight back with other offsets than it was given, so the
  # plugin's own entries are found by their memo, not by their text.
  region_highlight=(${region_highlight:#*memo=efr*})
  if (( _efr_sticky )) && [[ $1 != hide && $CONTEXT == start ]]; then
    local REPLY tag
    local -a mine
    _efr_tag
    tag=$REPLY
    PREDISPLAY=$tag
    [[ -n $tag ]] && mine+=("P0 ${#tag} $EFR_TAG_STYLE memo=efr")
    if [[ $1 != tag ]]; then
      # The robot without its trailing blanks, then the full mark, then the blanks.
      local gap=${EFR_STICKY_INDICATOR##*[^[:space:]]} word start
      word=${EFR_STICKY_INDICATOR%"$gap"}
      start=${#PREDISPLAY}
      if [[ -n $EFR_FULL_MARK ]] && _efr_full_mode; then
        PREDISPLAY+=$word
        mine+=("P$start ${#PREDISPLAY} $EFR_STICKY_STYLE memo=efr")
        PREDISPLAY+=$EFR_FULL_MARK$gap
      else
        PREDISPLAY+=$word$gap
        mine+=("P$start ${#PREDISPLAY} $EFR_STICKY_STYLE memo=efr")
      fi
      _efr_update_scope
      if [[ -n $_efr_scope_text ]]; then
        [[ -z $gap ]] && PREDISPLAY+=' '
        start=${#PREDISPLAY}
        PREDISPLAY+="$_efr_scope_text $_efr_scope_end "
        mine+=("P$start ${#PREDISPLAY} $EFR_SCOPE_STYLE memo=efr")
      fi
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
# matters`) still goes to the agent. A value must also look like an id (letters,
# digits and `.`, `_`, `:`, `/`, `-`), because the line runs as shell code and a
# daemon without a model list accepts any model: `model a;b` is a prompt, never `b`.
# Returns 1 for any other line.
_efr_setting_line() {
  emulate -L zsh -o extended_glob
  [[ $1 == (#b)[[:space:]]#(mode|model|effort)([[:space:]]##([^[:space:]]##)|)[[:space:]]# ]] ||
    return 1
  local name=$match[1] value=$match[3]
  if [[ -n $value && $value != default ]]; then
    [[ $value == [[:alnum:]._:/-]## ]] || return 1
    _efr_available || return 1
    local -a reply
    _efr_settings "--$name=$value" >/dev/null 2>&1 || return 1
  fi
  # Quoted all the same, so the line can never hold more than the one value.
  REPLY=",$name${value:+ ${(q)value}}"
}

# Sets REPLY to the line that runs for the prompt text $1 when the text is one plugin
# command word alone: `new`, `compact`, `context`, `resume`, `away` or `shell` becomes
# `,new` and so on, and `mode`, `model` or `effort` with at most one value becomes the
# line of _efr_setting_line. This is the rule of a `, ` line and of a line in sticky
# mode. Returns 1 for any other text, which stays a prompt.
_efr_command_word_line() {
  emulate -L zsh -o extended_glob
  if [[ $1 == (#b)[[:space:]]#(new|compact|context|resume|away|shell)[[:space:]]# ]]; then
    REPLY=",$match[1]"
    return 0
  fi
  _efr_setting_line "$1"
}

# Sets REPLY to the line that runs for the line $1 when it is a `,` prompt whose text
# is one command word alone (see _efr_command_word_line): `, resume` (or the sticky
# word, a blank and `resume`) becomes `,resume`. The blanks before it stay. Returns 1
# for any other line, also for a prompt of several lines.
_efr_comma_command_line() {
  emulate -L zsh -o extended_glob
  local word=
  _efr_sticky_word && word="|${(b)REPLY}"
  [[ $1 == (#b)([[:space:]]#)(,${~word})[[:space:]]##(*) ]] || return 1
  local lead=$match[1] text=${match[3]%%[[:space:]]##}
  [[ $text != *$'\n'* ]] || return 1
  _efr_command_word_line "$text" || return 1
  REPLY=$lead$REPLY
}

# Sets REPLY to the line that runs for the accepted line $1. In sticky agent mode a
# line goes to the agent unless it is a plugin line (`,` and the commands above, or a
# line that history recalled with the sticky word in front), starts with `!` (an
# escape hatch for one shell command), is a bare command word (see
# _efr_command_word_line), or is empty. Such a prompt line gets the indicator's own text in
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
  elif _efr_command_word_line "$line"; then
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
# instead of a `command not found`. A word that starts with `!` is a steer that lost
# its blank: `,!stop` becomes `,! stop`. Returns 1 for any other line, which stays as
# it is.
_efr_unknown_comma_line() {
  emulate -L zsh -o extended_glob
  [[ $1 == (#b)([[:space:]]#),([^[:space:]]##)(*) ]] || return 1
  # A line that defines a `,` function, such as `,mine() { ... }`, is shell code: the
  # name it defines cannot be found before the line runs.
  [[ $match[2]$match[3] == [^[:space:]\(]##[[:space:]]#'()'* ]] && return 1
  # whence knows every kind of name that zsh runs, the plugin's own commands too.
  whence -- ",$match[2]" >/dev/null && return 1
  if [[ $match[2] == '!'?* ]]; then
    REPLY="$match[1],! ${match[2]#!}$match[3]"
  else
    REPLY="$match[1], $match[2]$match[3]"
  fi
}

# True when $1 and $2 differ by exactly one letter added, dropped, changed, or two
# neighbours swapped.
_efr_one_edit() {
  emulate -L zsh
  local a=$1 b=$2 i=1
  if (( ${#a} > ${#b} )); then
    a=$2 b=$1
  fi
  (( ${#b} - ${#a} <= 1 )) || return 1
  while (( i <= ${#a} )) && [[ ${a[i]} == "${b[i]}" ]]; do
    (( i++ ))
  done
  if (( ${#a} < ${#b} )); then
    [[ ${a[i,-1]} == "${b[i+1,-1]}" ]]
  elif (( i > ${#a} )); then
    return 1
  else
    [[ ${a[i+1,-1]} == "${b[i+1,-1]}" ]] && return 0
    [[ ${a[i]} == "${b[i+1]}" && ${a[i+1]} == "${b[i]}" && ${a[i+2,-1]} == "${b[i+2,-1]}" ]]
  fi
}

# Sets REPLY to a hint when the first word of the line $1 is a typo of a plugin
# command's `,` word, such as `,moed` for `,mode`: one edit away from `,mode`,
# `,model`, `,effort`, `,compact`, `,context` or `,resume`, or the letters of `,new`,
# `,away` or `,shell` in another order. Only the order counts for the short `,new` and
# `,away`, because one edit from them is also the start of a prompt such as `,now` or
# `,sway`, and for `,shell`, which is one edit from `,spell`.
# A word that zsh could run is no typo. Returns 1 for any other line.
_efr_mistyped_command() {
  emulate -L zsh -o extended_glob
  [[ $1 == (#b)[[:space:]]#,([[:alpha:]]##)([[:space:]]*|) ]] || return 1
  local word=$match[1] name meant=
  whence -- ",$word" >/dev/null && return 1
  # `model` first: `,modl` is one edit from both, and more likely `,model` with a
  # letter lost.
  for name in model mode effort compact context resume; do
    if _efr_one_edit $word $name; then
      meant=$name
      break
    fi
  done
  [[ -z $meant && ${(j::)${(os::)word}} == enw ]] && meant=new
  [[ -z $meant && ${(j::)${(os::)word}} == aawy && $word != away ]] && meant=away
  [[ -z $meant && ${(j::)${(os::)word}} == ehlls && $word != shell ]] && meant=shell
  [[ -n $meant ]] || return 1
  REPLY="efr: ,$word is no command; did you mean ,$meant? \`, $word\` sends a prompt"
}

# For a plugin line $1 (`,`, `,new`, `,!`, `,compact` or the sticky word, followed by
# a prompt or a focus, which may span several lines): saves the prompt for the command
# and the user's two options for _efr_restore_options, sets REPLY to the line that
# runs (see _efr_cut_line) and returns 0. The caller then sets the options, because an
# emulate here would undo them. Any other line returns 1 and changes nothing.
_efr_stash_line() {
  emulate -L zsh -o extended_glob
  _efr_command_pattern
  [[ $1 == (#b)[[:space:]]#(${~REPLY})[[:space:]]##(*) ]] || return 1
  local rest=${match[2]%%[[:space:]]##}
  [[ -n $rest ]] || return 1
  _efr_stash=$rest
  _efr_stash_set=1
  _efr_saved_options=($options[interactivecomments] $options[banghist])
  _efr_cut_line "$1"
}

# For a line $1 of several lines that are all `,` prompts (blank lines aside): `, `
# and the text, a `,word` that names nothing (see _efr_unknown_comma_line), or the
# sticky word and the text; the first one may also be `,new` and the text. Saves the
# first prompt for the command as _efr_stash_line does, and the others in _efr_queue,
# for efr to queue behind it. Sets REPLY to the line that runs (see _efr_cut_line) and
# returns 0. A line that is a command word alone (`, resume`, `, mode auto`) is no
# prompt. Any other line returns 1 and changes nothing: it stays one prompt.
_efr_split_line() {
  emulate -L zsh -o extended_glob
  [[ $1 == *$'\n'* ]] || return 1
  local word= line text
  _efr_sticky_word && word="|${(b)REPLY}"
  local first=",new|,$word" later=",$word"
  local -a texts
  for line in "${(@f)1}"; do
    [[ -n ${line//[[:space:]]/} ]] || continue
    _efr_unknown_comma_line "$line" && line=$REPLY
    if (( $#texts )); then
      [[ $line == (#b)[[:space:]]#(${~later})[[:space:]]##(*) ]] || return 1
    else
      [[ $line == (#b)[[:space:]]#(${~first})[[:space:]]##(*) ]] || return 1
    fi
    text=${match[2]%%[[:space:]]##}
    [[ -n $text ]] || return 1
    # The words of _efr_command_word_line, without its call of efr for a value.
    [[ $text == (new|compact|context|resume|away|shell) ]] && return 1
    [[ $text == (mode|model|effort)([[:space:]]##[^[:space:]]##|) ]] && return 1
    texts+=("$text")
  done
  (( $#texts > 1 )) || return 1
  _efr_stash=$texts[1]
  _efr_stash_set=1
  _efr_queue=("${(@)texts[2,-1]}")
  _efr_saved_options=($options[interactivecomments] $options[banghist])
  _efr_cut_line "$1"
}

# Sets REPLY to the part of the line $1 that runs: up to the end of its first line
# that is not blank. When lines that are not blank follow, sets _efr_shown_rest to them,
# from the newline on, for the screen to show after it, and _efr_history_line to $1
# without the blanks at its end; otherwise empties both.
_efr_cut_line() {
  emulate -L zsh -o extended_glob
  REPLY=$1
  _efr_shown_rest=
  _efr_history_line=
  [[ $1 == (#b)([[:space:]]#[^[:space:]][^$'\n']#)(*) ]] || return 0
  [[ -n ${match[2]//[[:space:]]/} ]] || return 0
  REPLY=$match[1]
  _efr_shown_rest=$match[2]
  _efr_history_line=${1%%[[:space:]]##}
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
  # A typo of a plugin command would otherwise become a prompt, and so a model call:
  # the line stays for the user to fix, and a blank after the `,` still sends it.
  if _efr_mistyped_command "$BUFFER"; then
    zle -M "$REPLY"
    return 0
  fi
  local sticky_prompt=0
  if (( _efr_sticky )); then
    _efr_line_to_run "$BUFFER"
    [[ $REPLY == "$BUFFER" ]] || BUFFER=$REPLY
    # The line that runs takes the indicator's place on the screen: a prompt line
    # starts with the sticky word where the indicator stood, after the tag that stays,
    # so nothing moves, and a `!` line shows the command that runs.
    if (( _efr_prompt_line )); then _efr_show_indicator tag; else _efr_show_indicator hide; fi
    _efr_alias_sticky_word
    sticky_prompt=$_efr_prompt_line
  fi
  _efr_unknown_comma_line "$BUFFER" && BUFFER=$REPLY
  # A prompt line that sticky mode made is no command word: _efr_line_to_run checked.
  (( sticky_prompt )) || ! _efr_comma_command_line "$BUFFER" || BUFFER=$REPLY
  local _efr_shown_rest=
  if _efr_split_line "$BUFFER" || _efr_stash_line "$BUFFER"; then
    # The line stays as typed. The alias ends in `#`, which starts a comment only with
    # interactive_comments on, and bang_hist off keeps `!` in the prompt literal.
    # NOTE: of a prompt over several lines only the first line runs, because the
    # comment ends at the newline; the others show after it, as they were.
    if [[ -n $_efr_shown_rest ]]; then
      BUFFER=$REPLY
      POSTDISPLAY=$_efr_shown_rest
    fi
    setopt interactive_comments
    unsetopt bang_hist
  elif _efr_is_plugin_line "$BUFFER"; then
    # Any other plugin line, such as a bare `,new`: the alias's `#` must stay a plain
    # word for this line, and the rewrite quotes a prompt as one word.
    _efr_rewrite_line "$BUFFER"
    BUFFER=$REPLY
    _efr_saved_options=($options[interactivecomments] $options[banghist])
    unsetopt interactive_comments
  fi
  zle _efr_orig_accept_line
}

# --- scope of the next prompt ------------------------------------------------------

# Reads efrd's projects file (projects.tsv in the runtime root, see _efr_runtime_root)
# into _efr_project_roots and _efr_project_names, again only when its inode, time or
# size changed: efrd replaces it by a rename, so each write is a new inode. Each line
# is `<root><tab><name>`, the deepest root first. Returns 1 when there is no file,
# such as before efrd ever ran. It runs no program: zstat is a builtin, and zsh reads
# $(<file) itself.
_efr_read_projects() {
  emulate -L zsh
  local REPLY file
  local -A stat
  if ! _efr_runtime_root || ! zstat -H stat -- $REPLY/projects.tsv 2>/dev/null; then
    _efr_projects_seen=
    _efr_project_roots=()
    _efr_project_names=()
    return 1
  fi
  file=$REPLY/projects.tsv
  local stamp="$file:$stat[inode]:$stat[mtime]:$stat[size]"
  [[ $stamp == "$_efr_projects_seen" ]] && return 0
  _efr_projects_seen=$stamp
  _efr_project_roots=()
  _efr_project_names=()
  local line
  [[ -r $file ]] || return 0
  for line in "${(@f)$(<$file)}"; do
    [[ $line == /*$'\t'* ]] || continue
    _efr_project_roots+=("${line%%$'\t'*}")
    _efr_project_names+=("${line#*$'\t'}")
  done
  return 0
}

# Sets _efr_scope_text to the scope of a prompt sent now: the name of the registered
# project whose root holds $PWD, or else ${PWD:A} (its links resolved), as efrd routes;
# the outside mark (`○`) anywhere else; empty without a projects file. A root matches
# only whole components: ~/p/app holds ~/p/app/src but not ~/p/apple. The deepest root
# wins. Sets efr_scope to the same text with each `%` doubled for a prompt.
_efr_update_scope() {
  emulate -L zsh
  if ! _efr_read_projects; then
    _efr_scope_text= efr_scope=
    return 0
  fi
  local form root best= name=
  local -i i
  for form in $PWD ${PWD:A}; do
    for (( i = 1; i <= $#_efr_project_roots; i++ )); do
      root=$_efr_project_roots[i]
      [[ $form == "$root" || $form == "$root"/* || $root == / ]] || continue
      if (( ${#root} > ${#best} )); then
        best=$root
        name=$_efr_project_names[i]
      fi
    done
    [[ -n $best ]] && break
  done
  if [[ -n $best ]]; then
    _efr_scope_text=${name:-${best:t}}
  else
    _efr_scope_text=$_efr_outside_mark
  fi
  efr_scope=${_efr_scope_text//\%/%%}
}

# Prints the scope of a prompt sent now, for your own prompt: the project's name, such
# as `app`, or `○` outside projects, after the full mark (EFR_FULL_MARK) and a blank
# while the terminal's prompts run in full mode. No colours, and nothing without a
# projects file. Each `%` of the name is doubled, as in $efr_scope, so a prompt with
# prompt_subst shows it as typed. It starts no program, but $(efr_prompt_info) is a
# subshell; the variable $efr_scope needs none. For example:
#   setopt prompt_subst; RPROMPT='$(efr_prompt_info)'
efr_prompt_info() {
  emulate -L zsh
  _efr_update_scope
  [[ -n $efr_scope ]] || return 0
  if [[ -n $EFR_FULL_MARK ]] && _efr_full_mode; then
    print -r -- "$EFR_FULL_MARK $efr_scope"
  else
    print -r -- "$efr_scope"
  fi
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

# When this shell loaded the plugin, in whole seconds. A notice file that was last
# written before it belongs to an earlier terminal with the same name, such as one
# before a reboot, and is dropped unseen. Re-sourcing keeps the first value. 0, when
# zsh/datetime is missing, drops nothing.
(( ${+_efr_shell_start} )) || typeset -gi _efr_shell_start=${${EPOCHREALTIME-0}%%[.,]*}

# The daemon leaves notices (an approval waiting, a finished turn) in one file per
# terminal under its runtime root; the prompt shows them. Moving the file first means
# a notice written while printing lands in a new file instead of being lost. The rule
# of _efr_shell_start works per file: the time of the file is the time of its last
# line, so one new line keeps the older lines of the same file too.
_efr_print_notices() {
  [[ -n $TTY ]] || return 0
  local REPLY
  _efr_runtime_root || return 0
  local file="$REPLY/notices/${${TTY#/dev/}//\//-}"
  [[ -s $file ]] || return 0
  local shown="$file.shown.$$"
  zf_mv -f -- "$file" "$shown" 2>/dev/null || return 0
  local -a mtime
  if (( _efr_shell_start > 0 )) && zstat -A mtime +mtime -- "$shown" 2>/dev/null \
      && (( mtime[1] < _efr_shell_start )); then
    zf_rm -f -- "$shown" 2>/dev/null
    return 0
  fi
  print -r -- "$(<$shown)"
  zf_rm -f -- "$shown" 2>/dev/null
}

# --- last command -----------------------------------------------------------------

# preexec gets the line as typed, just before it runs. The line is parsed by then, so
# a plugin line's own options can go.
_efr_preexec() {
  _efr_running=$1
  _efr_running_cwd=$PWD
  _efr_running_start=${EPOCHREALTIME-}
  _efr_restore_options
}

# Puts the whole text of a prompt over several lines in history, where zsh would keep
# only the first line, the one that ran (see _efr_cut_line). Enter on it sends the same
# prompts again. zsh's rule for a line with a blank in front still holds: such a line
# keeps out of history.
_efr_add_history() {
  local line=$_efr_history_line
  _efr_history_line=
  [[ -n $line ]] || return 0
  [[ -o hist_ignore_space && $line == [[:blank:]]* ]] && return 0
  print -sr -- "$line"
  # A history of its own for the first line, which zsh drops once the line is saved.
  fc -p
}

# Keeps the line that just finished, with its status, unless it ran a plugin command.
# It keeps the first _efr_command_max characters of the line. A line that starts with a
# blank stays private (HIST_IGNORE_SPACE's rule): efr gets no last command for it, and
# not the line before it either.
_efr_remember_command() {
  emulate -L zsh
  if [[ -n $_efr_running ]] && ! _efr_is_plugin_line "$_efr_running"; then
    if [[ $_efr_running == [[:blank:]]* ]]; then
      _efr_last_command=''
    else
      _efr_last_command=${_efr_running[1,_efr_command_max]}
    fi
    _efr_last_command_status=$1
    _efr_remember_recent $1
  fi
  _efr_running=''
}

# Adds the line that just finished, with its status $1, to the terminal's recent
# commands (see _efr_recent), unless it starts with a blank or runs efr itself. The
# JSON is built here, once, without a fork, so a `,` line only joins the entries. It
# keeps the first _efr_command_max characters of the line, as the last command does.
_efr_remember_recent() {
  emulate -L zsh -o extended_glob
  local line=${_efr_running[1,_efr_command_max]} REPLY entry
  [[ $line == [[:blank:]]* || -z ${line//[[:space:]]/} ]] && return 0
  [[ $line == efr([[:space:]]*|) ]] && return 0
  _efr_json_string "$line"
  entry="{\"command\":$REPLY"
  [[ $1 == <-> ]] && entry+=",\"status\":$1"
  if [[ -n $_efr_running_cwd ]]; then
    _efr_json_string "$_efr_running_cwd"
    entry+=",\"cwd\":$REPLY"
  fi
  [[ ${EPOCHSECONDS-} == <-> ]] && entry+=",\"ended\":$EPOCHSECONDS"
  if [[ -n $_efr_running_start && -n ${EPOCHREALTIME-} ]]; then
    local -i ms=$(( (EPOCHREALTIME - _efr_running_start) * 1000 ))
    (( ms >= 0 )) && entry+=",\"duration_ms\":$ms"
  fi
  _efr_recent+=("$entry}")
  (( $#_efr_recent > _efr_recent_max )) && _efr_recent=("${(@)_efr_recent[-_efr_recent_max,-1]}")
  return 0
}

# Shows the cursor after a line that ran efr. efr hides it while a turn runs and shows
# it on every way out that it controls; this covers the rest, such as a kill -9.
_efr_show_cursor() {
  (( _efr_called )) || return 0
  _efr_called=0
  [[ -t 1 ]] && print -rn -- $'\e[?25h'
  return 0
}

_efr_precmd() {
  # Must be first: the status of the line that just finished.
  local exit_status=$?
  _efr_show_cursor
  _efr_restore_options
  # A saved prompt that no command took (the line failed before it ran) must not
  # leak into the next one.
  _efr_stash=''
  _efr_stash_set=0
  _efr_queue=()
  _efr_history_line=
  _efr_remember_command $exit_status
  _efr_take_draft
  _efr_print_notices
  _efr_register_completion
  _efr_update_scope
}

# --- terminal background ----------------------------------------------------------

# Sets EFR_TERMINAL_BG to dark or light, for render.theme = "auto" in efr's config. It
# asks the terminal once, when the plugin loads: OSC 11 (the background colour), then
# DA1, which every terminal answers, so its reply marks the end. A background whose
# luminance (0.299 R + 0.587 G + 0.114 B) is over one half is light. No reply to OSC 11
# before the DA1 reply means dark.
#
# The plugin reads until the DA1 reply comes, for at most one second, also over a slow
# ssh connection: a reply that came later would go to the line editor as typed text.
# Only a terminal that answers nothing makes the shell start one second later.
#
# efr never asks itself: a reply comes in on stdin, and efr would take the keys that the
# user typed ahead for the shell. Here no key is lost: there is no question when keys
# wait already (their reply would come after them), and keys typed while the replies
# are on their way go back to the line editor without the replies. When the DA1 reply
# does not come, only the keys before the first escape go back. A value set by hand, a
# re-source, a terminal that is not one or a dumb one ask nothing.
_efr_probe_background() {
  emulate -L zsh
  setopt extended_glob
  [[ -z ${EFR_TERMINAL_BG-} && -t 0 && -t 1 && ${TERM:-dumb} != dumb ]] || return 0
  zmodload zsh/zselect 2>/dev/null || return 0
  zmodload -F zsh/datetime p:EPOCHREALTIME 2>/dev/null || return 0
  # zselect -t counts hundredths of a second; 0 only looks. It writes what it found to
  # an array of its own, so it never touches REPLY or reply.
  local -a ready
  zselect -a ready -t 0 -r 0 && return 0
  local saved
  saved=$(command stty -g 2>/dev/null) || return 0
  command stty -icanon -echo min 1 time 0 2>/dev/null || return 0
  print -rn -- $'\e]11;?\e\\\e[c'
  local answer='' key=''
  local -F deadline=$(( EPOCHREALTIME + 1 )) left
  local -i hundredths ended=0
  while (( (left = deadline - EPOCHREALTIME) > 0 )); do
    (( hundredths = left * 100 + 1 ))
    zselect -a ready -t $hundredths -r 0 || continue
    read -r -s -k 1 -u 0 key || break
    answer+=$key
    if [[ $answer == *$'\e['\?[0-9\;]#c ]]; then
      ended=1
      break
    fi
  done
  command stty $saved 2>/dev/null
  # What is not a reply was typed: it goes back to the line editor.
  local typed
  if (( ended )); then
    typed=${answer%$'\e['\?[0-9\;]#c}
    typed=${typed/$'\e]11;'[^$'\a\e']#($'\a'|$'\e\\')}
  else
    typed=${answer%%$'\e'*}
  fi
  [[ -n $typed ]] && print -rz -- $typed
  local background=dark
  if [[ $answer == (#b)*$'\e]11;rgb:'([[:xdigit:]]##)/([[:xdigit:]]##)/([[:xdigit:]]##)* ]]; then
    local -F red=$(( 16#$match[1] / (16.0 ** $#match[1] - 1) ))
    local -F green=$(( 16#$match[2] / (16.0 ** $#match[2] - 1) ))
    local -F blue=$(( 16#$match[3] / (16.0 ** $#match[3] - 1) ))
    (( 0.299 * red + 0.587 * green + 0.114 * blue > 0.5 )) && background=light
  fi
  typeset -gx EFR_TERMINAL_BG=$background
  return 0
}

# --- completion -------------------------------------------------------------------

# The arguments of `,mode`, `,model` and `,effort`: `default` and the names that efr
# accepts. Only the first argument completes.
_efr_complete_mode() {
  (( CURRENT == 2 )) || return 1
  compadd -- default manual cautious auto full
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
alias ,=', #' ,new=',new #' ,compact=',compact #'
alias ',!=,! #'
_efr_alias_sticky_word
# A shell that ran a version before PREDISPLAY still has its indicator in PROMPT.
[[ $PROMPT == "$_efr_old_prompt_indicator"* ]] && PROMPT=${PROMPT#"$_efr_old_prompt_indicator"}
# Ctrl+Space sends NUL (^@) in common terminals.
bindkey -M emacs '^@' _efr_toggle_sticky
bindkey -M viins '^@' _efr_toggle_sticky
add-zsh-hook precmd _efr_precmd
add-zsh-hook preexec _efr_preexec
add-zsh-hook zshaddhistory _efr_add_history
_efr_register_completion
_efr_probe_background
_efr_clean_drafts

_efr_available || print -u2 -- "efr.plugin.zsh: efr is not on PATH; the , commands stay inactive until it is installed"
