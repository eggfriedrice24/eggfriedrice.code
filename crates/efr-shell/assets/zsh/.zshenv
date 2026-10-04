# efr: the ZDOTDIR shim of a hidden conversation shell.
#
# efr starts the hidden zsh with ZDOTDIR pointing at this directory, so zsh reads this
# file first. It hands the user's own startup files back to zsh and then loads the
# efr integration:
#
# 1. ZDOTDIR goes back to the user's value (kept in _EFR_USER_ZDOTDIR) or is unset,
#    so zsh reads the user's .zprofile, .zshrc and .zlogin next, as in any terminal,
#    and child shells never see this directory.
# 2. The user's .zshenv is sourced here, because zsh has already passed that step.
# 3. In an interactive shell, efr-integration.zsh from this directory is sourced. It
#    only defines functions and arms a hook for the first prompt, so it works whatever
#    the user's .zshrc does later.
#
# `builtin` keeps an alias from a global startup file away from these commands.

builtin typeset _efr_dir=${${(%):-%x}:A:h}

if [[ -n ${_EFR_USER_ZDOTDIR+set} ]]; then
  builtin export ZDOTDIR=$_EFR_USER_ZDOTDIR
  builtin unset _EFR_USER_ZDOTDIR
else
  builtin unset ZDOTDIR
fi

{
  # An unset ZDOTDIR means $HOME to zsh, and zsh skips unreadable startup files.
  builtin typeset _efr_rc=${ZDOTDIR:-$HOME}/.zshenv
  [[ -r $_efr_rc ]] && builtin source -- $_efr_rc
} always {
  if [[ -o interactive && -r $_efr_dir/efr-integration.zsh ]]; then
    builtin source -- $_efr_dir/efr-integration.zsh
  fi
  builtin unset _efr_dir _efr_rc
}
