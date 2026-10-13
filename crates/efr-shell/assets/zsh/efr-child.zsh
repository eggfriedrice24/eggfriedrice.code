# efr: the child shell of one call that efr-sbx runs for the auto mode.
#
# Written for efr. efr-sbx starts it as `zsh -f efr-child.zsh DIR`, inside the sandbox
# for a contained call and outside it for an approved exit. DIR holds:
#
#   snapshot.zsh  the trusted shell's functions, aliases and options
#   state.zsh     what earlier contained calls left; the exit child's DIR has none,
#                 because it never runs anything that the sandbox made
#   line          the model's command line
#   mounts        the host's mount table, with mounts.json, its sizes: efrd writes
#                 them for a line that runs df or findmnt (the mount view)
#
# A file that is missing skips that step. The script replays the snapshot and the state,
# notes the exported variables, functions and aliases, evaluates the line, and then
# reports on descriptor 3 what changed, in the records format of efr-sandbox:
#
#   efr-records NUL v1 NUL
#   cwd NUL <dir> NUL
#   export NUL <name> NUL <value> NUL      unset NUL <name> NUL
#   func NUL <name> NUL <body> NUL         unfunc NUL <name> NUL
#   alias NUL <name> NUL <value> NUL       unalias NUL <name> NUL
#   end NUL <status> NUL
#
# The line runs in this shell and can change everything here, these records included.
# efr-sbx treats them as untrusted data: it checks every name and value again, and a
# stream without its end record keeps nothing. The report is written by an EXIT trap,
# so it also comes after `exit N`.
#
# Ctrl+C (SIGINT) and Ctrl+\ (SIGQUIT) must stop the line, also when they come before
# it starts. A zsh that is not interactive ignores SIGQUIT, and a TRAPINT or TRAPQUIT
# function of the snapshot or the state can catch a signal and let the shell go on. So
# until the line starts, a trap of this script only notes the signal. Before the line,
# the gate gives both signals their default action and ends the shell with 128 plus the
# signal when one came; a signal after the gate ends the shell and the line.

# The signal that came before the line, or 0.
builtin typeset -gi _efr_child_stop=0

# Notes SIGINT and SIGQUIT in _efr_child_stop. Setting the traps also removes a TRAPINT
# or TRAPQUIT function. The functions of this script are defined before the snapshot
# and the state, so that their aliases do not change them.
_efr_child_note_signals() {
  # The traps stay after the function, also when the snapshot set LOCAL_TRAPS.
  builtin setopt local_options
  builtin unsetopt local_traps
  builtin trap '_efr_child_stop=2' INT
  builtin trap '_efr_child_stop=3' QUIT
}

# The gate before the line: from here, SIGINT and SIGQUIT have their default action and
# end this shell. A signal that came before is in _efr_child_stop; then the shell ends
# with 128 plus the signal, and the line never runs.
_efr_child_gate() {
  builtin setopt local_options
  builtin unsetopt local_traps
  builtin trap - INT QUIT
  (( _efr_child_stop )) && builtin exit $(( 128 + _efr_child_stop ))
  return 0
}

# df with the host's mounts: the snapshot's table, printed by the launcher next to this
# script, for a line of options only. An operand, `--`, or an option that the table
# does not follow (the launcher then exits 2 with no output) runs the real df, which
# shows the sandbox's own file system: for a path, that is where a write really goes.
_efr_child_df() {
  builtin emulate -L zsh
  builtin local arg
  for arg in "$@"; do
    [[ $arg == -- ]] && { command df "$@"; return }
  done
  "$_efr_child_dir/efr-sbx" mounts df "$@"
  builtin local -i st=$?
  (( st == 2 )) && { command df "$@"; return }
  return st
}

