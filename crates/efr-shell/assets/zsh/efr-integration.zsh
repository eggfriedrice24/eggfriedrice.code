# efr: shell integration for the hidden conversation shell.
#
# Written for efr. It emits the same marks as the zsh integrations of ghostty and
# kitty, so efr's scanner and a ghostty screen read one stream the same way:
#
#   ESC ] 133 ; A ; cl=line BEL      a primary prompt starts            (precmd)
#   ESC ] 133 ; P ; k=s BEL          a continuation prompt starts       (zle-line-init)
#   ESC ] 133 ; B BEL                the prompt ends, the input starts  (zle-line-init)
#   ESC ] 133 ; C BEL                the command line starts to run     (preexec)
#   ESC ] 133 ; D ; <status> BEL     the command line has finished      (precmd)
#   ESC ] 133 ; D BEL                a prompt ended without running anything (precmd)
#   ESC ] 7 ; kitty-shell-cwd://<host><path> BEL   the working directory
#   ESC ] 133 ; efr-sbx ; <nonce> BEL  a sandboxed call has ended   (_efr_hs_sbx)
#
# efr cuts a command's output from the recording between the end of C and the start
# of D, so nothing else may print between them: the precmd hook runs first and the
# preexec hook runs last. The directory report goes out before D, so the directory a
# command leaves behind is known when D arrives.
#
# .zshenv sources this file before the user's .zshrc. All it does now is define
# functions and arm _efr_hs_init, which runs at the first prompt, after every startup
# file, and installs the hooks around whatever the user's files set up.
#
# Every name here starts with _efr_hs_, so nothing the user's .zshrc loads can
# replace it. The efr plugin for interactive terminals (efr.plugin.zsh) has hooks of
# its own with the shorter efr prefix; with the same names, sourcing it from .zshrc
# would replace these hooks and the hidden shell would never print A, C or D. The one
# exception is the compinit wrapper at the end, which must have compinit's name.

# The programs that a permission rule trusts by name, from the daemon. An alias or a
# function of the same name from the user's startup files would run something else
# for a line that the engine allowed. Read once and kept out of every child's
# environment. The user's .zshenv has run, so the `-` keeps NO_UNSET from stopping
# this file when the daemon passed none.
builtin typeset -ga _efr_hs_trusted
_efr_hs_trusted=(${(s: :)_EFR_HS_TRUSTED_PROGRAMS-})
builtin unset _EFR_HS_TRUSTED_PROGRAMS

# The editor stub that efr writes next to this file. %x names this file while it is
# sourced, and nowhere later.
builtin typeset -g _efr_hs_editor=${${(%):-%x}:A:h}/efr-editor

# $functions and $options, which the sandbox's wrapper reads, and $EPOCHREALTIME, with
# which it times its steps.
builtin zmodload zsh/parameter
builtin zmodload -F zsh/datetime p:EPOCHREALTIME 2>/dev/null

# The sandbox of the auto mode: the conversation's sandbox dir ($R/sbx/<conversation>)
# and the launcher copy ($R/bin/efr-sbx), from the daemon. Read once, read-only, and
# kept out of every child's environment, like the trusted programs above.
builtin typeset -gr _efr_hs_sbx_dir=${_EFR_HS_SBX_DIR-}
builtin typeset -gr _efr_hs_sbx_bin=${_EFR_HS_SBX_BIN-}
builtin unset _EFR_HS_SBX_DIR _EFR_HS_SBX_BIN
# 1 while snapshot.zsh may be older than this shell's functions, aliases and options:
# from the start, and after every line that efr did not type for a sandboxed call.
builtin typeset -gi _efr_hs_sbx_stale=1

# 0: nothing shown yet, 1: a prompt is shown, 2: a command line runs.
builtin typeset -gi _efr_hs_state=0
builtin typeset -g _efr_hs_pwd=
# The PATH that _efr_hs_report_path wrote last.
builtin typeset -g _efr_hs_path=
# 1 when the user's options asked for zsh's PROMPT_SP mark; see _efr_hs_precmd.
builtin typeset -gi _efr_hs_prompt_sp=0

