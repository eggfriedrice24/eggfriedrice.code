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
# functions and arm _efr_init, which runs at the first prompt, after every startup
# file, and installs the hooks around whatever the user's files set up.

# 0: nothing shown yet, 1: a prompt is shown, 2: a command line runs.
builtin typeset -gi _efr_state=0
builtin typeset -g _efr_pwd=
# 1 when the user's options asked for zsh's PROMPT_SP mark; see _efr_precmd.
builtin typeset -gi _efr_prompt_sp=0

_efr_report_pwd() {
  builtin emulate -L zsh
  [[ $PWD == "$_efr_pwd" ]] && return 0
  # A control character in the path would end the OSC sequence early.
  [[ $PWD == *[[:cntrl:]]* ]] && return 0
  _efr_pwd=$PWD
  builtin print -rn -- $'\e]7;kitty-shell-cwd://'"${HOST}${PWD}"$'\a'
}

_efr_precmd() {
  builtin local -i st=$?
  builtin emulate -L zsh
  # Some plugins run precmd hooks from inside a widget to refresh the prompt; a mark
  # printed then would land in the middle of the line editor's display.
  builtin zle && return 0
  _efr_report_pwd
  if (( _efr_state == 2 )); then
    builtin print -rn -- $'\e]133;D;'"${st}"$'\a'
  elif (( _efr_state == 1 )); then
    builtin print -rn -- $'\e]133;D\a'
  fi
  # zsh prints its PROMPT_SP mark before any precmd hook runs, which would put it
  # inside the output of every command, so _efr_init turns the option off and the
  # same mark is printed here, after D.
  if (( _efr_prompt_sp )); then
    builtin printf '%s%*s\r \r' "${(%)PROMPT_EOL_MARK-%B%S%#%s%b}" $(( COLUMNS - 1 )) ''
  fi
  builtin print -rn -- $'\e]133;A;cl=line\a'
  _efr_state=1
  # Hooks that plugins add later go to the end; this hook must stay last so that no
  # other hook's output counts as the next command's.
  preexec_functions=(${preexec_functions:#_efr_preexec} _efr_preexec)
}

_efr_preexec() {
  builtin emulate -L zsh
  # The precmd array is reordered here and not in precmd, where zsh walks it.
  precmd_functions=(_efr_precmd ${precmd_functions:#_efr_precmd})
  builtin print -rn -- $'\e]133;C\a'
  _efr_state=2
}

_efr_line_init() {
  builtin emulate -L zsh
  case $CONTEXT in
    start) builtin print -rn -- $'\e]133;B\a' ;;
    cont) builtin print -rn -- $'\e]133;P;k=s\a\e]133;B\a' ;;
  esac
}

_efr_install() {
  builtin emulate -L zsh
  precmd_functions=(${precmd_functions:#_efr_init})

  # zsh loads the line editor when it first starts it, which is after this first
  # precmd, and add-zle-hook-widget gives up when the module is not loaded yet.
  builtin zmodload zsh/zle
  builtin autoload -Uz add-zle-hook-widget
  add-zle-hook-widget line-init _efr_line_init
  chpwd_functions=(${chpwd_functions:#_efr_report_pwd} _efr_report_pwd)
  preexec_functions=(${preexec_functions:#_efr_preexec} _efr_preexec)
  precmd_functions=(_efr_precmd ${precmd_functions:#_efr_precmd})

  # efr types each command as one bracketed paste followed by Enter. The paste key is
  # bound to the plain builtin widget here, so plugins that rewrite pasted text (such
  # as url-quote-magic) cannot change what runs. A line that is unfinished (an
  # unclosed quote) is cancelled with a key bound to send-break: unlike Ctrl+C, a key
  # waits in the input until the line editor reads it, while a SIGINT that arrives
  # during zle-line-init is lost.
  builtin zle -A .bracketed-paste _efr_bracketed_paste
  builtin local keymap
  for keymap in emacs viins vicmd; do
    builtin bindkey -M $keymap $'\e[200~' _efr_bracketed_paste
    builtin bindkey -M $keymap $'\e[efr-cancel~' send-break
  done
}

# No `emulate -L` here: it would make the option changes below local to this function.
_efr_init() {
  _efr_install

  # See _efr_precmd: the PROMPT_SP mark moves from before the precmd hooks to after D.
  if [[ -o prompt_sp && -o prompt_cr ]]; then
    _efr_prompt_sp=1
  fi

  # The commands come from a model, not from a person at the keyboard: `!` must not
  # expand history, a spelling prompt must not wait for an answer, and the user's
  # history file must not fill up with them. efr keeps its own recording.
  builtin setopt no_bang_hist no_correct no_correct_all no_prompt_sp
  builtin unset HISTFILE

  # This hook already runs as a precmd hook, so the first prompt is marked from here.
  _efr_precmd
}

builtin typeset -ga precmd_functions preexec_functions chpwd_functions
precmd_functions+=(_efr_init)
