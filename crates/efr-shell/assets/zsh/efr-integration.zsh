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
# would replace these hooks and the hidden shell would never print A, C or D.

# The programs that a permission rule trusts by name, from the daemon. An alias or a
# function of the same name from the user's startup files would run something else
# for a line that the engine allowed. Read once and kept out of every child's
# environment. The user's .zshenv has run, so the `-` keeps NO_UNSET from stopping
# this file when the daemon passed none.
builtin typeset -ga _efr_hs_trusted
_efr_hs_trusted=(${(s: :)_EFR_HS_TRUSTED_PROGRAMS-})
builtin unset _EFR_HS_TRUSTED_PROGRAMS

# 0: nothing shown yet, 1: a prompt is shown, 2: a command line runs.
builtin typeset -gi _efr_hs_state=0
builtin typeset -g _efr_hs_pwd=
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

_efr_hs_precmd() {
  builtin local -i st=$?
  builtin emulate -L zsh
  # Some plugins run precmd hooks from inside a widget to refresh the prompt; a mark
  # printed then would land in the middle of the line editor's display.
  builtin zle && return 0
  _efr_hs_report_pwd
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
  # Hooks that plugins add later go to the end; this hook must stay last so that no
  # other hook's output counts as the next command's.
  preexec_functions=(${preexec_functions:#_efr_hs_preexec} _efr_hs_preexec)
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
  builtin emulate -L zsh
  # The precmd array is reordered here and not in precmd, where zsh walks it.
  precmd_functions=(_efr_hs_precmd ${precmd_functions:#_efr_hs_precmd})
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
  builtin autoload -Uz add-zle-hook-widget
  add-zle-hook-widget line-init _efr_hs_line_init
  chpwd_functions=(${chpwd_functions:#_efr_hs_report_pwd} _efr_hs_report_pwd)
  preexec_functions=(${preexec_functions:#_efr_hs_preexec} _efr_hs_preexec)
  precmd_functions=(_efr_hs_precmd ${precmd_functions:#_efr_hs_precmd})

  # efr types each command as a key that empties the line, one bracketed paste and
  # Enter. Text that someone typed at the attached screen and did not send would
  # otherwise join the command: `rm -rf ` left at the prompt and a pasted `build/tmp`
  # would run `rm -rf build/tmp`. The paste key is bound to the plain builtin widget
  # here, so plugins that rewrite pasted text (such as url-quote-magic) cannot change
  # what runs. A line that is unfinished (an unclosed quote) is cancelled with a key
  # bound to send-break: unlike Ctrl+C, a key waits in the input until the line
  # editor reads it, while a SIGINT that arrives during zle-line-init is lost.
  builtin zle -N _efr_hs_clear_line
  builtin zle -A .bracketed-paste _efr_hs_bracketed_paste
  builtin local keymap
  for keymap in emacs viins vicmd; do
    builtin bindkey -M $keymap $'\e[efr-clear~' _efr_hs_clear_line
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

# A command line that the permission engine allowed must run as written. A global
# alias (`alias -g L='| less'`, as oh-my-zsh's common-aliases defines) expands any
# word of a line into pipes or other programs, and a suffix alias (`alias -s txt=vim`)
# runs a program for a word that only names a file, so the hidden shell keeps neither,
# nor an alias or a function named like a program that a rule trusts.
_efr_hs_plain_words() {
  builtin emulate -L zsh
  builtin zmodload zsh/parameter
  (( ${#galiases} )) && builtin unalias -- ${(k)galiases}
  (( ${#saliases} )) && builtin unalias -s -- ${(k)saliases}
  builtin local name
  for name in $_efr_hs_trusted; do
    (( ${+aliases[$name]} )) && builtin unalias -- $name
    (( ${+functions[$name]} )) && builtin unfunction -- $name
  done
  return 0
}

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

  # The permission engine counts a pattern such as `x*` as at least one word. With
  # NULL_GLOB or CSH_NULL_GLOB a pattern that matches nothing would vanish, and
  # `systemctl show x*` would run as `systemctl show`.
  builtin setopt no_null_glob no_csh_null_glob

  # No pager either: nobody reads one on the hidden screen, and a command that opens
  # less would wait until someone quit it. The daemon sets these already; they are
  # set again because the user's .zshrc often exports PAGER=less.
  builtin export PAGER=cat GIT_PAGER=cat SYSTEMD_PAGER=cat MANPAGER=cat

  # This hook already runs as a precmd hook, so the first prompt is marked from here.
  _efr_hs_precmd
}

builtin typeset -ga precmd_functions preexec_functions chpwd_functions
precmd_functions+=(_efr_hs_init)