_efr_hs_report_pwd() {
  builtin emulate -L zsh
  [[ $PWD == "$_efr_hs_pwd" ]] && return 0
  # A control character in the path would end the OSC sequence early.
  [[ $PWD == *[[:cntrl:]]* ]] && return 0
  _efr_hs_pwd=$PWD
  builtin print -rn -- $'\e]7;kitty-shell-cwd://'"${HOST}${PWD}"$'\a'
}

# Writes this shell's PATH to $_efr_hs_sbx_dir/path when it changed. The user's
# startup files set it, and efrd resolves the program words of an exit question with
# it, so the question names the program that this shell runs. The dir exists from the
# first sandboxed call on; until then nothing is written.
_efr_hs_report_path() {
  builtin emulate -L zsh
  [[ -n $_efr_hs_sbx_dir && $PATH != "$_efr_hs_path" && -d $_efr_hs_sbx_dir ]] || return 0
  builtin print -r -- $PATH 2>/dev/null >| $_efr_hs_sbx_dir/path && _efr_hs_path=$PATH
  return 0
}

_efr_hs_precmd() {
  builtin local -i st=$?
  builtin emulate -L zsh
  # Some plugins run precmd hooks from inside a widget to refresh the prompt; a mark
  # printed then would land in the middle of the line editor's display.
  builtin zle && return 0
  _efr_hs_report_pwd
  _efr_hs_report_path
  if (( _efr_hs_state == 2 )); then
    _efr_hs_drain
    builtin print -rn -- $'\e]133;D;'"${st}"$'\a'
  elif (( _efr_hs_state == 1 )); then
    builtin print -rn -- $'\e]133;D\a'
  fi
  # zsh prints its PROMPT_SP mark before any precmd hook runs, which would put it
  # inside the output of every command, so _efr_hs_init turns the option off and the
  # same mark is printed here, after D.
  if (( _efr_hs_prompt_sp )); then
    builtin printf '%s%*s\r \r' "${(%)PROMPT_EOL_MARK-%B%S%#%s%b}" $(( COLUMNS - 1 )) ''
  fi
  builtin print -rn -- $'\e]133;A;cl=line\a'
  _efr_hs_state=1
  _efr_hs_no_user_hooks
}

# The hidden shell runs no hook and no prompt code of the user, in every mode. It cd's
# into directories that a model chose and draws a prompt after every call, so a prompt
# theme (starship, powerlevel10k, vcs_info) or a chpwd hook (direnv, nvm, a venv
# switcher) would run git or source files that a sandboxed call just wrote, outside the
# sandbox. _efr_hs_install removes them once the startup files have run, and every
# prompt removes those that a plugin or a command line added since. The user's
# functions, aliases and exported variables stay; the sandbox's child shell replays
# them. Only this hook stays in precmd; preexec resets that list, because zsh walks it
# here.
_efr_hs_no_user_hooks() {
  preexec_functions=(_efr_hs_preexec)
  chpwd_functions=(_efr_hs_report_pwd)
  periodic_functions=()
  zshaddhistory_functions=()
  PS1='%# '
  RPS1=
}

# Throws away input that reached the terminal while a command ran and that the
# command never read: an answer that efr wrote for a password prompt just as sudo gave
# up, or keys typed at the attached screen. The line editor would otherwise read it as
# the next command line, so `hunter2` and Enter would run as a command, show on the
# screen and land in the recording. It runs before D. efr types a marked (Auto) command
# only at a ready prompt, after it has seen D, so none is lost here. A sentinel run is
# the exception: efr types it at once, without waiting for D (it is meant for a shell
# started inside this one, or for a shell whose marks have not come yet), so a sentinel
# line that arrives as a command ends is thrown away here. Meant for a nested shell
# that just ended, it must not run in this one; its run gets no end marker and waits
# for its timeout.
_efr_hs_drain() {
  builtin emulate -L zsh
  builtin local junk
  while builtin read -s -t 0 -k 1 junk 2>/dev/null; do :; done
  return 0
}