# Returns 0 when findmnt's arguments pick a table of their own: a file, another task,
# the kernel, fstab, the poll mode or the verify mode. A long option counts in any
# abbreviation of these.
_efr_child_findmnt_own_table() {
  builtin emulate -L zsh
  builtin local arg name letters letter
  builtin local -a own takes
  own=(tab-file task kernel fstab poll verify mtab)
  takes=(direction mountpoint options output filter source target types timeout id uniq-id)
  builtin integer skip=0
  for arg in "$@"; do
    if (( skip )); then
      skip=0
      continue
    fi
    case $arg in
      (--) return 1 ;;
      (--?*)
        name=${${arg#--}%%=*}
        for letters in $own; do
          [[ $letters == ${name}* ]] && return 0
        done
        if [[ $arg != *=* ]]; then
          for letters in $takes; do
            [[ $letters == ${name}* ]] && skip=1
          done
        fi
        ;;
      (-?*)
        letters=${arg#-}
        while [[ -n $letters ]]; do
          letter=${letters[1]}
          letters=${letters[2,-1]}
          case $letter in
            ([FNkspxm]) return 0 ;;
            ([dMOoQSTtw])
              [[ -z $letters ]] && skip=1
              break
              ;;
          esac
        done
        ;;
    esac
  done
  return 1
}

# findmnt with the host's mounts: the snapshot's table as findmnt's tab file, and for
# -D, -I and their long forms alone, the sizes of the snapshot. Arguments that pick a
# table of their own run the real findmnt as they are.
_efr_child_findmnt() {
  builtin emulate -L zsh
  _efr_child_findmnt_own_table "$@" && { command findmnt "$@"; return }
  builtin local arg
  builtin integer sizes=$#
  for arg in "$@"; do
    [[ $arg == (-D|--df|-I|--dfi) ]] || sizes=0
  done
  if (( sizes )); then
    "$_efr_child_dir/efr-sbx" mounts findmnt-df "$@"
    builtin local -i st=$?
    (( st == 2 )) || return st
  fi
  command findmnt --tab-file "$_efr_child_dir/mounts" "$@"
}

_efr_child_note_signals

builtin typeset -g _efr_child_dir=${1-}
builtin zmodload zsh/parameter

[[ -n $_efr_child_dir && -r $_efr_child_dir/snapshot.zsh ]] &&
  builtin source -- $_efr_child_dir/snapshot.zsh
[[ -n $_efr_child_dir && -r $_efr_child_dir/state.zsh ]] &&
  builtin source -- $_efr_child_dir/state.zsh

# NOTE: again after the snapshot and the state, which can define TRAPINT or TRAPQUIT,
# and before the start lists below, so the records do not report those as removed.
_efr_child_note_signals

# The mount view: when efrd wrote the snapshot of the host's mounts for this call, df
# and findmnt print the host's mounts. A function of that name from the snapshot or the
# state wins. The `function` form takes the name with no alias of the snapshot, and
# the bodies above were read before the snapshot's aliases. The records never report
# these two functions.
builtin typeset -ga _efr_child_own_fns
if [[ -n $_efr_child_dir && -r $_efr_child_dir/mounts ]]; then
  if (( ! $+functions[df] )); then
    function df { _efr_child_df "$@" }
    _efr_child_own_fns+=(df)
  fi
  if (( ! $+functions[findmnt] )); then
    function findmnt { _efr_child_findmnt "$@" }
    _efr_child_own_fns+=(findmnt)
  fi
fi

# Names that change in every shell or that efr sets; they are never reported.
builtin typeset -ga _efr_child_skip
_efr_child_skip=(PWD OLDPWD SHLVL _ EFR_SANDBOX)

# The exported scalars as name value pairs.
_efr_child_exports() {
  builtin emulate -L zsh -o extended_glob
  builtin local name
  reply=()
  for name in ${(k)parameters[(R)*export*]}; do
    [[ $parameters[$name] == *(array|association)* ]] && continue
    (( ${_efr_child_skip[(Ie)$name]} )) && continue
    reply+=("$name" "${(P)name}")
  done
}

# What the line starts with, as name value lists. A function, so that the user's
# options from the snapshot (KSH_ARRAYS, say) do not change how the lists are read.
builtin typeset -ga _efr_child_env0 _efr_child_fn0 _efr_child_al0
_efr_child_start() {
  builtin emulate -L zsh
  _efr_child_exports
  _efr_child_env0=("${(@)reply}")
  _efr_child_fn0=("${(@kv)functions}")
  _efr_child_al0=("${(@kv)aliases}")
}
_efr_child_start

# Writes the records of the call on descriptor 3; nothing when it is not open.
_efr_child_records() {
  builtin local -i st=$1
  builtin emulate -L zsh -o extended_glob
  builtin local -A was now
  builtin local -a out list
  builtin local name
  out=(efr-records v1 cwd "$PWD")
  # NOTE: each list is first compared whole, which costs one pass; only a list that
  # changed is compared name by name. A line seldom changes one, and a big rc has
  # hundreds of functions and aliases.
  _efr_child_exports
  if [[ ${(pj:\0:)reply} != ${(pj:\0:)_efr_child_env0} ]]; then
    was=("${(@)_efr_child_env0}")
    now=("${(@)reply}")
    for name in ${(k)now}; do
      [[ ${+was[$name]} == 1 && $was[$name] == "$now[$name]" ]] ||
        out+=(export "$name" "$now[$name]")
    done
    for name in ${(k)was}; do
      (( ${+now[$name]} )) || out+=(unset "$name")
    done
  fi
  list=("${(@kv)functions}")
  if [[ ${(pj:\0:)list} != ${(pj:\0:)_efr_child_fn0} ]]; then
    was=("${(@)_efr_child_fn0}")
    now=("${(@)list}")
    for name in ${(k)now:#_efr_*}; do
      (( ${_efr_child_own_fns[(Ie)$name]} )) && continue
      [[ ${+was[$name]} == 1 && $was[$name] == "$now[$name]" ]] ||
        out+=(func "$name" "$now[$name]")
    done
    for name in ${(k)was:#_efr_*}; do
      (( ${_efr_child_own_fns[(Ie)$name]} )) && continue
      (( ${+now[$name]} )) || out+=(unfunc "$name")
    done
  fi
  list=("${(@kv)aliases}")
  if [[ ${(pj:\0:)list} != ${(pj:\0:)_efr_child_al0} ]]; then
    was=("${(@)_efr_child_al0}")
    now=("${(@)list}")
    for name in ${(k)now}; do
      [[ ${+was[$name]} == 1 && $was[$name] == "$now[$name]" ]] ||
        out+=(alias "$name" "$now[$name]")
    done
    for name in ${(k)was}; do
      (( ${+now[$name]} )) || out+=(unalias "$name")
    done
  fi
  out+=(end "$st")
  { builtin print -rN -- "${(@)out}" } 2>/dev/null >&3
  return 0
}

builtin trap '_efr_child_records $?' EXIT

_efr_child_gate

# The line runs at the top level, as it would at a prompt, with the user's options
# from the snapshot.
if [[ -n $_efr_child_dir && -r $_efr_child_dir/line ]]; then
  builtin eval -- "$(<$_efr_child_dir/line)"
fi