_efr_hs_preexec() {
  builtin emulate -L zsh -o extended_glob
  # The precmd array is reset here and not in precmd, where zsh walks it.
  precmd_functions=(_efr_hs_precmd)
  # A line that efr typed for a sandboxed call changes nothing here but the directory
  # and filtered exports, so the snapshot of functions, aliases and options stays
  # fresh; any other line may change them.
  [[ ${3:-$1} == *' && \_efr_hs_sbx '[0-9a-f-](#c36) ]] || _efr_hs_sbx_stale=1
  builtin print -rn -- $'\e]133;C\a'
  _efr_hs_state=2
}

_efr_hs_line_init() {
  builtin emulate -L zsh
  case $CONTEXT in
    start) builtin print -rn -- $'\e]133;B\a' ;;
    cont) builtin print -rn -- $'\e]133;P;k=s\a\e]133;B\a' ;;
  esac
}

_efr_hs_install() {
  builtin emulate -L zsh
  precmd_functions=(${precmd_functions:#_efr_hs_init})

  # zsh loads the line editor when it first starts it, which is after this first
  # precmd, and add-zle-hook-widget gives up when the module is not loaded yet.
  builtin zmodload zsh/zle
  # The user's hooks go (see _efr_hs_no_user_hooks): the hook arrays, the functions
  # that zsh calls by name, and the line editor's hook widgets with the lists that
  # add-zle-hook-widget keeps for them.
  builtin local name
  for name in precmd preexec chpwd periodic zshaddhistory; do
    (( ${+functions[$name]} )) && builtin unfunction -- $name
  done
  for name in isearch-exit isearch-update line-pre-redraw line-init line-finish \
      history-line-set keymap-select; do
    builtin zstyle -d zle-$name widgets
    builtin zle -D zle-$name 2>/dev/null
  done
  builtin autoload -Uz add-zle-hook-widget
  add-zle-hook-widget line-init _efr_hs_line_init
  _efr_hs_no_user_hooks
  precmd_functions=(_efr_hs_precmd)

  # efr types each command as a key that empties the line, one bracketed paste and
  # Enter. Text that someone typed at the attached screen and did not send would
  # otherwise join the command: `rm -rf ` left at the prompt and a pasted `build/tmp`
  # would run `rm -rf build/tmp`. The paste key is bound to the plain builtin widget
  # here, so plugins that rewrite pasted text (such as url-quote-magic) cannot change
  # what runs. A line that is unfinished (an unclosed quote) is cancelled with a key
  # bound to send-break: unlike Ctrl+C, a key waits in the input until the line
  # editor reads it, while a SIGINT that arrives during zle-line-init is lost.
  builtin zle -N _efr_hs_clear_line
  builtin zle -N _efr_hs_forget_credentials
  builtin zle -A .bracketed-paste _efr_hs_bracketed_paste
  builtin local keymap
  for keymap in emacs viins vicmd; do
    builtin bindkey -M $keymap $'\e[efr-clear~' _efr_hs_clear_line
    builtin bindkey -M $keymap $'\e[efr-forget~' _efr_hs_forget_credentials
    builtin bindkey -M $keymap $'\e[200~' _efr_hs_bracketed_paste
    builtin bindkey -M $keymap $'\e[efr-cancel~' send-break
  done
}

# Empties the line and leaves the line editor in insert mode, so the paste that
# follows is the whole command line, even for a vi user left in command mode. efr
# types this key before every command, so the aliases are checked right before the
# line is read, after any plugin that loads while the prompt waits.
_efr_hs_clear_line() {
  BUFFER=
  CURSOR=0
  [[ $KEYMAP == vicmd ]] && builtin zle vi-insert
  _efr_hs_plain_words
  return 0
}

# With shell.sudo_cache = "per_call", efr types this key right after each command's D
# mark, so the line editor runs it before the next line: sudo and doas forget the
# credentials they cached for this terminal, and the next sudo asks for the password
# again. It is a key and not a command line so that nothing reaches the screen, the
# recording or a tool result, and nothing it prints is shown.
_efr_hs_forget_credentials() {
  builtin emulate -L zsh
  (( ${+commands[sudo]} )) && command sudo -k </dev/null >/dev/null 2>&1
  (( ${+commands[doas]} )) && command doas -L </dev/null >/dev/null 2>&1
  return 0
}

# A command line that the permission engine allowed must run as written. A global
# alias (`alias -g L='| less'`, as oh-my-zsh's common-aliases defines) expands any
# word of a line into pipes or other programs, and a suffix alias (`alias -s txt=vim`)
# runs a program for a word that only names a file, so the hidden shell keeps neither,
# nor an alias or a function named like a program that a rule trusts. zsh also
# expands an ordinary alias named `[[` in command position, and the fixed line of a
# sandboxed call starts with `[[`, so that alias goes too, and the reserved word is
# enabled again if a line disabled it.
_efr_hs_plain_words() {
  builtin emulate -L zsh
  builtin zmodload zsh/parameter
  (( ${#galiases} )) && builtin unalias -- ${(k)galiases}
  (( ${#saliases} )) && builtin unalias -s -- ${(k)saliases}
  builtin unalias -- '[[' 2>/dev/null
  builtin enable -r -- '[[' 2>/dev/null
  builtin local name
  for name in $_efr_hs_trusted; do
    (( ${+aliases[$name]} )) && builtin unalias -- $name
    (( ${+functions[$name]} )) && builtin unfunction -- $name
  done
  return 0
}

# The auto mode never types a model's line into this shell. efr writes the line to the
# call's dir ($R/sbx/<conversation>/<call>/line) and types one fixed line that checks
# these three functions and then runs `\_efr_hs_sbx <call id>`:
#
#   [[ "${functions[_efr_hs_sbx]-}${functions[_efr_hs_sbx_apply]-}${functions[_efr_hs_sbx_snapshot]-}" == "$_efr_hs_sbx_src" && -z ${functions[builtin]-}${functions[command]-} ]] && \_efr_hs_sbx <call id>
#
# A line that ran here in another mode and redefined one of them, or a function named
# builtin or command, makes the check fail, and nothing runs. Without this file zsh
# says `command not found`, and nothing runs either. The wrapper runs the launcher, which
# starts the call in the sandbox (or the approved exit child) and writes $CALL/apply,
# applies that file, and prints the end mark with the call's nonce. Only efrd and this
# shell know the nonce, so sandboxed code cannot print an end mark that efr accepts.
_efr_hs_sbx() {
  # The user's options, read before emulate -L sets its own, for a stale snapshot.
  builtin local -A _efr_hs_sbx_opts
  (( _efr_hs_sbx_stale )) && _efr_hs_sbx_opts=("${(@kv)options}")
  builtin emulate -L zsh -o extended_glob
  [[ $1 == [0-9a-f-](#c36) ]] || { builtin print -ru2 -- 'efr: bad call id'; return 125 }
  builtin local dir=$_efr_hs_sbx_dir/$1
  [[ $_efr_hs_sbx_dir == /* && -d $dir && ! -L $dir && $_efr_hs_sbx_bin == /* && -x $_efr_hs_sbx_bin ]] ||
    { builtin print -ru2 -- 'efr: the sandbox is missing'; return 125 }
  # The times of the steps, in seconds, for efrd's debug log ($CALL/times).
  builtin local -F t0=${EPOCHREALTIME:-0} t1 t2 t3
  _efr_hs_sbx_snapshot
  t1=${EPOCHREALTIME:-0}
  builtin local -i rc=125
  # NOTE: a SIGINT that reaches this shell (efr's interrupt, while the shell holds the
  # terminal again) makes an interactive zsh abort the rest of the function. Without
  # the end mark efr would wait for this call until its timeout and hold every later
  # line, so the mark goes out in an always block. INT and QUIT are ignored only once
  # the launcher returned: bwrap and the child would keep an ignored signal.
  {
    builtin command $_efr_hs_sbx_bin run --call-dir $dir
    rc=$?
  } always {
    t2=${EPOCHREALTIME:-0}
    builtin trap '' INT QUIT
    _efr_hs_sbx_apply $dir/apply
    t3=${EPOCHREALTIME:-0}
    builtin print -r -- "snapshot $(( t1 - t0 )) launcher $(( t2 - t1 )) apply $(( t3 - t2 ))" \
      2>/dev/null >| $dir/times
    # The precmd hook reports the real $PWD again before D, also when it did not change.
    _efr_hs_pwd=
    builtin print -rn -- $'\e]133;efr-sbx;'"$(<$dir/nonce)"$'\a'
  }
  return $rc
}

# Applies $CALL/apply: NUL-separated `cd <dir>`, `export <name> <value>` and
# `unset <name>` records that the launcher filtered already. This repeats a small part
# of that filter and never evaluates text; the first record it does not know ends it.
_efr_hs_sbx_apply() {
  builtin emulate -L zsh -o extended_glob
  [[ -f $1 && ! -L $1 ]] || return 0
  builtin local -a f
  f=("${(@0)"$(<$1)"}")
  builtin local -i i=1
  while (( i <= $#f )); do
    case $f[i] in
      (cd)
        [[ $f[i+1] == /* && $f[i+1] != /(var/|)tmp(/*|) && -d $f[i+1] ]] && builtin cd -q -- "$f[i+1]"
        (( i += 2 )) ;;
      (export)
        [[ $f[i+1] == [A-Za-z_][A-Za-z0-9_](#c0,127) && $f[i+1] != (PATH|LD_*|*_PRELOAD|ZDOTDIR|FPATH|HOME|GIT_*) ]] &&
          builtin export -- "$f[i+1]=$f[i+2]"
        (( i += 3 )) ;;
      (unset)
        [[ $f[i+1] == [A-Za-z_][A-Za-z0-9_](#c0,127) && $f[i+1] != (PATH|HOME|ZDOTDIR) ]] &&
          builtin unset -- "$f[i+1]"
        (( i += 2 )) ;;
      (*) return 0 ;;
    esac
  done
  return 0
}

# Writes $_efr_hs_sbx_dir/snapshot.zsh when it is stale: this shell's functions (not
# efr's), its aliases, and its options, for the child shell of each call to replay. A
# snapshot above 4 KiB is compiled with zcompile, which the child's `source` reads
# instead. The options come from _efr_hs_sbx, which reads them before emulate -L.
_efr_hs_sbx_snapshot() {
  builtin emulate -L zsh -o extended_glob
  builtin local file=$_efr_hs_sbx_dir/snapshot.zsh
  (( _efr_hs_sbx_stale )) || [[ ! -f $file ]] || return 0
  builtin local -a names stubs on off
  # Not efr's own functions, of this file or of the user's efr plugin. Not TRAPINT and
  # TRAPQUIT either: while the child replays the snapshot, one that returns 0 would
  # catch Ctrl+C, and the line would run after it.
  names=(${(k)functions:#(_efr?*|compinit|TRAPINT|TRAPQUIT)})
  # Not the functions named _* that zsh has not loaded yet: compinit marks about 900
  # completion functions so, a child shell has no completion, and the child of every
  # call spent milliseconds to replay them and to compare them after the line.
  stubs=(${(M)${(k)functions[(R)builtin autoload -X*]}:#_*})
  names=(${names:|stubs})
  builtin local name
  for name in ${(k)_efr_hs_sbx_opts}; do
    # Options that only an interactive shell, its startup or its job control have.
    [[ $name == (interactive|login|monitor|zle|shinstdin|singlecommand|privileged|restricted) ]] && continue
    if [[ $_efr_hs_sbx_opts[$name] == on ]]; then on+=($name); else off+=($name); fi
  done
  builtin local snapshot
  # The child starts with zsh's own aliases (run-help=man): a function of the same
  # name would not parse, so the snapshot drops them all and replays only this shell's.
  snapshot="$(
    builtin print -r -- "builtin unalias -m '*' 2>/dev/null; builtin unalias -s -m '*' 2>/dev/null"
    (( $#names )) && builtin typeset -f -- $names
    builtin alias -L
    (( $#on )) && builtin print -r -- "builtin setopt ${(j: :)${(@o)on}} 2>/dev/null"
    (( $#off )) && builtin print -r -- "builtin unsetopt ${(j: :)${(@o)off}} 2>/dev/null"
  )"
  builtin print -r -- $snapshot >| $file || return 0
  builtin zmodload -F zsh/files b:zf_rm
  zf_rm -f -- $file.zwc
  (( $#snapshot > 4096 )) && builtin zcompile -U -- $file
  _efr_hs_sbx_stale=0
  return 0
}

# The three functions as zsh holds them now, for the check of the fixed line. It is
# read-only, so nothing can unset or change it.
builtin typeset -gr _efr_hs_sbx_src="$functions[_efr_hs_sbx]$functions[_efr_hs_sbx_apply]$functions[_efr_hs_sbx_snapshot]"

# No `emulate -L` here: it would make the option changes below local to this function.
_efr_hs_init() {
  _efr_hs_install
  _efr_hs_plain_words

  # See _efr_hs_precmd: the PROMPT_SP mark moves from before the precmd hooks to after D.
  if [[ -o prompt_sp && -o prompt_cr ]]; then
    _efr_hs_prompt_sp=1
  fi

  # The commands come from a model, not from a person at the keyboard: `!` must not
  # expand history, a spelling prompt must not wait for an answer, and the user's
  # history file must not fill up with them. efr keeps its own recording.
  builtin setopt no_bang_hist no_correct no_correct_all no_prompt_sp
  builtin unset HISTFILE

  # The prompt is plain text (see _efr_hs_no_user_hooks), and no prompt expands a
  # parameter or runs a command substitution.
  builtin setopt no_prompt_subst

  # The terminal settings are frozen: a program that leaves the terminal raw or with
  # echo off cannot change how this shell reads the next line.
  builtin ttyctl -f

  # The permission engine counts a pattern such as `x*` as at least one word. With
  # NULL_GLOB or CSH_NULL_GLOB a pattern that matches nothing would vanish, and
  # `systemctl show x*` would run as `systemctl show`.
  builtin setopt no_null_glob no_csh_null_glob

  # No pager either: nobody reads one on the hidden screen, and a command that opens
  # less would wait until someone quit it. The daemon sets these already; they are
  # set again because the user's .zshrc often exports PAGER=less.
  builtin export PAGER=cat GIT_PAGER=cat SYSTEMD_PAGER=cat MANPAGER=cat AWS_PAGER= GH_PAGER=cat BAT_PAGER=cat

  # No editor either, for the same reason: git commit without -m would open one and
  # wait. The stub next to this file fails at once and says why. The daemon sets these
  # already; a .zshrc often exports EDITOR=vim.
  builtin export EDITOR=$_efr_hs_editor VISUAL=$_efr_hs_editor GIT_EDITOR=$_efr_hs_editor GIT_SEQUENCE_EDITOR=$_efr_hs_editor SUDO_EDITOR=$_efr_hs_editor SYSTEMD_EDITOR=$_efr_hs_editor

  # This hook already runs as a precmd hook, so the first prompt is marked from here.
  _efr_hs_precmd
}

# compinit asks before it loads completions from a directory of fpath that other
# users can write to, and waits for a key. Nobody types one in the hidden shell, so
# its first prompt would never come. Ubuntu's /etc/zsh/zshrc calls compinit in every
# interactive shell, and GitHub's Ubuntu runner image makes all of /usr/share
# writable by everyone, zsh's vendor-completions directory among it. So every call,
# from a startup file or from a plugin that loads later, runs as `compinit -i`, which
# leaves those directories out of fpath as the answer y does. A caller's -u or -C
# still wins, because compinit reads its options in order.
_efr_hs_compinit() {
  builtin unfunction compinit
  builtin autoload -Uz compinit
  {
    compinit -i "$@"
  } always {
    # compinit turns itself back into a plain autoload function when it ends.
    _efr_hs_wrap_compinit
  }
}

# The `function` keyword keeps an alias named compinit from renaming the wrapper.
_efr_hs_wrap_compinit() {
  function compinit { _efr_hs_compinit "$@"; }
}

_efr_hs_wrap_compinit

builtin typeset -ga precmd_functions preexec_functions chpwd_functions
precmd_functions+=(_efr_hs_init)
